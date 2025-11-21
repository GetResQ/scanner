use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::{Semaphore, mpsc::Sender};

use crate::config::Config;
use crate::gha::is_error_level;
use crate::ui::UiEvent;

mod execution;
mod process_runner;
mod selection;

pub use execution::CheckResult;

pub async fn run_checks(
    config: &Config,
    filters: &[String],
    force: bool,
    workers: usize,
    quiet: bool,
    ui_events: Option<Sender<UiEvent>>,
    root: &std::path::Path,
) -> Result<Vec<CheckResult>> {
    let selected = selection::select_checks(config, filters, force);

    if selected.is_empty() {
        return Err(anyhow!("no checks matched the requested filters"));
    }

    let semaphore = Arc::new(Semaphore::new(workers.max(1)));
    let mut handles = Vec::new();

    for check in selected {
        let semaphore = semaphore.clone();
        let permit = semaphore.acquire_owned().await.unwrap();
        let check_clone = check.clone();
        let ui_tx = ui_events.clone();
        let root = root.to_path_buf();

        let handle = tokio::spawn(async move {
            let _permit = permit;
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

            let result = execution::run_single_check(&check_clone, &root).await;

            if let Some(tx) = ui_tx.as_ref() {
                match &result {
                    Ok(res) => {
                        let success = res.exit_code == Some(0)
                            && !res.annotations.iter().any(|a| is_error_level(a.level));
                        let msg = if success {
                            "ok".to_string()
                        } else {
                            format!("{} issues", res.annotations.len())
                        };
                        let _ = tx
                            .send(UiEvent::CheckFinished {
                                name: check_clone.name.clone(),
                                success,
                                message: msg,
                                output: Some(res.raw_output.clone()),
                            })
                            .await;
                    }
                    Err(err) => {
                        let _ = tx
                            .send(UiEvent::CheckFinished {
                                name: check_clone.name.clone(),
                                success: false,
                                message: format!("{err:#}"),
                                output: Some(format!("{err:#}")),
                            })
                            .await;
                    }
                }
            }

            result
        });

        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => results.push(result),
            Ok(Err(err)) => return Err(err),
            Err(join_err) => return Err(anyhow!("join error: {join_err:?}")),
        }
    }

    Ok(results)
}
