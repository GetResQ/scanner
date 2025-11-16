mod agents;
mod cli;
mod config;
mod demo;
mod fix;
mod gha;
mod process;
mod runner;
mod ui;

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "scanner",
    version,
    about = "Concurrent project scanner and fixer",
    author = "Resq"
)]
pub struct Cli {
    /// Path to scanner configuration (defaults to scanner.toml)
    #[arg(long)]
    config: Option<std::path::PathBuf>,

    /// Root directory to run checks from (defaults to config's folder or current dir)
    #[arg(long)]
    root: Option<std::path::PathBuf>,

    /// Maximum number of concurrent workers (0 = number of CPUs)
    #[arg(long, default_value_t = 0)]
    workers: usize,

    /// Batch size for fixer runs (only used during fixing)
    #[arg(long, default_value_t = 5)]
    batch_size: usize,

    /// Only run checks; do not attempt to fix
    #[arg(long)]
    dry_run: bool,

    /// Skip automatic fixing stage after failed checks
    #[arg(long)]
    no_fix: bool,

    /// Disable interactive TUI and emit plain logs
    #[arg(long)]
    quiet: bool,

    /// Agent to use for analyzer/fixer (codex|claude). Overrides config agents.
    #[arg(long, value_parser = ["codex", "claude"])]
    agent: Option<String>,

    /// Include disabled checks when named explicitly
    #[arg(long)]
    force: bool,

    /// Model name for the selected agent (e.g. gpt-5.1-codex-mini, gpt-5-codex, sonnet, opus)
    #[arg(short = 'm', long)]
    model: Option<String>,

    #[command(subcommand)]
    pub command: Option<cli::Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    cli::run(cli).await
}
