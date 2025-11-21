use std::path::Path;

use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::config::Check;
use crate::error::CheckError;
use crate::gha::{Annotation, is_error_level, parse_annotations};
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
                return Err(CheckError::FormatterFailed {
                    exit_code: fmt_exit,
                    stderr: fmt_output.clone(),
                }
                .into());
            }
            let annotations = parse_annotations(&fmt_output);
            (fmt_output, annotations)
        }
    } else {
        let annotations = parse_annotations(&combined_output);
        if exit_code != Some(0) && annotations.is_empty() {
            return Err(CheckError::NoAnnotations { exit_code }.into());
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
