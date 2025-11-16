use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawCheck {
    name: String,
    command: Vec<String>,
    #[serde(default)]
    formatter: Option<Vec<String>>,
    #[serde(default)]
    fixer: Option<Vec<String>>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawAgent {
    pub command: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct RawAgents {
    #[serde(default)]
    pub analyzer: Option<RawAgent>,
    #[serde(default)]
    pub fixer: Option<RawAgent>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    checks: Vec<RawCheck>,
    #[serde(default)]
    agents: RawAgents,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub command: CommandSpec,
    pub formatter: Option<CommandSpec>,
    pub fixer: Option<CommandSpec>,
    pub env: HashMap<String, String>,
    pub timeout: Option<Duration>,
    pub enabled: bool,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub command: CommandSpec,
    pub env: HashMap<String, String>,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Default)]
pub struct Agents {
    pub analyzer: Option<Agent>,
    pub fixer: Option<Agent>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub checks: Vec<Check>,
    pub agents: Agents,
}

impl Config {
    pub fn from_toml(input: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(input)?;
        let mut checks = Vec::new();

        for raw_check in raw.checks {
            if raw_check.command.is_empty() {
                bail!("check '{}' must define a non-empty command", raw_check.name);
            }

            let command = CommandSpec {
                program: raw_check.command[0].clone(),
                args: raw_check.command[1..].to_vec(),
            };

            let formatter = match raw_check.formatter {
                Some(cmd) => {
                    if cmd.is_empty() {
                        bail!(
                            "formatter for '{}' must define a non-empty command",
                            raw_check.name
                        );
                    }
                    Some(CommandSpec {
                        program: cmd[0].clone(),
                        args: cmd[1..].to_vec(),
                    })
                }
                None => None,
            };

            let fixer = match raw_check.fixer {
                Some(cmd) => {
                    if cmd.is_empty() {
                        bail!(
                            "fixer for '{}' must define a non-empty command",
                            raw_check.name
                        );
                    }
                    Some(CommandSpec {
                        program: cmd[0].clone(),
                        args: cmd[1..].to_vec(),
                    })
                }
                None => None,
            };

            let timeout = raw_check.timeout.map(Duration::from_secs);

            let enabled = raw_check.enabled.unwrap_or(true);

            checks.push(Check {
                name: raw_check.name,
                command,
                formatter,
                fixer,
                env: raw_check.env,
                timeout,
                enabled,
                tags: raw_check.tags,
                description: raw_check.description,
                cwd: raw_check.cwd,
            });
        }

        let agents = Agents {
            analyzer: raw
                .agents
                .analyzer
                .map(|agent| Self::convert_agent("analyzer", agent))
                .transpose()?,
            fixer: raw
                .agents
                .fixer
                .map(|agent| Self::convert_agent("fixer", agent))
                .transpose()?,
        };

        Ok(Config { checks, agents })
    }
}

impl Config {
    fn convert_agent(role: &str, raw: RawAgent) -> Result<Agent> {
        if raw.command.is_empty() {
            bail!("{role} agent must define a non-empty command");
        }
        Ok(Agent {
            command: CommandSpec {
                program: raw.command[0].clone(),
                args: raw.command[1..].to_vec(),
            },
            env: raw.env,
            timeout: raw.timeout.map(Duration::from_secs),
        })
    }
}
