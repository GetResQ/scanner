use std::time::Duration;

use arboard::Clipboard;
use ratatui::crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::CrosstermBackend;
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::ui::events::UiEvent;
use crate::ui::render::{cleanup_terminal, detail_text, draw};
use crate::ui::state::CheckRow;

pub fn spawn_ui(enable: bool) -> (Option<Sender<UiEvent>>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(100);
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

    let mut rows: Vec<CheckRow> = Vec::new();
    let mut selected: usize = 0;

    let mut finished = false;
    let mut exit_requested = false;
    let mut exit_armed = false;
    let mut footer_msg = "↑/↓ move • q/esc exit • y copy details".to_string();
    let mut clipboard = Clipboard::new().ok();

    loop {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                UiEvent::CheckStarted { name, desc } => {
                    if let Some(row) = rows.iter_mut().find(|r| r.name == name) {
                        row.status = "running…".into();
                        row.success = None;
                        row.output = Some("running…".into());
                        row.desc = desc;
                    } else {
                        rows.push(CheckRow::new(name, desc));
                    }
                }
                UiEvent::CheckFinished {
                    name,
                    success,
                    message,
                    output,
                } => {
                    if let Some(row) = rows.iter_mut().find(|r| r.name == name) {
                        row.success = Some(success);
                        row.status = message;
                        row.output = output;
                    }
                }
                UiEvent::Done => {
                    finished = true;
                    exit_armed = false;
                    footer_msg = "↑/↓ move • q/esc exit • y copy details".to_string();
                }
            }
        }

        if event::poll(Duration::from_millis(10)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if finished || exit_armed {
                            exit_requested = true;
                        } else {
                            exit_armed = true;
                            footer_msg =
                                "Checks still running – press q/esc again to exit".to_string();
                        }
                    }
                    KeyCode::Up => {
                        if selected > 0 {
                            selected -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if selected + 1 < rows.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Char('y') => {
                        if let (Some(cb), Some(row)) = (clipboard.as_mut(), rows.get(selected)) {
                            let _ = cb.set_text(detail_text(row));
                        }
                    }
                    _ => {}
                }
            }
        }

        draw(&mut terminal, &rows, selected, &footer_msg);

        if exit_requested {
            break;
        }
    }

    cleanup_terminal(terminal);
}
