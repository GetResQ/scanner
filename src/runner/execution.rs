use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::config::Check;
use crate::error::CheckError;
use crate::gha::{Annotation, AnnotationLevel, is_error_level, parse_annotations};
use crate::ui::UiEvent;

use super::process_runner::{run_formatter, run_process, run_process_streaming};

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub check: Check,
    pub exit_code: Option<i32>,
    pub raw_output: String,
    pub annotations: Vec<Annotation>,
}

pub(crate) async fn run_single_check(
    check: &Check,
    root: &Path,
    ui_tx: Option<Sender<UiEvent>>,
) -> Result<CheckResult> {
    let initial = run_check_once(check, root, ui_tx.clone()).await?;

    if initial.exit_code == Some(0) && !initial.annotations.iter().any(|a| is_error_level(a.level))
    {
        return Ok(initial);
    }

    if let Some(fixer_cmd) = &check.fixer {
        let _ = run_process(
            fixer_cmd,
            &check.env,
            check.timeout,
            root,
            check.cwd.as_ref(),
        )
        .await;
        let rerun = run_check_once(check, root, ui_tx).await?;
        return Ok(rerun);
    }

    Ok(initial)
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

fn resolve_workdir(root: &Path, maybe_cwd: Option<&String>) -> PathBuf {
    if let Some(cwd) = maybe_cwd {
        let path = Path::new(cwd);
        if path.is_absolute() {
            return path.to_path_buf();
        }
        if let Some(stripped) = path.to_str()
            && let Some(rest) = stripped.strip_prefix("~/")
            && let Some(home) = dirs::home_dir()
        {
            return home.join(rest);
        }
        root.join(path)
    } else {
        root.to_path_buf()
    }
}

fn normalize_annotation_paths(
    annotations: &mut [Annotation],
    root: &Path,
    check_cwd: Option<&String>,
) {
    let workdir = resolve_workdir(root, check_cwd);

    for ann in annotations.iter_mut() {
        let Some(file) = ann.file.as_ref() else {
            continue;
        };

        // Prefer resolving relative paths against the check's working directory. This makes
        // compiler outputs (e.g. `tsc` diagnostics) usable when checks run in subfolders.
        if !file.is_absolute() {
            let candidate = workdir.join(file);
            if candidate.exists() {
                if let Ok(rel) = candidate.strip_prefix(root) {
                    ann.file = Some(clean_path(rel));
                } else {
                    ann.file = Some(candidate);
                }
                continue;
            }

            ann.file = Some(clean_path(file));
            continue;
        }

        // For absolute paths, try to convert to root-relative to keep the UI stable.
        if let Ok(rel) = file.strip_prefix(root) {
            ann.file = Some(clean_path(rel));
        }
    }
}

async fn run_check_once(
    check: &Check,
    root: &Path,
    ui_tx: Option<Sender<UiEvent>>,
) -> Result<CheckResult> {
    let (exit_code, combined_output) = run_process_streaming(
        &check.command,
        &check.env,
        check.timeout,
        root,
        check.cwd.as_ref(),
        Some(check.name.clone()),
        ui_tx,
    )
    .await?;

    let (_formatted_output, mut annotations) = if let Some(formatter) = &check.formatter {
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
                return Err(CheckError::FormatterFailed {
                    exit_code: fmt_exit,
                    stderr: fmt_output.clone(),
                }
                .into());
            }
            let mut annotations = parse_annotations(&fmt_output);
            if annotations.is_empty() {
                // Formatter produced no annotations; fall back to parsing the raw output.
                annotations = parse_annotations(&combined_output);
            }
            if annotations.is_empty() {
                annotations.push(Annotation {
                    level: AnnotationLevel::Error,
                    actionable: false,
                    file: None,
                    line: None,
                    end_line: None,
                    column: None,
                    end_column: None,
                    title: Some("no annotations".to_string()),
                    message: format!(
                        "check exited with {exit_code:?} but produced no GitHub Actions annotations; configure a formatter or update the check output"
                    ),
                });
            }
            (fmt_output, annotations)
        }
    } else {
        let mut annotations = parse_annotations(&combined_output);
        if exit_code != Some(0) && annotations.is_empty() {
            annotations.push(Annotation {
                level: AnnotationLevel::Error,
                actionable: false,
                file: None,
                line: None,
                end_line: None,
                column: None,
                end_column: None,
                title: Some("no annotations".to_string()),
                message: format!(
                    "check exited with {exit_code:?} but produced no GitHub Actions annotations; configure a formatter or update the check output"
                ),
            });
        }
        (combined_output.clone(), annotations)
    };

    normalize_annotation_paths(&mut annotations, root, check.cwd.as_ref());

    Ok(CheckResult {
        check: check.clone(),
        exit_code,
        raw_output: combined_output,
        annotations,
    })
}
