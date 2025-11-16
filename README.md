# Scanner

AI-powered project scanner with TUI output and optional automated fixes.
Scanner uses a set of tools defined by the user to run various automated test.
After running these test, scanner launches concurrent AI agents to diagnose and
fix problems.

### Note:
All tools are expected to **return results in Github Actions**  format. If not available
the user can specify a customer formatter.

## Install (pre-built binaries)
```bash
curl -sSL https://raw.githubusercontent.com/mazdak/scanner/refs/heads/master/scripts/install.sh | bash
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
