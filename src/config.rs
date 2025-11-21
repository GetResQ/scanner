use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::error::ConfigError;

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
    #[serde(default)]
    lock: Option<String>,
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
    /// Optional lock group name to serialize checks that contend for a shared resource.
    pub lock: Option<String>,
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
                return Err(ConfigError::EmptyCommand {
                    name: raw_check.name,
                }
                .into());
            }

            let command = CommandSpec {
                program: raw_check.command[0].clone(),
                args: raw_check.command[1..].to_vec(),
            };

            let formatter = match raw_check.formatter {
                Some(cmd) => {
                    if cmd.is_empty() {
                        return Err(ConfigError::EmptyFormatter {
                            name: raw_check.name,
                        }
                        .into());
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
                        return Err(ConfigError::EmptyFixer {
                            name: raw_check.name,
                        }
                        .into());
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
                lock: raw_check.lock,
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
            return Err(ConfigError::EmptyAgentCommand {
                role: role.to_string(),
            }
            .into());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
[[checks]]
name = "lint"
command = ["cargo", "clippy"]
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.checks.len(), 1);
        assert_eq!(config.checks[0].name, "lint");
        assert_eq!(config.checks[0].command.program, "cargo");
        assert_eq!(config.checks[0].command.args, vec!["clippy"]);
        assert!(config.checks[0].enabled);
    }

    #[test]
    fn parse_check_with_all_fields() {
        let toml = r#"
[[checks]]
name = "test"
command = ["cargo", "test", "--all"]
formatter = ["format-output"]
fixer = ["cargo", "fix"]
timeout = 300
enabled = false
tags = ["rust", "unit"]
description = "Run unit tests"
cwd = "./backend"
lock = "backend"

[checks.env]
RUST_BACKTRACE = "1"
"#;
        let config = Config::from_toml(toml).unwrap();
        let check = &config.checks[0];

        assert_eq!(check.name, "test");
        assert_eq!(check.command.program, "cargo");
        assert_eq!(check.command.args, vec!["test", "--all"]);
        assert_eq!(check.formatter.as_ref().unwrap().program, "format-output");
        assert_eq!(check.fixer.as_ref().unwrap().program, "cargo");
        assert_eq!(check.timeout, Some(Duration::from_secs(300)));
        assert!(!check.enabled);
        assert_eq!(check.tags, vec!["rust", "unit"]);
        assert_eq!(check.description, Some("Run unit tests".to_string()));
        assert_eq!(check.cwd, Some("./backend".to_string()));
        assert_eq!(check.lock, Some("backend".to_string()));
        assert_eq!(check.env.get("RUST_BACKTRACE"), Some(&"1".to_string()));
    }

    #[test]
    fn parse_multiple_checks() {
        let toml = r#"
[[checks]]
name = "lint"
command = ["cargo", "clippy"]

[[checks]]
name = "test"
command = ["cargo", "test"]

[[checks]]
name = "fmt"
command = ["cargo", "fmt", "--check"]
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.checks.len(), 3);
        assert_eq!(config.checks[0].name, "lint");
        assert_eq!(config.checks[1].name, "test");
        assert_eq!(config.checks[2].name, "fmt");
    }

    #[test]
    fn parse_config_with_agents() {
        let toml = r#"
[[checks]]
name = "test"
command = ["cargo", "test"]

[agents.analyzer]
command = ["codex", "exec", "--json"]
timeout = 600

[agents.fixer]
command = ["codex", "exec", "--json", "--apply"]
"#;
        let config = Config::from_toml(toml).unwrap();

        let analyzer = config.agents.analyzer.as_ref().unwrap();
        assert_eq!(analyzer.command.program, "codex");
        assert_eq!(analyzer.timeout, Some(Duration::from_secs(600)));

        let fixer = config.agents.fixer.as_ref().unwrap();
        assert_eq!(fixer.command.program, "codex");
        assert!(fixer.timeout.is_none());
    }

    #[test]
    fn empty_command_fails() {
        let toml = r#"
[[checks]]
name = "bad"
command = []
"#;
        let result = Config::from_toml(toml);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("non-empty command")
        );
    }

    #[test]
    fn empty_formatter_command_fails() {
        let toml = r#"
[[checks]]
name = "bad"
command = ["cargo", "test"]
formatter = []
"#;
        let result = Config::from_toml(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("formatter"));
    }

    #[test]
    fn empty_fixer_command_fails() {
        let toml = r#"
[[checks]]
name = "bad"
command = ["cargo", "test"]
fixer = []
"#;
        let result = Config::from_toml(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("fixer"));
    }

    #[test]
    fn empty_agent_command_fails() {
        let toml = r#"
[[checks]]
name = "test"
command = ["cargo", "test"]

[agents.analyzer]
command = []
"#;
        let result = Config::from_toml(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("analyzer"));
    }

    #[test]
    fn default_enabled_is_true() {
        let toml = r#"
[[checks]]
name = "test"
command = ["cargo", "test"]
"#;
        let config = Config::from_toml(toml).unwrap();
        assert!(config.checks[0].enabled);
    }

    #[test]
    fn empty_config_is_valid() {
        let toml = "";
        let config = Config::from_toml(toml).unwrap();
        assert!(config.checks.is_empty());
        assert!(config.agents.analyzer.is_none());
        assert!(config.agents.fixer.is_none());
    }

    #[test]
    fn invalid_toml_fails() {
        let toml = "this is not valid toml {{{{";
        let result = Config::from_toml(toml);
        assert!(result.is_err());
    }
}
