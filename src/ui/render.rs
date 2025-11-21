use std::io::Stdout;

use crossterm::cursor;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::pool::PoolStats;
use crate::ui::events::StreamType;
use crate::ui::state::{AppState, CheckRow};

/// Braille spinner frames for running tasks.
const BRAILLE_SPINNER: &[&str] = &[
    "\u{28f7}", // ⣷
    "\u{28ef}", // ⣯
    "\u{28df}", // ⣟
    "\u{287f}", // ⡿
    "\u{28bf}", // ⢿
    "\u{28fb}", // ⣻
    "\u{28fd}", // ⣽
    "\u{28fe}", // ⣾
];

/// Get spinner frame for given tick.
pub fn spinner_frame(tick: usize) -> &'static str {
    BRAILLE_SPINNER[tick % BRAILLE_SPINNER.len()]
}

pub(crate) fn draw(
    terminal: &mut ratatui::Terminal<CrosstermBackend<Stdout>>,
    state: &AppState,
    footer_msg: &str,
) {
    let items: Vec<ListItem> = state
        .rows
        .iter()
        .enumerate()
        .map(|(idx, row)| list_item(row, idx == state.selected, state.spinner_tick))
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Checks"));

    // Build detail panel content
    let detail_content = build_detail_content(state);
    let detail = Paragraph::new(detail_content)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Output"));

    let _ = terminal.draw(|frame| {
        // Main layout: content area + pool bar + footer
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // Main content
                Constraint::Length(1), // Pool bar
                Constraint::Length(1), // Footer
            ])
            .split(frame.area());

        // Split main content into left (checks) and right (details/stream)
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(outer[0]);

        frame.render_widget(list, columns[0]);
        frame.render_widget(detail, columns[1]);

        // Render pool bar
        let pool_bar = render_pool_bar(state.pool_stats.as_ref(), outer[1].width as usize);
        let pool_widget = Paragraph::new(pool_bar);
        frame.render_widget(pool_widget, outer[1]);

        // Render footer
        let footer = Paragraph::new(footer_msg.to_string())
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: true });
        frame.render_widget(footer, outer[2]);
    });
}

/// Build the detail panel content - shows selected check details or live stream.
fn build_detail_content(state: &AppState) -> String {
    // If we have a selected row, show its details
    if let Some(row) = state.rows.get(state.selected) {
        // If the selected row is running, show live stream for it
        if row.success.is_none() && !state.stream_buffer.is_empty() {
            let mut lines = Vec::new();
            lines.push(format!("Check: {}", row.name));
            lines.push("Status: running".to_string());
            lines.push(String::new());
            lines.push("--- Live Output ---".to_string());

            // Show stream lines for this source
            for sl in state.stream_buffer.iter().rev().take(50) {
                if sl.source == row.name {
                    let prefix = match sl.stream {
                        StreamType::Stderr => "!",
                        StreamType::Stdout => " ",
                    };
                    lines.push(format!("{} {}", prefix, sl.line));
                }
            }

            // If no specific output, show all recent
            if lines.len() <= 4 {
                lines.push("(showing all output)".to_string());
                for sl in state.stream_buffer.iter().rev().take(30) {
                    let prefix = match sl.stream {
                        StreamType::Stderr => "!",
                        StreamType::Stdout => " ",
                    };
                    lines.push(format!("[{}]{} {}", sl.source, prefix, sl.line));
                }
            }

            return lines.join("\n");
        }

        // Otherwise show static details
        return detail_text(row);
    }

    // No selection - show combined stream
    if !state.stream_buffer.is_empty() {
        let mut lines = vec!["--- Live Output ---".to_string()];
        for sl in state.stream_buffer.iter().rev().take(50) {
            let prefix = match sl.stream {
                StreamType::Stderr => "!",
                StreamType::Stdout => " ",
            };
            lines.push(format!("[{}]{} {}", sl.source, prefix, sl.line));
        }
        return lines.join("\n");
    }

    "(no output)".to_string()
}

/// Render the pool utilization bar.
fn render_pool_bar(stats: Option<&PoolStats>, width: usize) -> Line<'static> {
    let Some(stats) = stats else {
        return Line::from(vec![Span::styled(
            " Pool: --",
            Style::default().fg(Color::DarkGray),
        )]);
    };

    // Calculate bar width (leave room for text)
    let text_width = 25; // " [........] N/N active (Q queued)"
    let bar_width = width.saturating_sub(text_width).max(8);

    // Calculate filled portion
    let filled = if stats.capacity > 0 {
        (stats.active as f64 / stats.capacity as f64 * bar_width as f64).round() as usize
    } else {
        0
    };
    let empty = bar_width.saturating_sub(filled);

    // Build bar using block characters
    let filled_str: String = "\u{2588}".repeat(filled); // █
    let empty_str: String = "\u{2591}".repeat(empty); // ░

    // Color based on utilization
    let bar_color = if stats.active == stats.capacity {
        Color::Red
    } else if stats.active > stats.capacity / 2 {
        Color::Yellow
    } else {
        Color::Green
    };

    let mut spans = vec![
        Span::raw(" "),
        Span::styled(filled_str, Style::default().fg(bar_color)),
        Span::styled(empty_str, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
    ];

    // Add stats text
    let stats_text = if stats.queued > 0 {
        format!(
            "{}/{} active (+{} queued)",
            stats.active, stats.capacity, stats.queued
        )
    } else {
        format!("{}/{} active", stats.active, stats.capacity)
    };

    spans.push(Span::styled(stats_text, Style::default().fg(Color::White)));

    Line::from(spans)
}

pub(crate) fn detail_text(row: &CheckRow) -> String {
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

pub(crate) fn cleanup_terminal(mut terminal: ratatui::Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show);
    let _ = terminal.show_cursor();
    println!();
}

fn list_item(row: &CheckRow, is_selected: bool, spinner_tick: usize) -> ListItem<'static> {
    let status = match row.success {
        Some(true) => "[OK]".to_string(),
        Some(false) => "[X]".to_string(),
        None => format!(" {} ", spinner_frame(spinner_tick)),
    };
    let base_style = match row.success {
        Some(true) => Style::default().fg(Color::Green),
        Some(false) => Style::default().fg(Color::Red),
        None => Style::default().fg(Color::Cyan),
    };
    let line_style = if is_selected {
        base_style.add_modifier(Modifier::BOLD)
    } else {
        base_style
    };
    let status_style = line_style.add_modifier(Modifier::BOLD);
    let indicator = if is_selected { "|" } else { " " };
    let line = Line::from(vec![
        Span::styled(indicator.to_string(), line_style),
        Span::raw(" "),
        Span::styled(status, status_style),
        Span::raw(" "),
        Span::styled(row.name.clone(), line_style),
    ]);
    ListItem::new(line)
}
