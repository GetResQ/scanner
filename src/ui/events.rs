use crate::pool::PoolStats;

/// Type of output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    /// A check/task has started running.
    CheckStarted { name: String, desc: Option<String> },
    /// A check/task has finished.
    CheckFinished {
        name: String,
        success: bool,
        message: String,
        output: Option<String>,
    },
    /// Pool statistics update.
    PoolStats(PoolStats),
    /// A line of output from a running process.
    StreamLine {
        source: String,
        stream: StreamType,
        line: String,
    },
    /// All work is done.
    Done,
}
