mod agents;
mod cli;
mod config;
mod demo;
mod error;
mod fix;
mod gha;
mod pool;
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

    /// Disable colors and spinners (plain text output)
    #[arg(long)]
    quiet: bool,

    /// Enable interactive TUI (experimental)
    #[arg(long)]
    tui: bool,

    /// Show verbose output including streaming from checks and agents
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Agent to use for analyzer/fixer (codex|claude). Overrides config agents.
    #[arg(long, value_parser = ["codex", "claude"])]
    agent: Option<String>,

    /// Include disabled checks when named explicitly
    #[arg(long)]
    force: bool,

    /// Model name for the selected agent (e.g. gpt-5.1-codex-max, gpt-5-codex, sonnet, opus)
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_accepts_quiet_flag() {
        let cli = Cli::try_parse_from(["scanner", "--quiet"]).expect("parse");
        assert!(cli.quiet);
    }

    #[test]
    fn cli_rejects_removed_plain_flag() {
        let err = Cli::try_parse_from(["scanner", "--plain"]).expect_err("expected parse error");
        assert!(err.to_string().contains("--plain"));
    }
}
