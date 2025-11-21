# Scanner

AI-powered project scanner with TUI output and optional automated fixes.
Scanner uses a set of tools defined by the user to run various automated tests.
After running these tests, scanner launches concurrent AI agents to diagnose and
fix problems.

## Prerequisites

Scanner requires an AI coding agent to analyze and fix errors. You must have **one of the following** installed and configured:

### Option 1: Codex (OpenAI)
```bash
# Install Codex CLI
npm install -g @openai/codex

# Login and configure
codex auth
```

### Option 2: Claude Code (Anthropic)
```bash
# Install Claude Code
npm install -g @anthropic-ai/claude-code

# Login and configure
claude login
```

Once configured, specify which agent to use:
- Via CLI: `scanner --agent codex` or `scanner --agent claude`
- Via config: Set `[agents.analyzer]` and `[agents.fixer]` in `scanner.toml`

### Note:
All check tools are expected to **return results in GitHub Actions annotation format** (`::error file=X,line=Y::message`). If your tool outputs a different format (e.g., JSON), specify a `formatter` command that converts the output to GHA format.

## Install (pre-built binaries)
```bash
curl -sSL https://raw.githubusercontent.com/getresq/scanner/master/scripts/install.sh | bash
```

## Build from source (Rust)
Install Rust toolchain (1.79+), `cargo`.

```sh
cargo build --release
# binary: target/release/scanner
```

To install into your toolchain’s bin dir:

```sh
cargo install --path .
```

## Usage
- Run scanner: `scanner`
- Disable fixes: `scanner --no-fix`
- TUI keys: `↑/↓` move, `y` copy details, `q/esc` exit (double-press while checks run).

## Configuration
See `scanner.toml` for checks and agent settings. Each project can keep its own config alongside the codebase.

Tip: if multiple checks contend for a shared resource (package manager cache, codegen outputs, etc.), set a shared `lock = "name"` on those checks to force them to run one-at-a-time while keeping other checks concurrent.
