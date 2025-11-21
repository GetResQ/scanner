use std::path::Path;

use anyhow::{Result, anyhow};

use crate::config::Check;
use crate::gha::{Annotation, is_error_level, parse_annotations};

use super::process_runner::{run_formatter, run_process};

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub check: Check,
    pub exit_code: Option<i32>,
    pub raw_output: String,
    pub annotations: Vec<Annotation>,
}

pub(crate) async fn run_single_check(check: &Check, root: &Path) -> Result<CheckResult> {
    let initial = run_check_once(check, root).await?;

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
        let rerun = run_check_once(check, root).await?;
        return Ok(rerun);
    }

    Ok(initial)
}

async fn run_check_once(check: &Check, root: &Path) -> Result<CheckResult> {
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
