use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json;
use tokio::sync::mpsc::Sender;

use crate::config::Agent;
use crate::error::FixError;
use crate::gha::{Annotation, AnnotationLevel, is_error_level};
use crate::pool::Pool;
use crate::process;
use crate::runner::CheckResult;
use crate::ui::UiEvent;

#[derive(Debug, Serialize)]
struct SerializableAnnotation<'a> {
    level: &'a str,
    file: Option<String>,
    line: Option<u64>,
    end_line: Option<u64>,
    column: Option<u64>,
    end_column: Option<u64>,
    title: Option<&'a str>,
    message: &'a str,
}

#[derive(Debug, Serialize)]
struct AnalyzerInput<'a> {
    groups: Vec<AnalyzerGroup<'a>>,
}

#[derive(Debug, Serialize)]
struct AnalyzerGroup<'a> {
    check: &'a str,
    error_type: String,
    files: Vec<String>,
    annotations: Vec<SerializableAnnotation<'a>>,
}

#[derive(Debug, Serialize)]
struct FixerInput<'a> {
    check: &'a str,
    error_type: &'a str,
    analysis: &'a str,
    files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ErrorGroup {
    pub check: String,
    pub error_type: String,
    pub files: Vec<String>,
    pub annotations: Vec<Annotation>,
}

