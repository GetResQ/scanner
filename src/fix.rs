use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc::Sender};

use crate::config::Agent;
use crate::error::FixError;
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
struct AnalyzerInput<'a> {
    task: &'static str,
    groups: Vec<AnalyzerGroup<'a>>,
}

#[derive(Debug, Serialize)]
struct FixerPrompt<'a> {
    task: &'static str,
    check: &'a str,
    error_type: &'a str,
    analysis: &'a str,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AnalyzerGroup<'a> {
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

const ANALYZER_TASK: &str = "\
Analyze the following build/lint errors. Read the referenced files to understand the code context. \
Output a concise fix strategy describing how to resolve each error type. \
Do not modify any files - only analyze and describe the fixes needed.";

const FIXER_TASK: &str = "\
Apply the fix strategy from the analysis to resolve the errors in the listed files. \
Edit each file to fix the errors. Be precise and minimal - only change what is necessary to fix the errors.";

/// Run analyzer for a single check's error groups.
pub async fn run_analyzer(
    agent: &Agent,
    groups: &[ErrorGroup],
    root: &std::path::Path,
) -> Result<String> {
    let input = AnalyzerInput {
        task: ANALYZER_TASK,
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

    let file_locks = Arc::new(build_file_locks(groups));
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
            let batch_len = batch.len();
            let file_locks = file_locks.clone();

            // Spawn each batch on the pool - they compete fairly for slots
            let handle = pool.spawn(async move {
                let _permits = acquire_file_locks(&file_locks, &batch).await?;
                let prompt = FixerPrompt {
                    task: FIXER_TASK,
                    check: &check,
                    error_type: &error_type,
                    analysis: &analysis,
                    files: batch,
                };
                let payload = serde_json::to_vec(&prompt)?;
                run_agent_command(&agent, &payload, &root)
                    .await
                    .with_context(|| {
                        format!("fixer batch failed for {check}:{error_type} ({batch_len} file(s))")
                    })?;
                Ok::<(), anyhow::Error>(())
            });

            handles.push(handle);
        }
    }

    let mut errors = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(join_err) => errors.push(anyhow!("fixer batch panicked: {join_err:?}")),
        }
    }

    if !errors.is_empty() {
        let msg = errors
            .into_iter()
            .enumerate()
            .map(|(idx, e)| format!("{}. {e:#}", idx + 1))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow!("one or more fixer batches failed:\n{msg}"));
    }

    Ok(())
}

fn build_file_locks(groups: &[ErrorGroup]) -> HashMap<String, Arc<Semaphore>> {
    let mut locks = HashMap::new();
    for group in groups {
        for file in &group.files {
            locks
                .entry(file.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(1)));
        }
    }
    locks
}

