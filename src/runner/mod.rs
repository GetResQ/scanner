use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::sync::mpsc::Sender;

use crate::config::{Check, Config, Setup};
use crate::gha::{Annotation, AnnotationLevel, is_error_level};
use crate::pool::Pool;
use crate::ui::{UiEvent, sanitize_text_for_tui};

mod execution;
mod process_runner;
mod selection;

pub use execution::CheckResult;

/// Run a setup command. Returns the exit code.
pub async fn run_setup(
    setup: &Setup,
    root: &std::path::Path,
    ui_tx: Option<Sender<UiEvent>>,
) -> Option<i32> {
    let result = process_runner::run_process_streaming(
        &setup.command,
        &setup.env,
        setup.timeout,
        root,
        setup.cwd.as_ref(),
        Some(format!("setup:{}", setup.name)),
        ui_tx,
    )
    .await;

    match result {
        Ok((exit_code, _output)) => exit_code,
        Err(_) => None,
    }
}

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
            actionable: false,
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

    // Optional per-check lock groups to serialize contended tools/resources.
    let mut lock_groups: HashMap<String, Arc<Semaphore>> = HashMap::new();
    for check in &selected {
        if let Some(lock) = check.lock.as_ref() {
            lock_groups
                .entry(lock.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(1)));
        }
    }
    let lock_groups = Arc::new(lock_groups);

    let mut handles = Vec::new();

    for check in selected {
        let check_clone = check.clone();
        let check_for_join = check.clone();
        let ui_tx = ui_events.clone();
        let root = root.to_path_buf();
        let lock_groups = lock_groups.clone();

        // Spawn through the pool - waits for a slot if pool is full
        let handle = pool.spawn(async move {
            let _lock_permit = match check_clone.lock.as_deref() {
                Some(lock) => lock_groups
                    .get(lock)
                    .expect("lock group present")
                    .clone()
                    .acquire_owned()
                    .await
                    .ok(),
                None => None,
            };

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
                    // Stream the error so it shows in verbose mode
                    if let Some(tx) = ui_tx.as_ref() {
                        let _ = tx
                            .send(UiEvent::StreamLine {
                                source: check_clone.name.clone(),
                                stream: crate::ui::StreamType::Stderr,
                                line: error_msg.clone(),
                            })
                            .await;
                    }
                    synthesize_failed_result(check_clone.clone(), &error_msg)
                }
            };

            if let Some(tx) = ui_tx.as_ref() {
                let success = check_result.exit_code == Some(0)
                    && !check_result
                        .annotations
                        .iter()
                        .any(|a| is_error_level(a.level));
                let msg = if success {
                    "ok".to_string()
                } else if check_result.exit_code.is_none() {
                    // Execution failure (not a normal exit)
                    "failed to run".to_string()
                } else {
                    format!("{} issues", check_result.annotations.len())
                };
                let output = Some(sanitize_text_for_tui(&check_result.raw_output));
                let _ = tx
                    .send(UiEvent::CheckFinished {
                        name: check_clone.name.clone(),
                        success,
                        message: msg,
                        output,
                    })
                    .await;
            }

            check_result
        });

        handles.push((check_for_join, handle));
    }

    // Collect results - all checks are included, even those that failed to execute
    let mut results = Vec::new();
    for (check, handle) in handles {
        match handle.await {
            Ok(result) => results.push(result),
            Err(join_err) => {
                // Task panicked - this is a bug in scanner itself, not a check failure
                let msg = format!("task panic: {join_err:?}");
                if let Some(tx) = ui_events.as_ref() {
                    let _ = tx
                        .send(UiEvent::CheckFinished {
                            name: check.name.clone(),
                            success: false,
                            message: "panic".to_string(),
                            output: Some(msg.clone()),
                        })
                        .await;
                } else if !quiet {
                    eprintln!("check task panic for {}: {join_err:?}", check.name);
                }
                results.push(synthesize_failed_result(check, &msg));
            }
        }
    }

    results
}
