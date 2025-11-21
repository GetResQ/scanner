//! Custom error types for the scanner.
//!
//! This module provides structured error types that enable better error handling,
//! pattern matching, and user-facing error messages.

use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

// Duration is used by ProcessError::Timeout

/// Errors that can occur during check execution.
#[derive(Debug, Error)]
pub enum CheckError {
    /// The formatter command failed.
    #[error("formatter failed with exit code {exit_code:?}: {stderr}")]
    FormatterFailed {
        exit_code: Option<i32>,
        stderr: String,
    },
}

/// Errors that can occur during the fix pipeline.
#[derive(Debug, Error)]
pub enum FixError {
    /// Invalid batch size configuration.
    #[error("batch size must be greater than 0")]
    InvalidBatchSize,
}

/// Errors that can occur during configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to read the configuration file.
    #[error("failed to read config at {path}: {reason}")]
    ReadFailed { path: PathBuf, reason: String },

    /// Failed to parse the configuration file.
    #[error("failed to parse config at {path}: {reason}")]
    ParseFailed { path: PathBuf, reason: String },

    /// A check has an empty command.
    #[error("check '{name}' must define a non-empty command")]
    EmptyCommand { name: String },

    /// A check's formatter has an empty command.
    #[error("formatter for check '{name}' must define a non-empty command")]
    EmptyFormatter { name: String },

    /// A check's fixer has an empty command.
    #[error("fixer for check '{name}' must define a non-empty command")]
    EmptyFixer { name: String },

    /// An agent has an empty command.
    #[error("{role} agent must define a non-empty command")]
    EmptyAgentCommand { role: String },
}

/// Errors related to agent resolution.
#[derive(Debug, Error)]
pub enum AgentError {
    /// The specified agent type is not supported.
    #[error("unsupported agent type: {0}")]
    UnsupportedType(String),

    /// The agent binary was not found in PATH.
    #[error("agent binary '{binary}' not found in PATH")]
    BinaryNotFound { binary: String },

    /// No agent configured for the specified role.
    #[error("no {role} agent configured (use --agent or configure in scanner.toml)")]
    NotConfigured { role: String },
}

/// Errors that can occur during CLI operations.
#[derive(Debug, Error)]
pub enum CliError {
    /// No checks matched the provided filters.
    #[error("no checks matched the requested filters: {filters:?}")]
    NoMatchingChecks { filters: Vec<String> },

    /// The specified root path does not exist.
    #[error("--root path does not exist: {0}")]
    RootNotFound(PathBuf),

    /// The specified root path is not a directory.
    #[error("--root must be a directory: {0}")]
    RootNotDirectory(PathBuf),

    /// Checks failed and no fixes were attempted.
    #[error("{count} check(s) failed ({reason}; no fixes attempted)")]
    ChecksFailed { count: usize, reason: String },

    /// Checks still failing after fixes were applied.
    #[error("{count} check(s) still failing after fixes")]
    FixesIncomplete { count: usize },

    /// Checks still failing after fixes, but some failures are not auto-fixable
    /// because they produced no actionable GitHub Actions annotations.
    #[error(
        "{count} check(s) still failing after fixes ({unfixable} not auto-fixable: no actionable GitHub Actions annotations)"
    )]
    FixesIncompleteUnfixable { count: usize, unfixable: usize },
}

/// Errors that can occur during process execution.
#[derive(Debug, Error)]
pub enum ProcessError {
    /// Failed to spawn the process.
    #[error("failed to spawn process: {0}")]
    SpawnFailed(String),

    /// The process timed out.
    #[error("process timed out after {0:?}")]
    Timeout(Duration),

    /// Failed to write to stdin.
    #[error("failed to write to stdin: {0}")]
    StdinWriteFailed(String),

    /// Failed to read from stdout/stderr.
    #[error("failed to read process output: {0}")]
    OutputReadFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_error_display() {
        let err = CheckError::FormatterFailed {
            exit_code: Some(1),
            stderr: "parse error".to_string(),
        };
        assert!(err.to_string().contains("exit code"));
        assert!(err.to_string().contains("parse error"));
    }

    #[test]
    fn fix_error_display() {
        let err = FixError::InvalidBatchSize;
        assert_eq!(err.to_string(), "batch size must be greater than 0");
    }

    #[test]
    fn config_error_display() {
        let err = ConfigError::EmptyCommand {
            name: "test".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "check 'test' must define a non-empty command"
        );
    }

    #[test]
    fn cli_error_display() {
        let err = CliError::NoMatchingChecks {
            filters: vec!["foo".to_string(), "bar".to_string()],
        };
        assert!(err.to_string().contains("foo"));
        assert!(err.to_string().contains("bar"));
    }

    #[test]
    fn process_error_display() {
        let err = ProcessError::SpawnFailed("not found".to_string());
        assert!(err.to_string().contains("spawn"));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn agent_error_display() {
        let err = AgentError::NotConfigured {
            role: "analyzer".to_string(),
        };
        assert!(err.to_string().contains("analyzer"));
        assert!(err.to_string().contains("configured"));
    }
}
