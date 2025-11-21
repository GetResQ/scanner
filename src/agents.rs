use std::time::Duration;

use anyhow::Result;
use which::which;

use crate::Cli;
use crate::config;
use crate::config::Agent;
use crate::config::CommandSpec;
use crate::error::AgentError;

pub fn resolve_agent(role: &str, cli: &Cli, cfg: &config::Config) -> Result<Agent> {
    // CLI overrides config; if CLI agent is set, synthesize it.
    if let Some(agent_name) = &cli.agent {
        return synthesize_agent(agent_name, cli.model.clone(), role);
    }
    // Otherwise pull from role-specific config.
    let agent_opt = match role {
        "analyzer" => cfg.agents.analyzer.as_ref(),
        "fixer" => cfg.agents.fixer.as_ref(),
        _ => None,
    };
    if let Some(agent) = agent_opt {
        return Ok(agent.clone());
    }
    Err(AgentError::NotConfigured {
        role: role.to_string(),
    }
    .into())
}

fn synthesize_agent(agent_name: &str, model_override: Option<String>, role: &str) -> Result<Agent> {
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
            let mut args = vec![
                "exec".to_string(),
                "--model".to_string(),
                model,
                "-c".to_string(),
                "model_reasoning_effort=\"medium\"".to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
                "-".to_string(),
            ];
            if role == "fixer" {
                // For fixing we need non-interactive tool execution.
                args.insert(1, "--dangerously-bypass-approvals-and-sandbox".to_string());
            }
            args
        }
        "claude" => {
            let mut args = vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "text".to_string(),
                "--input-format".to_string(),
                "text".to_string(),
                "--no-session-persistence".to_string(),
                "--model".to_string(),
                model,
            ];

            if role == "fixer" {
                args.push("--dangerously-skip-permissions".to_string());
                args.push("--tools".to_string());
                args.push("default".to_string());
            } else {
                // Analyzer should not modify the workspace.
                args.push("--tools".to_string());
                args.push("Read".to_string());
                // Avoid interactive permission prompts in non-interactive mode.
                args.push("--permission-mode".to_string());
                args.push("bypassPermissions".to_string());
            }

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
