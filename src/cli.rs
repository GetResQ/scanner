use std::path::PathBuf;

use anyhow::{Context, Result};
use atty;

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
        return demo::run_demo(use_tui).await;
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

    let use_tui = !cli.quiet && atty::is(atty::Stream::Stdout);
    let (ui_tx, ui_handle) = ui::spawn_ui(use_tui, pool.clone());

    let check_results = runner::run_checks(
        &cfg,
        &filters,
        cli.force,
        &pool,
        cli.quiet,
        ui_tx.clone(),
        &root,
    )
    .await;

    if check_results.is_empty() {
        if let Some(tx) = ui_tx {
            let _ = tx.send(ui::UiEvent::Done).await;
        }
        let _ = ui_handle.await;
        return Err(CliError::NoMatchingChecks { filters }.into());
    }

    let failures: Vec<_> = check_results
        .iter()
        .filter(|res| {
            res.exit_code != Some(0) || res.annotations.iter().any(|a| gha::is_error_level(a.level))
        })
        .collect();

    if failures.is_empty() {
        if let Some(tx) = ui_tx {
            let _ = tx.send(ui::UiEvent::Done).await;
        }
        let _ = ui_handle.await;
        return Ok(());
    }

    if cli.dry_run || cli.no_fix {
        if let Some(tx) = ui_tx {
            let _ = tx.send(ui::UiEvent::Done).await;
        }
        let _ = ui_handle.await;
        let reason = if cli.dry_run { "dry-run" } else { "no-fix" };
        return Err(CliError::ChecksFailed {
            count: failures.len(),
            reason: reason.to_string(),
        }
        .into());
    }

    let analyzer = resolve_agent("analyzer", &cli, &cfg)?;
    let fixer = resolve_agent("fixer", &cli, &cfg)?;

    // Group errors by check type
    let errors_by_check = fix::group_errors_by_check(&check_results);
    if errors_by_check.is_empty() {
        if let Some(tx) = ui_tx {
            let _ = tx.send(ui::UiEvent::Done).await;
        }
        let _ = ui_handle.await;
        return Err(CliError::ChecksFailed {
            count: failures.len(),
            reason: "no actionable error groups".to_string(),
        }
        .into());
    }

    // Run the fix pipeline: each check gets its own analyzer -> fixer(s)
    fix::run_fix_pipeline(
        &analyzer,
        &fixer,
        &errors_by_check,
        cli.batch_size,
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
        cli.quiet,
        ui_tx.clone(),
        &root,
    )
    .await;

    let remaining: Vec<_> = post_results
        .iter()
        .filter(|res| {
            res.exit_code != Some(0) || res.annotations.iter().any(|a| gha::is_error_level(a.level))
        })
        .collect();

    if let Some(tx) = ui_tx {
        let _ = tx.send(ui::UiEvent::Done).await;
    }
    let _ = ui_handle.await;

    if remaining.is_empty() {
        Ok(())
    } else {
        Err(CliError::FixesIncomplete {
            count: remaining.len(),
        }
        .into())
    }
}

fn compute_root(cli: &Cli, config_path: &PathBuf) -> Result<PathBuf> {
    if let Some(root) = &cli.root {
        if !root.exists() {
            return Err(CliError::RootNotFound(root.clone()).into());
        }
        if !root.is_dir() {
            return Err(CliError::RootNotDirectory(root.clone()).into());
        }
        return Ok(root.clone());
    }

    if let Some(parent) = config_path.parent() {
        if parent.exists() {
            return Ok(parent.to_path_buf());
        }
    }

    std::env::current_dir().context("failed to determine current directory")
}
