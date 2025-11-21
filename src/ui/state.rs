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
            status: "running…".into(),
            success: None,
            desc,
            output: Some("running…".into()),
        }
    }
}
