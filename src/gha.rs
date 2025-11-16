use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnnotationLevel {
    Error,
    Warning,
    Notice,
}

#[derive(Debug, Clone)]
pub struct Annotation {
    pub level: AnnotationLevel,
    pub file: Option<PathBuf>,
    pub line: Option<u64>,
    pub end_line: Option<u64>,
    pub column: Option<u64>,
    pub end_column: Option<u64>,
    pub title: Option<String>,
    pub message: String,
}

/// Parse a single GitHub Actions annotation line, e.g.:
/// `::error file=app.js,line=1::Missing semicolon`
pub fn parse_annotation_line(line: &str) -> Option<Annotation> {
    if !line.starts_with("::") {
        return None;
    }

    let rest = &line[2..];
    let mut parts = rest.splitn(2, "::");
    let head = parts.next()?.trim();
    let message = parts.next().unwrap_or("").to_string();

    let mut head_parts = head.splitn(2, ' ');
    let command = head_parts.next()?.trim();
    let params_str = head_parts.next().unwrap_or("").trim();

    let level = match command.to_ascii_lowercase().as_str() {
        "error" => AnnotationLevel::Error,
        "warning" => AnnotationLevel::Warning,
        "notice" => AnnotationLevel::Notice,
        _ => return None,
    };

    let mut file = None;
    let mut line_no = None;
    let mut end_line = None;
    let mut column = None;
    let mut end_column = None;
    let mut title = None;

    if !params_str.is_empty() {
        for pair in params_str.split(',') {
            let (key, value) = match pair.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "file" => file = Some(PathBuf::from(value)),
                "line" => line_no = value.parse().ok(),
                "endLine" => end_line = value.parse().ok(),
                "col" | "column" => column = value.parse().ok(),
                "endColumn" => end_column = value.parse().ok(),
                "title" => title = Some(value.to_string()),
                _ => {}
            }
        }
    }

    Some(Annotation {
        level,
        file,
        line: line_no,
        end_line,
        column,
        end_column,
        title,
        message,
    })
}

/// Parse all annotations from the given output text.
pub fn parse_annotations(output: &str) -> Vec<Annotation> {
    output
        .lines()
        .filter_map(|line| parse_annotation_line(line.trim_end()))
        .collect()
}

pub fn is_error_level(level: AnnotationLevel) -> bool {
    matches!(level, AnnotationLevel::Error)
}
