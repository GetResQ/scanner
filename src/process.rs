use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::time;

use crate::config::CommandSpec;
use crate::error::ProcessError;
use crate::ui::{StreamType, UiEvent, sanitize_text_for_tui};

/// Run a command with optional stdin, collecting stdout/stderr, honoring timeout, and killing on timeout.
pub async fn run_command(
    spec: &CommandSpec,
    env: &HashMap<String, String>,
    root: &Path,
    timeout: Option<Duration>,
    stdin: Option<Vec<u8>>,
) -> Result<(Option<i32>, Vec<u8>, Vec<u8>)> {
    run_command_streaming(spec, env, root, timeout, stdin, None, None).await
}

/// Run a command with optional streaming of output lines.
pub async fn run_command_streaming(
    spec: &CommandSpec,
    env: &HashMap<String, String>,
    root: &Path,
    timeout: Option<Duration>,
    stdin: Option<Vec<u8>>,
    source_name: Option<String>,
    ui_tx: Option<Sender<UiEvent>>,
) -> Result<(Option<i32>, Vec<u8>, Vec<u8>)> {
    let wants_stdin = stdin.is_some();
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .envs(env)
        .current_dir(root)
        .stdin(if wants_stdin {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| ProcessError::SpawnFailed(e.to_string()))?;

    if let Some(input) = stdin
        && let Some(mut child_stdin) = child.stdin.take()
    {
        child_stdin
            .write_all(&input)
            .await
            .map_err(|e| ProcessError::StdinWriteFailed(e.to_string()))?;
        child_stdin
            .shutdown()
            .await
            .map_err(|e| ProcessError::StdinWriteFailed(e.to_string()))?;
    }

    // Stream stdout
    let stdout_handle = {
        let stdout = child.stdout.take();
        let ui_tx = ui_tx.clone();
        let source = source_name.clone();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(out) = stdout {
                if let (Some(tx), Some(src)) = (ui_tx, source) {
                    // Stream lines as they come
                    let mut reader = BufReader::new(out);
                    let mut line = String::new();
                    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                        let trimmed = sanitize_text_for_tui(line.trim_end());
                        buf.extend_from_slice(line.as_bytes());
                        let _ = tx
                            .send(UiEvent::StreamLine {
                                source: src.clone(),
                                stream: StreamType::Stdout,
                                line: trimmed,
                            })
                            .await;
                        line.clear();
                    }
                } else {
                    // No streaming, just collect
                    let mut out = out;
                    let _ = out.read_to_end(&mut buf).await;
                }
            }
            buf
        })
    };

    // Stream stderr
    let stderr_handle = {
        let stderr = child.stderr.take();
        let ui_tx = ui_tx;
        let source = source_name;
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(err) = stderr {
                if let (Some(tx), Some(src)) = (ui_tx, source) {
                    // Stream lines as they come
                    let mut reader = BufReader::new(err);
                    let mut line = String::new();
                    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                        let trimmed = sanitize_text_for_tui(line.trim_end());
                        buf.extend_from_slice(line.as_bytes());
                        let _ = tx
                            .send(UiEvent::StreamLine {
                                source: src.clone(),
                                stream: StreamType::Stderr,
                                line: trimmed,
                            })
                            .await;
                        line.clear();
                    }
                } else {
                    // No streaming, just collect
                    let mut err = err;
                    let _ = err.read_to_end(&mut buf).await;
                }
            }
            buf
        })
    };

    let status = if let Some(dur) = timeout {
        match time::timeout(dur, child.wait()).await {
            Ok(res) => res.map_err(|e| ProcessError::OutputReadFailed(e.to_string()))?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(ProcessError::Timeout(dur).into());
            }
        }
    } else {
        child
            .wait()
            .await
            .map_err(|e| ProcessError::OutputReadFailed(e.to_string()))?
    };

    let stdout = stdout_handle.await.unwrap_or_default();
    let stderr = stderr_handle.await.unwrap_or_default();

    Ok((status.code(), stdout, stderr))
}
