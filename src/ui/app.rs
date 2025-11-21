use std::time::Duration;

use arboard::Clipboard;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::CrosstermBackend;
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::pool::Pool;
use crate::ui::events::UiEvent;
use crate::ui::render::{cleanup_terminal, detail_text, draw};
use crate::ui::state::{AppState, CheckRow};

pub fn spawn_ui(enable: bool, pool: Pool) -> (Option<Sender<UiEvent>>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(256);

    // Spawn pool stats ticker
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

    let handle = tokio::spawn(async move { run_ui(enable, rx).await });
    (Some(tx), handle)
}

async fn run_ui(enable: bool, mut rx: Receiver<UiEvent>) {
    if !enable {
        while rx.recv().await.is_some() {}
        return;
    }

    if enable_raw_mode().is_err() {
        // fallback: consume events and do nothing
        while rx.recv().await.is_some() {}
        return;
    }

    let mut terminal = match ratatui::Terminal::new(CrosstermBackend::new(std::io::stdout())) {
        Ok(t) => t,
        Err(_) => {
            while rx.recv().await.is_some() {}
            let _ = disable_raw_mode();
            return;
        }
    };

    let _ = terminal.clear();

    let mut state = AppState::new();
    let mut clipboard = Clipboard::new().ok();
    let mut footer_msg = "Up/Down move | q/Esc exit | y copy".to_string();

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
                    footer_msg = "Up/Down move | q/Esc exit | y copy".to_string();
                }
            }
        }

        // Poll for keyboard input
        if event::poll(Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                match key.code {
                    // Ctrl+C always quits
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        state.exit_requested = true;
                    }
                    // q/Esc only quit when not busy
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if state.finished {
                            state.exit_requested = true;
                        } else {
                            // Show warning - scanner is busy
                            footer_msg = "Scanner busy - Ctrl+C to force quit".to_string();
                        }
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
        }

        // Tick spinner animation
        state.tick_spinner();

        draw(&mut terminal, &state, &footer_msg);

        if state.exit_requested {
            break;
        }
    }

    cleanup_terminal(terminal);
}
