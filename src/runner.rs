use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::{Semaphore, mpsc::Sender};

use crate::config::{Check, CommandSpec, Config};
use crate::gha::{Annotation, is_error_level, parse_annotations};
use crate::process;
use crate::ui::UiEvent;

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub check: Check,
    pub exit_code: Option<i32>,
    pub raw_output: String,
    pub annotations: Vec<Annotation>,
}

pub async fn run_checks(
    config: &Config,
    filters: &[String],
    force: bool,
    workers: usize,
    quiet: bool,
    ui_events: Option<Sender<UiEvent>>,
    root: &std::path::Path,
) -> Result<Vec<CheckResult>> {
    let selected = select_checks(config, filters, force);

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
            let result = run_single_check(&check_clone, &root).await;
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
            Err(join_err) => return Err(anyhow::anyhow!("join error: {join_err:?}")),
        }
    }

    Ok(results)
}

fn select_checks<'a>(config: &'a Config, filters: &[String], force: bool) -> Vec<Check> {
    if filters.is_empty() {
        return config
            .checks
            .iter()
            .filter(|c| c.enabled)
            .cloned()
            .collect();
    }

    let filter_set: HashSet<String> = filters.iter().map(|s| s.to_ascii_lowercase()).collect();

    config
        .checks
        .iter()
        .filter(|check| {
            let name_match = filter_set.contains(&check.name.to_ascii_lowercase());
            let tag_match = check
                .tags
                .iter()
                .any(|t| filter_set.contains(&t.to_ascii_lowercase()));

            // Force only applies to explicit name matches; tag matches still honor enabled.
            (name_match && (check.enabled || force)) || (tag_match && check.enabled)
        })
        .cloned()
        .collect()
}

async fn run_single_check(check: &Check, root: &std::path::Path) -> Result<CheckResult> {
    let initial = run_check_once(check, root).await?;

    // If passed, return
    if initial.exit_code == Some(0) || !initial.annotations.iter().any(|a| is_error_level(a.level))
    {
        return Ok(initial);
    }

    // If fixer command present, run it, then re-run the check once
    if let Some(fixer_cmd) = &check.fixer {
        let _ = run_process(
            fixer_cmd,
            &check.env,
            check.timeout,
            root,
            check.cwd.as_ref(),
        )
        .await;
        let rerun = run_check_once(check, root).await?;
        return Ok(rerun);
    }

    Ok(initial)
}

async fn run_check_once(check: &Check, root: &std::path::Path) -> Result<CheckResult> {
    let (exit_code, combined_output) = run_process(
        &check.command,
        &check.env,
        check.timeout,
        root,
        check.cwd.as_ref(),
    )
    .await?;

    let (_formatted_output, annotations) = if let Some(formatter) = &check.formatter {
        if exit_code == Some(0) {
            (String::new(), Vec::new())
        } else {
            let (fmt_exit, fmt_output) = run_formatter(
                formatter,
                &check.env,
                &combined_output,
                check.timeout,
                root,
                check.cwd.as_ref(),
            )
            .await?;
            if fmt_exit != Some(0) {
                eprintln!("formatter for '{}' exited with {:?}", check.name, fmt_exit);
            }
            let annotations = parse_annotations(&fmt_output);
            (fmt_output, annotations)
        }
    } else {
        let annotations = parse_annotations(&combined_output);
        if exit_code != Some(0) && annotations.is_empty() {
            return Err(anyhow!(
                "check '{}' failed but did not emit any GitHub Actions annotations",
                check.name
            ));
        }
        (combined_output.clone(), annotations)
    };

    Ok(CheckResult {
        check: check.clone(),
        exit_code,
        raw_output: combined_output,
        annotations,
    })
}

async fn run_process(
    spec: &CommandSpec,
    env: &HashMap<String, String>,
    timeout: Option<std::time::Duration>,
    root: &std::path::Path,
    cwd: Option<&String>,
) -> Result<(Option<i32>, String)> {
    let workdir = resolve_workdir(root, cwd);
    let (status, stdout_buf, stderr_buf) =
        process::run_command(spec, env, &workdir, timeout, None).await?;

    let mut text = String::new();
    if !stdout_buf.is_empty() {
        text.push_str(&String::from_utf8_lossy(&stdout_buf));
    }
    if !stderr_buf.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&stderr_buf));
    }

    Ok((status, text))
}

async fn run_formatter(
    spec: &CommandSpec,
    env: &HashMap<String, String>,
    input: &str,
    timeout: Option<std::time::Duration>,
    root: &std::path::Path,
    cwd: Option<&String>,
) -> Result<(Option<i32>, String)> {
    let workdir = resolve_workdir(root, cwd);
    let (status, stdout_buf, stderr_buf) = process::run_command(
        spec,
        env,
        &workdir,
        timeout,
        Some(input.as_bytes().to_vec()),
    )
    .await?;

    let mut text = String::new();
    if !stdout_buf.is_empty() {
        text.push_str(&String::from_utf8_lossy(&stdout_buf));
    }
    if !stderr_buf.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&stderr_buf));
    }

    Ok((status, text))
}

fn resolve_workdir(root: &std::path::Path, maybe_cwd: Option<&String>) -> std::path::PathBuf {
    if let Some(cwd) = maybe_cwd {
        let path = std::path::Path::new(cwd);
        if path.is_absolute() {
            return path.to_path_buf();
        }
        if let Some(stripped) = path.to_str() {
            if stripped.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    return home.join(&stripped[2..]);
                }
            }
        }
        root.join(path)
    } else {
        root.to_path_buf()
    }
}
