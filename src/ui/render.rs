use std::io::Stdout;

use ratatui::crossterm::terminal::disable_raw_mode;
use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::ui::state::CheckRow;

pub(crate) fn draw(
    terminal: &mut ratatui::Terminal<CrosstermBackend<Stdout>>,
    rows: &[CheckRow],
    selected: usize,
    footer_msg: &str,
) {
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| list_item(row, idx == selected))
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
    let _ = terminal.clear();
    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();
    println!();
}

fn list_item(row: &CheckRow, is_selected: bool) -> ListItem<'_> {
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
    let line_style = if is_selected {
        base_style.add_modifier(Modifier::BOLD)
    } else {
        base_style
    };
    let status_style = line_style.add_modifier(Modifier::BOLD);
    let indicator = if is_selected { "|" } else { " " };
    let line = Line::from(vec![
        Span::styled(indicator, line_style),
        Span::raw(" "),
        Span::styled(status, status_style),
        Span::raw(" "),
        Span::styled(row.name.clone(), line_style.clone()),
    ]);
    ListItem::new(line)
}
