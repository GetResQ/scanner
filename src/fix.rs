use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json;
use tokio::sync::Semaphore;

use crate::config::Agent;
use crate::gha::{Annotation, AnnotationLevel, is_error_level};
use crate::process;
use crate::runner::CheckResult;

#[derive(Debug, Serialize)]
struct SerializableAnnotation<'a> {
    level: &'a str,
    file: Option<String>,
    line: Option<u64>,
    end_line: Option<u64>,
    column: Option<u64>,
    end_column: Option<u64>,
    title: Option<&'a str>,
    message: &'a str,
}

#[derive(Debug, Serialize)]
struct AnalyzerInput<'a> {
    groups: Vec<AnalyzerGroup<'a>>,
}

#[derive(Debug, Serialize)]
struct AnalyzerGroup<'a> {
    check: &'a str,
    error_type: String,
    files: Vec<String>,
    annotations: Vec<SerializableAnnotation<'a>>,
}

#[derive(Debug, Serialize)]
struct FixerInput<'a> {
    check: &'a str,
    error_type: &'a str,
    analysis: &'a str,
    files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ErrorGroup {
    pub check: String,
    pub error_type: String,
    pub files: Vec<String>,
    pub annotations: Vec<Annotation>,
}

pub fn group_errors(results: &[CheckResult]) -> Vec<ErrorGroup> {
    let mut grouped: HashMap<(String, String), (HashSet<String>, Vec<Annotation>)> = HashMap::new();

    for result in results {
        let treat_non_errors = result.exit_code != Some(0);
        for ann in &result.annotations {
            if !is_error_level(ann.level) && !treat_non_errors {
                continue;
            }
            let key = error_key(ann);
            let entry = grouped
                .entry((result.check.name.clone(), key))
                .or_insert_with(|| (HashSet::new(), Vec::new()));
            if let Some(path) = ann.file.as_ref() {
                entry.0.insert(path.display().to_string());
            }
            entry.1.push(ann.clone());
        }

        // If the process failed but no annotations, treat the whole check as one error group.
        if result.exit_code != Some(0) && result.annotations.is_empty() {
            let entry = grouped
                .entry((result.check.name.clone(), "process-failed".to_string()))
                .or_insert_with(|| (HashSet::new(), Vec::new()));
            entry.1.push(Annotation {
                level: AnnotationLevel::Error,
                file: None,
                line: None,
                end_line: None,
                column: None,
                end_column: None,
                title: Some("process failed".to_string()),
                message: result
                    .raw_output
                    .lines()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
        }
    }

    let mut out = Vec::new();
    for ((check, error_type), (files, anns)) in grouped {
        out.push(ErrorGroup {
            check,
            error_type,
            files: files.into_iter().collect(),
            annotations: anns,
        });
    }
    out
}

fn error_key(ann: &Annotation) -> String {
    ann.title
        .as_ref()
        .cloned()
        .unwrap_or_else(|| ann.message.clone())
}

pub async fn run_analyzer(
    agent: &Agent,
    groups: &[ErrorGroup],
    root: &std::path::Path,
) -> Result<String> {
    let input = AnalyzerInput {
        groups: groups
            .iter()
            .map(|g| AnalyzerGroup {
                check: &g.check,
                error_type: g.error_type.clone(),
                files: g.files.clone(),
                annotations: g
                    .annotations
                    .iter()
                    .map(|ann| SerializableAnnotation {
                        level: match ann.level {
                            AnnotationLevel::Error => "error",
                            AnnotationLevel::Warning => "warning",
                            AnnotationLevel::Notice => "notice",
                        },
                        file: ann.file.as_ref().map(|p| p.display().to_string()),
                        line: ann.line,
                        end_line: ann.end_line,
                        column: ann.column,
                        end_column: ann.end_column,
                        title: ann.title.as_deref(),
                        message: &ann.message,
                    })
                    .collect(),
            })
            .collect(),
    };

    let json = serde_json::to_vec(&input)?;
    run_agent_command(agent, &json, root).await
}

pub async fn run_fixer_batches(
    agent: &Agent,
    analysis_text: &str,
    groups: &[ErrorGroup],
    batch_size: usize,
    workers: usize,
    root: &std::path::Path,
) -> Result<()> {
    if batch_size == 0 {
        return Err(anyhow!("batch size must be > 0"));
    }

    let semaphore = Arc::new(Semaphore::new(workers.max(1)));
    let mut tasks = Vec::new();

    for group in groups {
        let batches = group.files.chunks(batch_size);
        for batch in batches {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let agent = agent.clone();
            let analysis = analysis_text.to_string();
            let check = group.check.to_string();
            let error_type = group.error_type.clone();
            let files: Vec<String> = batch.to_vec();
            let root = root.to_path_buf();

            let handle = tokio::spawn(async move {
                let _permit = permit;
                let input = FixerInput {
                    check: &check,
                    error_type: &error_type,
                    analysis: &analysis,
                    files,
                };
                let payload = serde_json::to_vec(&input)?;
                run_agent_command(&agent, &payload, &root).await?;
                Ok::<(), anyhow::Error>(())
            });

            tasks.push(handle);
        }
    }

    for handle in tasks {
        handle.await??;
    }

    Ok(())
}

async fn run_agent_command(
    agent: &Agent,
    payload: &[u8],
    root: &std::path::Path,
) -> Result<String> {
    let (code, stdout_buf, stderr_buf) = process::run_command(
        &agent.command,
        &agent.env,
        root,
        agent.timeout,
        Some(payload.to_vec()),
    )
    .await?;

    if code != Some(0) {
        return Err(anyhow!(
            "agent exited with {:?}: {}",
            code,
            String::from_utf8_lossy(&stderr_buf)
        ));
    }

    let mut text = String::from_utf8_lossy(&stdout_buf).to_string();
    if text.is_empty() && !stderr_buf.is_empty() {
        text = String::from_utf8_lossy(&stderr_buf).to_string();
    }
    Ok(text)
}
