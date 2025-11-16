use std::time::Duration;

use anyhow::{Context, Result};
use which::which;

use crate::Cli;
use crate::config;
use crate::config::Agent;
use crate::config::CommandSpec;

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
    anyhow::bail!("no {role} agent configured; specify --agent or agents.{role} in config");
}

fn synthesize_agent(agent_name: &str, model_override: Option<String>, role: &str) -> Result<Agent> {
    let kind = agent_name.to_ascii_lowercase();
    let (binary, default_model) = match kind.as_str() {
        "codex" => ("codex", "gpt-5.1-codex-mini"),
        "claude" => ("claude", "sonnet"),
        _ => anyhow::bail!("unsupported agent: {agent_name}"),
    };

    let model = model_override.unwrap_or_else(|| default_model.to_string());

    let path = which(binary).with_context(|| format!("{binary} executable not found in PATH"))?;

    let mut args = vec![
        "exec".to_string(),
        "--model".to_string(),
        model,
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "-".to_string(),
    ];
    if role == "fixer" {
        match kind.as_str() {
            "codex" => args.insert(3, "--dangerously-bypass-approvals-and-sandbox".to_string()),
            "claude" => args.insert(3, "--dangerously-skip-permissions".to_string()),
            _ => {}
        }
    }

    Ok(Agent {
        command: CommandSpec {
            program: path.display().to_string(),
            args,
        },
        env: std::collections::HashMap::new(),
        timeout: Some(Duration::from_secs(300)),
    })
}