async fn acquire_file_locks(
    file_locks: &HashMap<String, Arc<Semaphore>>,
    files: &[String],
) -> Result<Vec<OwnedSemaphorePermit>> {
    let mut files = files.to_vec();
    files.sort_unstable();
    files.dedup();

    let mut permits = Vec::with_capacity(files.len());
    for file in files {
        let sem = file_locks
            .get(&file)
            .ok_or_else(|| anyhow!("missing file lock for '{file}'"))?
            .clone();
        permits.push(
            sem.acquire_owned()
                .await
                .map_err(|_| anyhow!("file lock closed unexpectedly for '{file}'"))?,
        );
    }
    Ok(permits)
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
        let check_name_for_join = check_name.clone();
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
                    Ok(analysis) => (
                        true,
                        "done".to_string(),
                        Some(sanitize_text_for_tui(analysis)),
                    ),
                    Err(e) => {
                        let text = format!("{e:#}");
                        (false, text.clone(), Some(sanitize_text_for_tui(&text)))
                    }
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

        analyzer_handles.push((check_name_for_join, handle));
    }

    // Collect analyzer results
    let mut analyses = Vec::new();
    let mut errors = Vec::new();
    for (check_name, handle) in analyzer_handles {
        match handle.await {
            Ok(Ok((check_name, groups, analysis))) => {
                analyses.push((check_name, groups, analysis));
            }
            Ok(Err(e)) => errors.push(e.context(format!("analyzer failed for {check_name}"))),
            Err(join_err) => {
                errors.push(anyhow!("analyzer panicked for {check_name}: {join_err:?}"));
                if let Some(tx) = ui_tx.as_ref() {
                    let msg = format!("panic: {join_err:?}");
                    let _ = tx
                        .send(UiEvent::CheckFinished {
                            name: format!("analyze:{}", check_name),
                            success: false,
                            message: "panic".to_string(),
                            output: Some(sanitize_text_for_tui(&msg)),
                        })
                        .await;
                }
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
        let result =
            run_fixer_batches(fixer_agent, &analysis, &groups, batch_size, pool, root).await;

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
            errors.push(e.context(format!("fixer failed for {check_name}")));
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
        Err(anyhow!("fix pipeline failed:\n{msg}"))
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
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    #[test]
    fn build_file_locks_deduplicates_files_across_groups() {
        let groups = vec![
            ErrorGroup {
                check: "lint".to_string(),
                error_type: "E1".to_string(),
                files: vec!["a.rs".to_string(), "b.rs".to_string()],
                annotations: vec![make_error(Some("a.rs"), Some("E1"), "error")],
            },
            ErrorGroup {
                check: "lint".to_string(),
                error_type: "E2".to_string(),
                files: vec!["b.rs".to_string(), "c.rs".to_string()],
                annotations: vec![make_error(Some("b.rs"), Some("E2"), "error")],
            },
        ];

        let locks = build_file_locks(&groups);
        assert_eq!(locks.len(), 3);
        assert!(locks.contains_key("a.rs"));
        assert!(locks.contains_key("b.rs"));
        assert!(locks.contains_key("c.rs"));
    }

    #[tokio::test]
    async fn acquire_file_locks_dedups_duplicate_files_in_batch() {
        let mut locks = HashMap::new();
        locks.insert("a.rs".to_string(), Arc::new(Semaphore::new(1)));

        let res = tokio::time::timeout(
            Duration::from_millis(100),
            acquire_file_locks(&locks, &["a.rs".to_string(), "a.rs".to_string()]),
        )
        .await;

        let permits = res
            .expect("acquire_file_locks timed out")
            .expect("expected Ok permits");
        assert_eq!(permits.len(), 1);
    }

    #[tokio::test]
    async fn acquire_file_locks_serializes_overlapping_calls() {
        let mut locks = HashMap::new();
        locks.insert("a.rs".to_string(), Arc::new(Semaphore::new(1)));
        let locks = Arc::new(locks);

        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let locks = locks.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            handles.push(tokio::spawn(async move {
                let _permits = acquire_file_locks(&locks, &["a.rs".to_string()])
                    .await
                    .expect("acquire_file_locks");
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }

        for handle in handles {
            handle.await.expect("task join");
        }

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fixer_batches_reports_all_failures() {
        let agent = sh_agent("cat >/dev/null; echo fixer failed >&2; exit 7");
        let pool = Pool::new(4);
        let root = TempDir::new("fixer-batches-errors");

        let groups = vec![
            ErrorGroup {
                check: "lint".to_string(),
                error_type: "E1".to_string(),
                files: vec!["a.rs".to_string()],
                annotations: vec![make_error(Some("a.rs"), Some("E1"), "error")],
            },
            ErrorGroup {
                check: "lint".to_string(),
                error_type: "E2".to_string(),
                files: vec!["b.rs".to_string()],
                annotations: vec![make_error(Some("b.rs"), Some("E2"), "error")],
            },
        ];

        let err = run_fixer_batches(&agent, "analysis", &groups, 1, &pool, root.path())
            .await
            .expect_err("expected run_fixer_batches to fail");
        let msg = err.to_string();
        assert!(msg.contains("one or more fixer batches failed"));
        assert!(msg.contains("1."));
        assert!(msg.contains("2."));
        assert!(msg.contains("fixer batch failed for lint:E1"));
        assert!(msg.contains("fixer batch failed for lint:E2"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fix_pipeline_propagates_analyzer_failures() {
        let analyzer = sh_agent("cat >/dev/null; echo analyzer failed >&2; exit 2");
        let fixer = sh_agent("cat >/dev/null; exit 0");
        let pool = Pool::new(2);
        let root = TempDir::new("fix-pipeline-analyzer-fail");

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

        let err = run_fix_pipeline(
            &analyzer,
            &fixer,
            &errors_by_check,
            1,
            &pool,
            root.path(),
            None,
        )
        .await
        .expect_err("expected run_fix_pipeline to fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("fix pipeline failed"));
        assert!(msg.contains("analyzer failed for lint"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fix_pipeline_propagates_fixer_failures() {
        let analyzer = sh_agent("cat >/dev/null; echo analysis");
        let fixer = sh_agent("cat >/dev/null; echo fixer failed >&2; exit 3");
        let pool = Pool::new(2);
        let root = TempDir::new("fix-pipeline-fixer-fail");

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

        let err = run_fix_pipeline(
            &analyzer,
            &fixer,
            &errors_by_check,
            1,
            &pool,
            root.path(),
            None,
        )
        .await
        .expect_err("expected run_fix_pipeline to fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("fix pipeline failed"));
        assert!(msg.contains("fixer failed for lint"));
    }
}
