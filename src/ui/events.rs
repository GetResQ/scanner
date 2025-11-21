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
