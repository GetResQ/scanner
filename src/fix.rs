use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tokio::sync::mpsc::Sender;

use crate::config::Agent;
use crate::gha::{Annotation, AnnotationLevel, is_error_level};
use crate::pool::Pool;
use crate::process;
use crate::runner::CheckResult;
use crate::ui::{UiEvent, sanitize_text_for_tui};

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
struct SolverInput<'a> {
    task: &'static str,
    groups: Vec<SolverGroup<'a>>,
}

#[derive(Debug, Serialize)]
struct SolverGroup<'a> {
    check: &'a str,
    error_type: String,
    files: Vec<String>,
    annotations: Vec<SerializableAnnotation<'a>>,
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
            if !ann.actionable {
                continue;
            }
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

const SOLVER_TASK: &str = "\
Analyze the following build/lint errors and apply fixes directly in the referenced files. \
Read the files to understand the code context, then edit them to resolve the errors. \
Be precise and minimal - only change what is necessary to fix the errors.";

/// Run a solver agent for a single check's error groups.
pub async fn run_solver(
    agent: &Agent,
    groups: &[ErrorGroup],
    root: &std::path::Path,
) -> Result<String> {
    let input = SolverInput {
        task: SOLVER_TASK,
        groups: groups
            .iter()
            .map(|g| SolverGroup {
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

/// Run the full solve pipeline for all failed checks.
/// Each check type gets its own solver run.
pub async fn run_fix_pipeline(
    agent: &Agent,
    errors_by_check: &HashMap<String, Vec<ErrorGroup>>,
    pool: &Pool,
    root: &std::path::Path,
    ui_tx: Option<Sender<UiEvent>>,
) -> Result<()> {
    let mut handles = Vec::new();

    for (check_name, groups) in errors_by_check {
        let check_name = check_name.clone();
        let check_name_for_join = check_name.clone();
        let groups = groups.clone();
        let agent = agent.clone();
        let root = root.to_path_buf();
        let ui_tx = ui_tx.clone();

        let handle = pool.spawn(async move {
            if let Some(tx) = ui_tx.as_ref() {
                let _ = tx
                    .send(UiEvent::CheckStarted {
                        name: format!("solve:{}", check_name),
                        desc: Some(format!("Fixing {} errors", check_name)),
                    })
                    .await;
            }

            let result = run_solver(&agent, &groups, &root)
                .await
                .with_context(|| format!("solver failed for {check_name}"));

            if let Some(tx) = ui_tx.as_ref() {
                let (success, msg, output) = match &result {
                    Ok(text) => {
                        let trimmed = text.trim();
                        let output = if trimmed.is_empty() {
                            None
                        } else {
                            Some(sanitize_text_for_tui(trimmed))
                        };
                        (true, "applied".to_string(), output)
                    }
                    Err(e) => {
                        let text = format!("{e:#}");
                        (false, text.clone(), Some(sanitize_text_for_tui(&text)))
                    }
                };
                let _ = tx
                    .send(UiEvent::CheckFinished {
                        name: format!("solve:{}", check_name),
                        success,
                        message: msg,
                        output,
                    })
                    .await;
            }

            result.map(|_| ())
        });

        handles.push((check_name_for_join, handle));
    }

    let mut errors = Vec::new();
    for (check_name, handle) in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(join_err) => {
                errors.push(anyhow!("solver panicked for {check_name}: {join_err:?}"));
                if let Some(tx) = ui_tx.as_ref() {
                    let msg = format!("panic: {join_err:?}");
                    let _ = tx
                        .send(UiEvent::CheckFinished {
                            name: format!("solve:{}", check_name),
                            success: false,
                            message: "panic".to_string(),
                            output: Some(sanitize_text_for_tui(&msg)),
                        })
                        .await;
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        let msg = errors
            .into_iter()
            .enumerate()
            .map(|(idx, e)| format!("{}. {e:#}", idx + 1))
            .collect::<Vec<_>>()
            .join("\n");
        Err(anyhow!("solve pipeline failed:\n{msg}"))
    }
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
    use crate::config::{Agent, Check, CommandSpec};
    use crate::pool::Pool;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let mut path = std::env::temp_dir();
            path.push(format!("scanner-rs-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    fn sh_agent(script: &str) -> Agent {
        Agent {
            command: CommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), script.to_string()],
            },
            env: HashMap::new(),
            timeout: None,
        }
    }

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
            lock: None,
        }
    }

    fn make_result(
        check: Check,
        exit_code: Option<i32>,
        annotations: Vec<Annotation>,
    ) -> CheckResult {
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
            actionable: true,
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

        let e0001_group = lint_groups
            .iter()
            .find(|g| g.error_type == "E0001")
            .unwrap();
        assert_eq!(e0001_group.files.len(), 2);
        assert_eq!(e0001_group.annotations.len(), 2);

        let e0002_group = lint_groups
            .iter()
            .find(|g| g.error_type == "E0002")
            .unwrap();
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
    fn group_errors_skips_non_actionable_annotations() {
        let results = vec![make_result(
            make_check("lint"),
            Some(1),
            vec![Annotation {
                level: AnnotationLevel::Error,
                actionable: false,
                file: None,
                line: None,
                end_line: None,
                column: None,
                end_column: None,
                title: Some("no annotations".to_string()),
                message: "configure formatter".to_string(),
            }],
        )];

        let grouped = group_errors_by_check(&results);

        assert!(grouped.is_empty());
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

    #[cfg(unix)]
    #[tokio::test]
    async fn solve_pipeline_propagates_solver_failures() {
        let solver = sh_agent("cat >/dev/null; echo solver failed >&2; exit 3");
        let pool = Pool::new(2);
        let root = TempDir::new("solve-pipeline-fail");

        let mut errors_by_check = HashMap::new();
        errors_by_check.insert(
            "lint".to_string(),
            vec![ErrorGroup {
                check: "lint".to_string(),
                error_type: "E1".to_string(),
                files: vec!["a.rs".to_string()],
                annotations: vec![make_error(Some("a.rs"), Some("E1"), "error")],
            }],
        );

        let err = run_fix_pipeline(&solver, &errors_by_check, &pool, root.path(), None)
            .await
            .expect_err("expected run_fix_pipeline to fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("solve pipeline failed"));
        assert!(msg.contains("solver failed for lint"));
    }
}
