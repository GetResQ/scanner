use tokio::sync::mpsc::Sender;

use crate::config::Check;
use crate::gha::{is_error_level, Annotation, AnnotationLevel};
use crate::pool::Pool;
use crate::ui::UiEvent;

mod execution;
mod process_runner;
mod selection;

pub use execution::CheckResult;

use crate::config::Config;

/// Synthesize a failing CheckResult for checks that failed to execute.
/// This ensures misconfigured checks (binary not found, spawn failure, etc.)
/// still appear as failures rather than being silently dropped.
fn synthesize_failed_result(check: Check, error: &str) -> CheckResult {
    CheckResult {
        check: check.clone(),
        exit_code: None, // None indicates execution failure (not exit code)
        raw_output: error.to_string(),
        annotations: vec![Annotation {
            level: AnnotationLevel::Error,
            file: None,
            line: None,
            end_line: None,
            column: None,
            end_column: None,
            title: Some("execution failed".to_string()),
            message: error.to_string(),
        }],
    }
}

pub async fn run_checks(
    config: &Config,
    filters: &[String],
    force: bool,
    pool: &Pool,
    quiet: bool,
    ui_events: Option<Sender<UiEvent>>,
    root: &std::path::Path,
) -> Vec<CheckResult> {
    let selected = selection::select_checks(config, filters, force);

    if selected.is_empty() {
        return Vec::new();
    }

    let mut handles = Vec::new();

    for check in selected {
        let check_clone = check.clone();
        let ui_tx = ui_events.clone();
        let root = root.to_path_buf();
        let quiet = quiet;

        // Spawn through the pool - waits for a slot if pool is full
        let handle = pool.spawn(async move {
            if let Some(tx) = ui_tx.as_ref() {
                let _ = tx
                    .send(UiEvent::CheckStarted {
                        name: check_clone.name.clone(),
                        desc: check_clone.description.clone(),
                    })
                    .await;
            } else if !quiet {
                eprintln!("running check: {}", check_clone.name);
            }

            // Pass UI channel for streaming
            let result = execution::run_single_check(&check_clone, &root, ui_tx.clone()).await;

            // Convert errors to failing CheckResult so they're not lost
            let check_result = match result {
                Ok(res) => res,
                Err(err) => {
                    let error_msg = format!("{err:#}");
                    synthesize_failed_result(check_clone.clone(), &error_msg)
                }
            };

            if let Some(tx) = ui_tx.as_ref() {
                let success = check_result.exit_code == Some(0)
                    && !check_result.annotations.iter().any(|a| is_error_level(a.level));
                let msg = if success {
                    "ok".to_string()
                } else if check_result.exit_code.is_none() {
                    // Execution failure (not a normal exit)
                    "failed to run".to_string()
                } else {
                    format!("{} issues", check_result.annotations.len())
                };
                let _ = tx
                    .send(UiEvent::CheckFinished {
                        name: check_clone.name.clone(),
                        success,
                        message: msg,
                        output: Some(check_result.raw_output.clone()),
                    })
                    .await;
            }

            check_result
        });

        handles.push(handle);
    }

    // Collect results - all checks are included, even those that failed to execute
    let mut results = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(result) => results.push(result),
            Err(join_err) => {
                // Task panicked - this is a bug in scanner itself, not a check failure
                // Log it but we can't synthesize a result without the Check info
                if !quiet {
                    eprintln!("check task panic: {join_err:?}");
                }
            }
        }
    }

    results
}
