use std::time::{Duration, Instant};

use arboard::Clipboard;
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::CrosstermBackend;
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::pool::Pool;
use crate::ui::cli;
use crate::ui::events::UiEvent;
use crate::ui::render::{cleanup_terminal, detail_text, draw};
use crate::ui::state::{AppState, CheckRow};

pub fn spawn_ui(
    enable_tui: bool,
    use_color: bool,
    verbose: bool,
    pool: Pool,
) -> (Option<Sender<UiEvent>>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(256);

    // Pool stats ticker is only useful in interactive TUI mode.
    if enable_tui {
        let tx_stats = tx.clone();
        tokio::spawn(async move {
            loop {
                let stats = pool.stats();
                if tx_stats.send(UiEvent::PoolStats(stats)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }

    let handle = tokio::spawn(async move {
        if enable_tui {
            run_tui(rx).await;
        } else {
            cli::run_cli(rx, use_color, verbose).await;
        }
    });
    (Some(tx), handle)
}

async fn run_tui(mut rx: Receiver<UiEvent>) {
    if enable_raw_mode().is_err() {
        // fallback: consume events and do nothing
        while rx.recv().await.is_some() {}
        return;
    }

    // Ensure terminal is restored even if this task panics.
    struct TuiGuard {
        cleaned: bool,
    }
    impl Drop for TuiGuard {
        fn drop(&mut self) {
            if self.cleaned {
                return;
            }
            let _ = disable_raw_mode();
            let mut stdout = std::io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
        }
    }
    let mut guard = TuiGuard { cleaned: false };

    let mut stdout = std::io::stdout();
    if execute!(stdout, EnterAlternateScreen, cursor::Hide).is_err() {
        while rx.recv().await.is_some() {}
        return;
    }

    let mut terminal = match ratatui::Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(t) => t,
        Err(_) => {
            while rx.recv().await.is_some() {}
            return;
        }
    };

    let _ = terminal.clear();

    let mut state = AppState::new();
    let mut clipboard = Clipboard::new().ok();
    let mut footer_msg =
        "Up/Down move | q/Esc exit (double-press while running) | y copy".to_string();
    let mut quit_armed_until: Option<Instant> = None;

    loop {
        // Consume all pending events
        while let Ok(ev) = rx.try_recv() {
            match ev {
                UiEvent::CheckStarted { name, desc } => {
                    if let Some(row) = state.rows.iter_mut().find(|r| r.name == name) {
                        row.status = "running".into();
                        row.success = None;
                        row.output = Some("running".into());
                        row.desc = desc;
                    } else {
                        state.rows.push(CheckRow::new(name, desc));
                    }
                }
                UiEvent::CheckFinished {
                    name,
                    success,
                    message,
                    output,
                } => {
                    if let Some(row) = state.rows.iter_mut().find(|r| r.name == name) {
                        row.success = Some(success);
                        row.status = message;
                        row.output = output;
                    } else {
                        let mut row = CheckRow::new(name.clone(), None);
                        row.success = Some(success);
                        row.status = message;
                        row.output = output;
                        state.rows.push(row);
                    }
                }
                UiEvent::PoolStats(stats) => {
                    state.pool_stats = Some(stats);
                }
                UiEvent::StreamLine {
                    source,
                    stream,
                    line,
                } => {
                    state.add_stream_line(source, stream, line);
                }
                UiEvent::Done => {
                    state.finished = true;
                    footer_msg = "Done | Up/Down move | q/Esc exit | y copy".to_string();
                }
            }
        }

        // Reset quit-arm after timeout.
        if let Some(until) = quit_armed_until
            && Instant::now() > until
        {
            quit_armed_until = None;
            if !state.finished {
                footer_msg =
                    "Up/Down move | q/Esc exit (double-press while running) | y copy".to_string();
            }
        }

        // Poll for keyboard input
        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(Event::Key(key)) = event::read()
        {
            match key.code {
                // Ctrl+C always quits
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.exit_requested = true;
                }
                // q/Esc only quit when not busy
                KeyCode::Char('q') | KeyCode::Esc => {
                    if state.finished {
                        state.exit_requested = true;
                        continue;
                    }

                    // Double-press while checks run.
                    let now = Instant::now();
                    if let Some(until) = quit_armed_until
                        && now <= until
                    {
                        state.exit_requested = true;
                        continue;
                    }
                    quit_armed_until = Some(now + Duration::from_millis(800));
                    footer_msg = "Scanner busy - press q/Esc again to quit | Ctrl+C to force quit"
                        .to_string();
                }
                KeyCode::Up => {
                    if state.selected > 0 {
                        state.selected -= 1;
                    }
                }
                KeyCode::Down => {
                    if state.selected + 1 < state.rows.len() {
                        state.selected += 1;
                    }
                }
                KeyCode::Char('y') => {
                    if let (Some(cb), Some(row)) =
                        (clipboard.as_mut(), state.rows.get(state.selected))
                    {
                        let _ = cb.set_text(detail_text(row));
                    }
                }
                _ => {}
            }
        }

        // Tick spinner animation
        state.tick_spinner();

        draw(&mut terminal, &state, &footer_msg);

        if state.exit_requested {
            break;
        }
    }

    cleanup_terminal(terminal);
    guard.cleaned = true;
}
