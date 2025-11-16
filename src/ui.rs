use std::io::Stdout;
use std::time::Duration;

use arboard::Clipboard;
use ratatui::crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::CrosstermBackend;
use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tokio::sync::mpsc::{self, Receiver, Sender};

#[derive(Debug, Clone)]
pub enum UiEvent {
    CheckStarted {
        name: String,
        desc: Option<String>,
    },
    CheckFinished {
        name: String,
        success: bool,
        message: String,
        output: Option<String>,
    },
    Done,
}

#[derive(Debug, Clone)]
struct CheckRow {
    name: String,
    status: String,
    success: Option<bool>,
    desc: Option<String>,
    output: Option<String>,
}

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
                        rows.push(CheckRow {
                            name,
                            status: "running…".into(),
                            success: None,
                            desc,
                            output: Some("running…".into()),
                        });
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

        // Non-blocking poll for navigation/exit keys (works before and after finished)
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

fn draw(
    terminal: &mut ratatui::Terminal<CrosstermBackend<Stdout>>,
    rows: &[CheckRow],
    selected: usize,
    footer_msg: &str,
) {
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let status = match row.success {
                Some(true) => "[OK]",
                Some(false) => "[X]",
                None => "[..]",
            };
            let base_style = match row.success {
                Some(true) => Style::default().fg(Color::Green),
                Some(false) => Style::default().fg(Color::Red),
                None => Style::default().fg(Color::DarkGray),
            };
            let line_style = if idx == selected {
                base_style.add_modifier(Modifier::BOLD)
            } else {
                base_style
            };
            let status_style = line_style.add_modifier(Modifier::BOLD);
            let indicator = if idx == selected { "|" } else { " " };
            let line = Line::from(vec![
                Span::styled(indicator, line_style),
                Span::raw(" "),
                Span::styled(status, status_style),
                Span::raw(" "),
                Span::styled(row.name.clone(), line_style.clone()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Checks"));

    let detail = rows.get(selected).map(|row| {
        Paragraph::new(detail_text(row))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Details"))
    });

    let _ = terminal.draw(|frame| {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(frame.area());

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(outer[0]);

        frame.render_widget(list, columns[0]);
        if let Some(detail) = detail {
            frame.render_widget(detail, columns[1]);
        }

        let footer = Paragraph::new(footer_msg.to_string())
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: true });
        frame.render_widget(footer, outer[1]);
    });
}

fn detail_text(row: &CheckRow) -> String {
    let desc = &row.desc;
    let status = match row.success {
        Some(true) => "passed",
        Some(false) => "failed",
        None => "running",
    };
    let output = row.output.as_deref().unwrap_or("").trim();
    format!(
        "Check: {}\nStatus: {}\nMessage: {}\nDescription: {}\n\nOutput:\n{}",
        row.name,
        status,
        row.status,
        desc.as_deref().unwrap_or(""),
        if output.is_empty() {
            "(no output)"
        } else {
            output
        }
    )
}

fn cleanup_terminal(mut terminal: ratatui::Terminal<CrosstermBackend<Stdout>>) {
    let _ = terminal.clear();
    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();
    // Move to a fresh line so shell prompt renders cleanly
    println!();
}
