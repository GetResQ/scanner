use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::Cli;
use crate::agents::resolve_agent;
use crate::config;
use crate::demo;
use crate::error::{CliError, ConfigError};
use crate::fix;
use crate::gha;
use crate::pool::Pool;
use crate::runner;
use crate::ui;

#[derive(clap::Subcommand, Debug, Clone)]
pub enum Command {
    /// Run checks (optionally filtered by name or tag)
    Check {
        /// Check names or tags to run; if omitted, all checks run
        filters: Vec<String>,
    },
    /// Run a simulated TUI demo (no commands executed)
    Demo {
        /// Disable TUI (headless demo)
        #[arg(long)]
        quiet: bool,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    // Demo mode exits early
    if let Some(Command::Demo { quiet }) = &cli.command {
        let use_tui = !quiet && atty::is(atty::Stream::Stdout);
        let use_color = !cli.quiet && atty::is(atty::Stream::Stderr);
        return demo::run_demo(use_tui, use_color).await;
    }

    let config_path = if let Some(cfg) = &cli.config {
        cfg.clone()
    } else if let Some(root) = &cli.root {
        root.join("scanner.toml")
    } else {
        PathBuf::from("scanner.toml")
    };

    let raw = std::fs::read_to_string(&config_path).map_err(|e| ConfigError::ReadFailed {
        path: config_path.clone(),
        reason: e.to_string(),
    })?;

    let cfg = config::Config::from_toml(&raw).map_err(|e| ConfigError::ParseFailed {
        path: config_path.clone(),
        reason: e.to_string(),
    })?;

    let root = compute_root(&cli, &config_path)?;

    let filters = match &cli.command {
        Some(Command::Check { filters }) => filters.clone(),
        None => Vec::new(),
        Some(Command::Demo { .. }) => unreachable!(),
    };

    // Create the shared pool
    let pool = Pool::new(cli.workers);

    let use_tui = cli.tui && atty::is(atty::Stream::Stdout);
    let use_color = !cli.quiet && atty::is(atty::Stream::Stderr);
    let verbose = cli.verbose;
    let (ui_tx, ui_handle) = ui::spawn_ui(use_tui, use_color, verbose, pool.clone());

    let result: Result<()> = async {
        // Run setup commands first (sequentially)
        for setup in &cfg.setup {
            if let Some(tx) = ui_tx.as_ref() {
                let _ = tx
                    .send(ui::UiEvent::CheckStarted {
                        name: format!("setup:{}", setup.name),
                        desc: Some("Setting up".to_string()),
                    })
                    .await;
            }

            let exit_code = runner::run_setup(setup, &root, ui_tx.clone()).await;

            let success = exit_code == Some(0);
            if let Some(tx) = ui_tx.as_ref() {
                let _ = tx
                    .send(ui::UiEvent::CheckFinished {
                        name: format!("setup:{}", setup.name),
                        success,
                        message: if success {
                            "done".to_string()
                        } else {
                            format!("exit {exit_code:?}")
                        },
                        output: None,
                    })
                    .await;
            }

            if !success {
                return Err(CliError::SetupFailed {
                    name: setup.name.clone(),
                    exit_code,
                }
                .into());
            }
        }

        let check_results = runner::run_checks(
            &cfg,
            &filters,
            cli.force,
            &pool,
            false,
            ui_tx.clone(),
            &root,
        )
        .await;

        if check_results.is_empty() {
            return Err(CliError::NoMatchingChecks {
                filters: filters.clone(),
            }
            .into());
        }

        let failures: Vec<_> = check_results
            .iter()
            .filter(|res| {
                res.exit_code != Some(0)
                    || res.annotations.iter().any(|a| gha::is_error_level(a.level))
            })
            .collect();

        if failures.is_empty() {
            return Ok(());
        }

        if cli.dry_run {
            return Err(CliError::ChecksFailed {
                count: failures.len(),
                reason: "dry-run".to_string(),
            }
            .into());
        }

        let agent = resolve_agent(&cli, &cfg)?;

        // Group errors by check type
        let errors_by_check = fix::group_errors_by_check(&check_results);
        if errors_by_check.is_empty() {
            return Err(CliError::ChecksFailed {
                count: failures.len(),
                reason: "no actionable GitHub Actions annotations (configure a formatter or update tool output)".to_string(),
            }
            .into());
        }

        // Run the solve pipeline: each check gets a single agent run
        fix::run_fix_pipeline(
            &agent,
            &errors_by_check,
            &pool,
            &root,
            ui_tx.clone(),
        )
        .await?;

        // Re-run checks once after fixes
        let post_results = runner::run_checks(
            &cfg,
            &filters,
            cli.force,
            &pool,
            false,
            ui_tx.clone(),
            &root,
        )
        .await;

        let remaining: Vec<_> = post_results
            .iter()
            .filter(|res| {
                res.exit_code != Some(0)
                    || res.annotations.iter().any(|a| gha::is_error_level(a.level))
            })
            .collect();

        if remaining.is_empty() {
            Ok(())
        } else {
            let remaining_groups = fix::group_errors_by_check(&post_results);
            let unfixable = remaining
                .iter()
                .filter(|res| !remaining_groups.contains_key(&res.check.name))
                .count();

            if unfixable > 0 {
                Err(CliError::FixesIncompleteUnfixable {
                    count: remaining.len(),
                    unfixable,
                }
                .into())
            } else {
                Err(CliError::FixesIncomplete {
                    count: remaining.len(),
                }
                .into())
            }
        }
    }
    .await;

    if let Some(tx) = ui_tx {
        let _ = tx.send(ui::UiEvent::Done).await;
    }
    let _ = ui_handle.await;

    result
}

fn compute_root(cli: &Cli, config_path: &Path) -> Result<PathBuf> {
    if let Some(root) = &cli.root {
        if !root.exists() {
            return Err(CliError::RootNotFound(root.clone()).into());
        }
        if !root.is_dir() {
            return Err(CliError::RootNotDirectory(root.clone()).into());
        }
        return Ok(root.clone());
    }

    if let Some(parent) = config_path.parent()
        && parent.exists()
    {
        return Ok(parent.to_path_buf());
    }

    std::env::current_dir().context("failed to determine current directory")
}
