//! CLI output with optional colors and spinners.

use std::collections::HashSet;
use std::io::{Write, stderr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::style::{Color, ResetColor, SetForegroundColor};
use crossterm::{cursor, execute, terminal};
use tokio::sync::mpsc::Receiver;

use crate::ui::events::{StreamType, UiEvent};

/// Braille spinner frames.
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// ANSI color codes for spinner (always uses raw ANSI to avoid crossterm overhead).
mod ansi {
    pub const RESET: &str = "\x1b[0m";
    pub const CYAN: &str = "\x1b[36m";
}

/// Global flag to track if we have an active spinner line that needs clearing.
static HAS_SPINNER_LINE: AtomicBool = AtomicBool::new(false);

/// Output style configuration.
#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
    pub spinner: bool,
}

impl Style {
    pub fn colored() -> Self {
        Self {
            color: true,
            spinner: true,
        }
    }

    pub fn plain() -> Self {
        Self {
            color: false,
            spinner: false,
        }
    }
}

/// Clear the current line if we have a spinner showing.
fn clear_spinner_line() {
    if HAS_SPINNER_LINE.swap(false, Ordering::SeqCst) {
        let mut stderr = stderr();
        let _ = execute!(
            stderr,
            cursor::MoveToColumn(0),
            terminal::Clear(terminal::ClearType::CurrentLine)
        );
    }
}

/// Print with color support.
fn cprint(style: Style, color: Color, text: &str) {
    let mut stderr = stderr();
    if style.color {
        let _ = execute!(stderr, SetForegroundColor(color));
        eprint!("{text}");
        let _ = execute!(stderr, ResetColor);
    } else {
        eprint!("{text}");
    }
}

/// ASCII art logo displayed at startup.
const LOGO: &str = r#"
┏━━        ━━┓
   scanner
┗━━        ━━┛
"#;

/// Run the CLI output loop (non-TUI mode).
pub async fn run_cli(mut rx: Receiver<UiEvent>, use_color: bool, verbose: bool) {
    let style = if use_color {
        Style::colored()
    } else {
        Style::plain()
    };

    // Print logo banner when colors are enabled
    if style.color {
        cprint(style, Color::Cyan, LOGO);
        eprintln!();
    }

    let mut running: HashSet<String> = HashSet::new();
    let mut spinner_tick: usize = 0;
    let mut cursor_hidden = false;

    loop {
        // Process all available events
        loop {
            match rx.try_recv() {
                Ok(ev) => {
                    clear_spinner_line();

                    match ev {
                        UiEvent::CheckStarted { name, desc } => {
                            running.insert(name.clone());
                            print_started(&name, desc.as_deref(), style);
                        }
                        UiEvent::CheckFinished {
                            name,
                            success,
                            message,
                            ..
                        } => {
                            running.remove(&name);
                            print_finished(&name, success, &message, style);
                        }
                        UiEvent::StreamLine {
                            source,
                            stream,
                            line,
                        } => {
                            // Only show streaming output in verbose mode
                            if verbose {
                                print_stream(&source, stream, &line, style);
                            }
                        }
                        UiEvent::PoolStats(_) => {}
                        UiEvent::Done => {
                            clear_spinner_line();
                            if cursor_hidden {
                                let _ = execute!(stderr(), cursor::Show);
                            }
                            return;
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    if cursor_hidden {
                        let _ = execute!(stderr(), cursor::Show);
                    }
                    return;
                }
            }
        }

        // Update spinner for running checks
        if !running.is_empty() && style.spinner {
            // Hide cursor while spinner is active
            if !cursor_hidden {
                let _ = execute!(stderr(), cursor::Hide);
                cursor_hidden = true;
            }
            spinner_tick = spinner_tick.wrapping_add(1);
            print_spinner(&running, spinner_tick);
        } else if cursor_hidden {
            // Show cursor when no spinner
            let _ = execute!(stderr(), cursor::Show);
            cursor_hidden = false;
        }

        tokio::time::sleep(Duration::from_millis(80)).await;
    }
}

fn print_started(name: &str, desc: Option<&str>, style: Style) {
    cprint(style, Color::Cyan, &format!("● {name}"));
    if let Some(desc) = desc {
        cprint(style, Color::DarkGrey, &format!(" {desc}"));
    }
    eprintln!();
}

fn print_finished(name: &str, success: bool, message: &str, style: Style) {
    let (symbol, color) = if success {
        ("✓", Color::Green)
    } else {
        ("✗", Color::Red)
    };
    cprint(style, color, &format!("{symbol} {name}"));
    cprint(style, Color::DarkGrey, &format!(": {message}"));
    eprintln!();
}

fn print_stream(source: &str, stream: StreamType, line: &str, style: Style) {
    let color = match stream {
        StreamType::Stdout => Color::DarkGrey,
        StreamType::Stderr => Color::Yellow,
    };
    cprint(style, Color::DarkGrey, "│ ");
    cprint(style, color, &format!("[{source}] "));
    eprintln!("{line}");
}

fn print_spinner(running: &HashSet<String>, tick: usize) {
    let frame = SPINNER[tick % SPINNER.len()];
    let names: Vec<&str> = running.iter().map(|s| s.as_str()).collect();

    let text = if names.len() == 1 {
        format!("{frame} {}", names[0])
    } else {
        format!("{frame} {} tasks", names.len())
    };

    let mut stderr = stderr();
    let _ = execute!(stderr, cursor::MoveToColumn(0));
    eprint!("{}{}{}", ansi::CYAN, text, ansi::RESET);
    // Pad with spaces to clear any previous longer text
    eprint!("          ");
    let _ = execute!(stderr, cursor::MoveToColumn(0));
    let _ = stderr.flush();

    HAS_SPINNER_LINE.store(true, Ordering::SeqCst);
}
