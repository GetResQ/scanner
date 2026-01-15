use std::time::Duration;

use anyhow::Result;
use which::which;

use crate::Cli;
use crate::config;
use crate::config::{Agent, CommandSpec};
use crate::error::AgentError;

pub fn resolve_agent(cli: &Cli, cfg: &config::Config) -> Result<Agent> {
    // CLI overrides config; if CLI agent is set, synthesize it.
    if let Some(agent_name) = &cli.agent {
        return synthesize_agent(agent_name, cli.model.clone());
    }

    // Prefer the unified agent config if present, then fall back to legacy roles.
    if let Some(agent) = cfg.agent.as_ref() {
        return Ok(agent.clone());
    }
    if let Some(agent) = cfg.agents.fixer.as_ref() {
        return Ok(agent.clone());
    }
    if let Some(agent) = cfg.agents.analyzer.as_ref() {
        return Ok(agent.clone());
    }

    Err(AgentError::NotConfigured.into())
}

fn synthesize_agent(agent_name: &str, model_override: Option<String>) -> Result<Agent> {
    let kind = agent_name.to_ascii_lowercase();
    let (binary, default_model) = match kind.as_str() {
        "codex" => ("codex", "gpt-5.1-codex-max"),
        // Claude Code supports aliases like "opus" and "sonnet"; default to the requested Opus 4.5 model.
        "claude" => ("claude", "claude-opus-4-5-20251101"),
        _ => {
            return Err(AgentError::UnsupportedType(agent_name.to_string()).into());
        }
    };

    let model = model_override.unwrap_or_else(|| default_model.to_string());

    let path = which(binary).map_err(|_| AgentError::BinaryNotFound {
        binary: binary.to_string(),
    })?;

    let args = match kind.as_str() {
        "codex" => {
            let args = vec![
                "exec".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "--model".to_string(),
                model,
                "-c".to_string(),
                "model_reasoning_effort=\"medium\"".to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
                "-".to_string(),
            ];
            args
        }
        "claude" => {
            let args = vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "text".to_string(),
                "--input-format".to_string(),
                "text".to_string(),
                "--no-session-persistence".to_string(),
                "--model".to_string(),
                model,
                "--dangerously-skip-permissions".to_string(),
                "--tools".to_string(),
                "default".to_string(),
            ];
            args
        }
        _ => unreachable!("validated above"),
    };

    Ok(Agent {
        command: CommandSpec {
            program: path.display().to_string(),
            args,
        },
        env: std::collections::HashMap::new(),
        timeout: Some(Duration::from_secs(300)),
    })
}
