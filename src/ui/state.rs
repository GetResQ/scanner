use std::collections::VecDeque;

use crate::pool::PoolStats;
use crate::ui::events::StreamType;
use crate::ui::sanitize_text_for_tui;

/// Maximum number of stream lines to keep in buffer.
const MAX_STREAM_LINES: usize = 200;

#[derive(Debug, Clone)]
pub struct CheckRow {
    pub name: String,
    pub status: String,
    pub success: Option<bool>,
    pub desc: Option<String>,
    pub output: Option<String>,
}

impl CheckRow {
    pub fn new(name: String, desc: Option<String>) -> Self {
        Self {
            name,
            status: "running".into(),
            success: None,
            desc,
            output: Some("running".into()),
        }
    }
}

/// A single line of streamed output.
#[derive(Debug, Clone)]
pub struct StreamLine {
    pub source: String,
    pub stream: StreamType,
    pub line: String,
}

/// Application state for the TUI.
#[derive(Debug)]
pub struct AppState {
    pub rows: Vec<CheckRow>,
    pub selected: usize,
    pub pool_stats: Option<PoolStats>,
    pub stream_buffer: VecDeque<StreamLine>,
    pub finished: bool,
    pub exit_requested: bool,
    pub spinner_tick: usize,
    spinner_counter: usize,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            pool_stats: None,
            stream_buffer: VecDeque::with_capacity(MAX_STREAM_LINES),
            finished: false,
            exit_requested: false,
            spinner_tick: 0,
            spinner_counter: 0,
        }
    }

    pub fn add_stream_line(&mut self, source: String, stream: StreamType, line: String) {
        if self.stream_buffer.len() >= MAX_STREAM_LINES {
            self.stream_buffer.pop_front();
        }
        let line = sanitize_text_for_tui(&line);
        self.stream_buffer.push_back(StreamLine {
            source,
            stream,
            line,
        });
    }

    /// Advance spinner animation. Only changes frame every 3 ticks (~150ms at 50ms poll rate).
    pub fn tick_spinner(&mut self) {
        self.spinner_counter = self.spinner_counter.wrapping_add(1);
        if self.spinner_counter.is_multiple_of(3) {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
