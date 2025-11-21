use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::CommandSpec;
use crate::process;

pub(crate) async fn run_process(
    spec: &CommandSpec,
    env: &HashMap<String, String>,
    timeout: Option<std::time::Duration>,
    root: &Path,
    cwd: Option<&String>,
) -> Result<(Option<i32>, String)> {
    let workdir = resolve_workdir(root, cwd);
    let (status, stdout_buf, stderr_buf) =
        process::run_command(spec, env, &workdir, timeout, None).await?;

    Ok(combine_output(status, stdout_buf, stderr_buf))
}

pub(crate) async fn run_formatter(
    spec: &CommandSpec,
    env: &HashMap<String, String>,
    input: &str,
    timeout: Option<std::time::Duration>,
    root: &Path,
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

    Ok(combine_output(status, stdout_buf, stderr_buf))
}

fn combine_output(
    status: Option<i32>,
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
) -> (Option<i32>, String) {
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

    (status, text)
}

fn resolve_workdir(root: &Path, maybe_cwd: Option<&String>) -> PathBuf {
    if let Some(cwd) = maybe_cwd {
        let path = Path::new(cwd);
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
