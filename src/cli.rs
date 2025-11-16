use std::path::PathBuf;

use anyhow::{Context, Result};
use atty;
use num_cpus;

use crate::Cli;
use crate::agents::resolve_agent;
use crate::config;
use crate::demo;
use crate::fix;
use crate::gha;
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

    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config at {}", config_path.display()))?;

    let cfg = config::Config::from_toml(&raw)
        .with_context(|| format!("failed to parse config at {}", config_path.display()))?;

    let root = compute_root(&cli, &config_path)?;

    let filters = match &cli.command {
        Some(Command::Check { filters }) => filters.clone(),
        None => Vec::new(),
        Some(Command::Demo { .. }) => unreachable!(),
    };

    let workers = effective_workers(cli.workers);

    let use_tui = !cli.quiet && atty::is(atty::Stream::Stdout);
    let (ui_tx, ui_handle) = ui::spawn_ui(use_tui);

    let check_results = runner::run_checks(
        &cfg,
        &filters,
        cli.force,
        workers,
        cli.quiet,
        ui_tx.clone(),
        &root,
    )
    .await?;

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
        anyhow::bail!("checks failed ({}; no fixes attempted)", reason);
    }

    let analyzer = resolve_agent("analyzer", &cli, &cfg)?;
    let fixer = resolve_agent("fixer", &cli, &cfg)?;

    let error_groups = fix::group_errors(&check_results);
    if error_groups.is_empty() {
        anyhow::bail!("checks failed but no actionable error groups were produced");
    }

    if let Some(tx) = ui_tx.clone() {
        let _ = tx
            .send(ui::UiEvent::CheckStarted {
                name: "analyzer".into(),
                desc: Some("Analyze failures".into()),
            })
            .await;
    }
    let analysis = fix::run_analyzer(&analyzer, &error_groups, &root).await?;
    if let Some(tx) = ui_tx.clone() {
        let _ = tx
            .send(ui::UiEvent::CheckFinished {
                name: "analyzer".into(),
                success: true,
                message: "done".into(),
                output: Some(analysis.clone()),
            })
            .await;
    }

    if let Some(tx) = ui_tx.clone() {
        let _ = tx
            .send(ui::UiEvent::CheckStarted {
                name: "fixer".into(),
                desc: Some("Apply fixes".into()),
            })
            .await;
    }
    fix::run_fixer_batches(
        &fixer,
        &analysis,
        &error_groups,
        cli.batch_size,
        workers,
        &root,
    )
    .await?;
    if let Some(tx) = ui_tx.clone() {
        let _ = tx
            .send(ui::UiEvent::CheckFinished {
                name: "fixer".into(),
                success: true,
                message: "applied".into(),
                output: Some("Applied fixes.".into()),
            })
            .await;
    }

    // Re-run checks once after fixes
    let post_results = runner::run_checks(
        &cfg,
        &filters,
        cli.force,
        workers,
        cli.quiet,
        ui_tx.clone(),
        &root,
    )
    .await?;
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
        anyhow::bail!("checks still failing after fixes ({})", remaining.len())
    }
}

fn effective_workers(value: usize) -> usize {
    if value == 0 {
        num_cpus::get().max(1)
    } else {
        value
    }
}

fn compute_root(cli: &Cli, config_path: &PathBuf) -> Result<PathBuf> {
    if let Some(root) = &cli.root {
        if !root.exists() {
            anyhow::bail!("--root path does not exist: {}", root.display());
        }
        if !root.is_dir() {
            anyhow::bail!("--root must be a directory: {}", root.display());
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