/// Groups errors by check name, returning a map of check -> error groups.
pub fn group_errors_by_check(results: &[CheckResult]) -> HashMap<String, Vec<ErrorGroup>> {
    let mut grouped: HashMap<(String, String), (HashSet<String>, Vec<Annotation>)> = HashMap::new();

    for result in results {
        let treat_non_errors = result.exit_code != Some(0);
        for ann in &result.annotations {
            if !is_error_level(ann.level) && !treat_non_errors {
                continue;
            }
            let key = error_key(ann);
            let entry = grouped
                .entry((result.check.name.clone(), key))
                .or_insert_with(|| (HashSet::new(), Vec::new()));
            if let Some(path) = ann.file.as_ref() {
                entry.0.insert(path.display().to_string());
            }
            entry.1.push(ann.clone());
        }

        // If the process failed but no annotations, treat the whole check as one error group.
        if result.exit_code != Some(0) && result.annotations.is_empty() {
            let entry = grouped
                .entry((result.check.name.clone(), "process-failed".to_string()))
                .or_insert_with(|| (HashSet::new(), Vec::new()));
            entry.1.push(Annotation {
                level: AnnotationLevel::Error,
                file: None,
                line: None,
                end_line: None,
                column: None,
                end_column: None,
                title: Some("process failed".to_string()),
                message: result
                    .raw_output
                    .lines()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
        }
    }

    let mut by_check: HashMap<String, Vec<ErrorGroup>> = HashMap::new();
    for ((check, error_type), (files, anns)) in grouped {
        by_check.entry(check.clone()).or_default().push(ErrorGroup {
            check,
            error_type,
            files: files.into_iter().collect(),
            annotations: anns,
        });
    }
    by_check
}

/// Legacy function for backward compatibility - returns flat list of all error groups.
#[allow(dead_code)]
pub fn group_errors(results: &[CheckResult]) -> Vec<ErrorGroup> {
    group_errors_by_check(results)
        .into_values()
        .flatten()
        .collect()
}

fn error_key(ann: &Annotation) -> String {
    ann.title
        .as_ref()
        .cloned()
        .unwrap_or_else(|| ann.message.clone())
}

/// Run analyzer for a single check's error groups.
pub async fn run_analyzer(
    agent: &Agent,
    groups: &[ErrorGroup],
    root: &std::path::Path,
) -> Result<String> {
    let input = AnalyzerInput {
        groups: groups
            .iter()
            .map(|g| AnalyzerGroup {
                check: &g.check,
                error_type: g.error_type.clone(),
                files: g.files.clone(),
                annotations: g
                    .annotations
                    .iter()
                    .map(|ann| SerializableAnnotation {
                        level: match ann.level {
                            AnnotationLevel::Error => "error",
                            AnnotationLevel::Warning => "warning",
                            AnnotationLevel::Notice => "notice",
                        },
                        file: ann.file.as_ref().map(|p| p.display().to_string()),
                        line: ann.line,
                        end_line: ann.end_line,
                        column: ann.column,
                        end_column: ann.end_column,
                        title: ann.title.as_deref(),
                        message: &ann.message,
                    })
                    .collect(),
            })
            .collect(),
    };

    let json = serde_json::to_vec(&input)?;
    run_agent_command(agent, &json, root).await
}

/// Run fixer batches for a single check's error groups.
///
/// Each batch is spawned directly on the pool, competing fairly for slots.
/// This avoids the deadlock issue of nested pool spawns while still respecting
/// the pool's concurrency limit.
pub async fn run_fixer_batches(
    agent: &Agent,
    analysis_text: &str,
    groups: &[ErrorGroup],
    batch_size: usize,
    pool: &Pool,
    root: &std::path::Path,
) -> Result<()> {
    if batch_size == 0 {
        return Err(FixError::InvalidBatchSize.into());
    }

    let mut handles = Vec::new();

    for group in groups {
        let batches: Vec<Vec<String>> = group
            .files
            .chunks(batch_size)
            .map(|chunk| chunk.to_vec())
            .collect();

        for batch in batches {
            let agent = agent.clone();
            let analysis = analysis_text.to_string();
            let check = group.check.to_string();
            let error_type = group.error_type.clone();
            let root = root.to_path_buf();

            // Spawn each batch on the pool - they compete fairly for slots
            let handle = pool.spawn(async move {
                let input = FixerInput {
                    check: &check,
                    error_type: &error_type,
                    analysis: &analysis,
                    files: batch,
                };
                let payload = serde_json::to_vec(&input)?;
                run_agent_command(&agent, &payload, &root).await?;
                Ok::<(), anyhow::Error>(())
            });

            handles.push(handle);
        }
    }

    for handle in handles {
        handle.await??;
    }

    Ok(())
}

/// Run the full analyze-then-fix pipeline for all failed checks.
/// Each check type gets its own analyzer -> fixer(s) sequence.
///
/// The pipeline runs in two phases:
/// 1. All analyzers run in parallel (via pool)
/// 2. All fixer batches run in parallel (via pool) - batches compete fairly for slots
pub async fn run_fix_pipeline(
    analyzer_agent: &Agent,
    fixer_agent: &Agent,
    errors_by_check: &HashMap<String, Vec<ErrorGroup>>,
    batch_size: usize,
    pool: &Pool,
    root: &std::path::Path,
    ui_tx: Option<Sender<UiEvent>>,
) -> Result<()> {
    // Phase 1: Run all analyzers in parallel via pool
    let mut analyzer_handles = Vec::new();

    for (check_name, groups) in errors_by_check {
        let check_name = check_name.clone();
        let groups = groups.clone();
        let agent = analyzer_agent.clone();
        let root = root.to_path_buf();
        let ui_tx = ui_tx.clone();

        let handle = pool.spawn(async move {
            // Notify UI that analyzer started
            if let Some(tx) = ui_tx.as_ref() {
                let _ = tx
                    .send(UiEvent::CheckStarted {
                        name: format!("analyze:{}", check_name),
                        desc: Some(format!("Analyzing {} errors", check_name)),
                    })
                    .await;
            }

            let result = run_analyzer(&agent, &groups, &root).await;

            // Notify UI of result
            if let Some(tx) = ui_tx.as_ref() {
                let (success, msg, output) = match &result {
                    Ok(analysis) => (true, "done".to_string(), Some(analysis.clone())),
                    Err(e) => (false, format!("{e:#}"), Some(format!("{e:#}"))),
                };
                let _ = tx
                    .send(UiEvent::CheckFinished {
                        name: format!("analyze:{}", check_name),
                        success,
                        message: msg,
                        output,
                    })
                    .await;
            }

            result.map(|analysis| (check_name, groups, analysis))
        });

        analyzer_handles.push(handle);
    }

    // Collect analyzer results
    let mut analyses = Vec::new();
    for handle in analyzer_handles {
        match handle.await {
            Ok(Ok((check_name, groups, analysis))) => {
                analyses.push((check_name, groups, analysis));
            }
            Ok(Err(e)) => {
                // Analyzer failed - log but continue with other checks
                eprintln!("analyzer error: {e:#}");
            }
            Err(join_err) => {
                eprintln!("analyzer task panic: {join_err:?}");
            }
        }
    }

    // Phase 2: Run all fixer batches in parallel via pool
    // Batches are spawned directly on the pool, competing fairly for slots
    for (check_name, groups, analysis) in analyses {
        // Notify UI that fixer started
        if let Some(tx) = ui_tx.as_ref() {
            let _ = tx
                .send(UiEvent::CheckStarted {
                    name: format!("fix:{}", check_name),
                    desc: Some(format!("Fixing {} errors", check_name)),
                })
                .await;
        }

        // run_fixer_batches spawns batches directly on the pool
        let result = run_fixer_batches(
            fixer_agent,
            &analysis,
            &groups,
            batch_size,
            pool,
            root,
        )
        .await;

        // Notify UI of result
        if let Some(tx) = ui_tx.as_ref() {
            let (success, msg) = match &result {
                Ok(()) => (true, "applied".to_string()),
                Err(e) => (false, format!("{e:#}")),
            };
            let _ = tx
                .send(UiEvent::CheckFinished {
                    name: format!("fix:{}", check_name),
                    success,
                    message: msg,
                    output: None,
                })
                .await;
        }

        if let Err(e) = result {
            eprintln!("fixer error for {}: {e:#}", check_name);
        }
    }

    Ok(())
}

async fn run_agent_command(
    agent: &Agent,
    payload: &[u8],
    root: &std::path::Path,
) -> Result<String> {
    let (code, stdout_buf, stderr_buf) = process::run_command(
        &agent.command,
        &agent.env,
        root,
        agent.timeout,
        Some(payload.to_vec()),
    )
    .await?;

    if code != Some(0) {
        return Err(anyhow!(
            "agent exited with {:?}: {}",
            code,
            String::from_utf8_lossy(&stderr_buf)
        ));
    }

    let mut text = String::from_utf8_lossy(&stdout_buf).to_string();
    if text.is_empty() && !stderr_buf.is_empty() {
        text = String::from_utf8_lossy(&stderr_buf).to_string();
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Check, CommandSpec};
    use std::path::PathBuf;

    fn make_check(name: &str) -> Check {
        Check {
            name: name.to_string(),
            command: CommandSpec {
                program: "echo".to_string(),
                args: vec![],
            },
            formatter: None,
            fixer: None,
            env: HashMap::new(),
            timeout: None,
            enabled: true,
            tags: vec![],
            description: None,
            cwd: None,
        }
    }

    fn make_result(check: Check, exit_code: Option<i32>, annotations: Vec<Annotation>) -> CheckResult {
        CheckResult {
            check,
            exit_code,
            raw_output: String::new(),
            annotations,
        }
    }

    fn make_error(file: Option<&str>, title: Option<&str>, message: &str) -> Annotation {
        Annotation {
            level: AnnotationLevel::Error,
            file: file.map(PathBuf::from),
            line: Some(1),
            end_line: None,
            column: None,
            end_column: None,
            title: title.map(String::from),
            message: message.to_string(),
        }
    }

    #[test]
    fn group_errors_by_check_groups_by_title() {
        let results = vec![make_result(
            make_check("lint"),
            Some(1),
            vec![
                make_error(Some("a.rs"), Some("E0001"), "error 1"),
                make_error(Some("b.rs"), Some("E0001"), "error 2"),
                make_error(Some("c.rs"), Some("E0002"), "different error"),
            ],
        )];

        let grouped = group_errors_by_check(&results);

        assert_eq!(grouped.len(), 1);
        let lint_groups = grouped.get("lint").unwrap();
        assert_eq!(lint_groups.len(), 2); // Two error types: E0001 and E0002

        let e0001_group = lint_groups.iter().find(|g| g.error_type == "E0001").unwrap();
        assert_eq!(e0001_group.files.len(), 2);
        assert_eq!(e0001_group.annotations.len(), 2);

        let e0002_group = lint_groups.iter().find(|g| g.error_type == "E0002").unwrap();
        assert_eq!(e0002_group.files.len(), 1);
    }

    #[test]
    fn group_errors_separate_checks() {
        let results = vec![
            make_result(
                make_check("lint"),
                Some(1),
                vec![make_error(Some("a.rs"), Some("E0001"), "lint error")],
            ),
            make_result(
                make_check("test"),
                Some(1),
                vec![make_error(Some("b.rs"), Some("test_failed"), "test error")],
            ),
        ];

        let grouped = group_errors_by_check(&results);

        assert_eq!(grouped.len(), 2);
        assert!(grouped.contains_key("lint"));
        assert!(grouped.contains_key("test"));
    }

    #[test]
    fn group_errors_handles_process_failure_no_annotations() {
        let results = vec![make_result(make_check("lint"), Some(1), vec![])];

        let grouped = group_errors_by_check(&results);

        assert_eq!(grouped.len(), 1);
        let lint_groups = grouped.get("lint").unwrap();
        assert_eq!(lint_groups.len(), 1);
        assert_eq!(lint_groups[0].error_type, "process-failed");
    }

    #[test]
    fn group_errors_skips_successful_checks() {
        let results = vec![
            make_result(make_check("lint"), Some(0), vec![]),
            make_result(
                make_check("test"),
                Some(1),
                vec![make_error(Some("a.rs"), Some("E0001"), "error")],
            ),
        ];

        let grouped = group_errors_by_check(&results);

        // Only "test" should be grouped (lint succeeded with exit 0)
        assert_eq!(grouped.len(), 1);
        assert!(grouped.contains_key("test"));
    }

    #[test]
    fn group_errors_uses_message_as_fallback_key() {
        let results = vec![make_result(
            make_check("lint"),
            Some(1),
            vec![
                make_error(Some("a.rs"), None, "same message"),
                make_error(Some("b.rs"), None, "same message"),
            ],
        )];

        let grouped = group_errors_by_check(&results);

        let lint_groups = grouped.get("lint").unwrap();
        assert_eq!(lint_groups.len(), 1); // Both grouped under same message
        assert_eq!(lint_groups[0].files.len(), 2);
    }
}
