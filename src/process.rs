use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time;

use crate::config::CommandSpec;

/// Run a command with optional stdin, collecting stdout/stderr, honoring timeout, and killing on timeout.
pub async fn run_command(
    spec: &CommandSpec,
    env: &HashMap<String, String>,
    root: &Path,
    timeout: Option<Duration>,
    stdin: Option<Vec<u8>>,
) -> Result<(Option<i32>, Vec<u8>, Vec<u8>)> {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .envs(env)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    if let Some(input) = stdin {
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(&input).await?;
        }
    }

    let stdout_handle = {
        let mut stdout = child.stdout.take();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut out) = stdout.take() {
                let _ = out.read_to_end(&mut buf).await;
            }
            buf
        })
    };

    let stderr_handle = {
        let mut stderr = child.stderr.take();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut err) = stderr.take() {
                let _ = err.read_to_end(&mut buf).await;
            }
            buf
        })
    };

    let status = if let Some(dur) = timeout {
        match time::timeout(dur, child.wait()).await {
            Ok(res) => res?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(anyhow!("command timed out"));
            }
        }
    } else {
        child.wait().await?
    };

    let stdout = stdout_handle.await.unwrap_or_default();
    let stderr = stderr_handle.await.unwrap_or_default();

    Ok((status.code(), stdout, stderr))
}
