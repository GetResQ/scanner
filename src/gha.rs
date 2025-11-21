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
    /// Whether this annotation represents an actionable tool-reported issue that
    /// should be sent to the fixer agent. Synthetic annotations (e.g. "no GitHub
    /// Actions annotations produced") should set this to false.
    pub actionable: bool,
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
        actionable: true,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_annotation() {
        let line = "::error file=app.js,line=10::Missing semicolon";
        let ann = parse_annotation_line(line).unwrap();
        assert_eq!(ann.level, AnnotationLevel::Error);
        assert!(ann.actionable);
        assert_eq!(ann.file, Some(PathBuf::from("app.js")));
        assert_eq!(ann.line, Some(10));
        assert_eq!(ann.message, "Missing semicolon");
    }

    #[test]
    fn parse_warning_annotation() {
        let line = "::warning file=src/lib.rs,line=42,col=5::Unused variable";
        let ann = parse_annotation_line(line).unwrap();
        assert_eq!(ann.level, AnnotationLevel::Warning);
        assert!(ann.actionable);
        assert_eq!(ann.file, Some(PathBuf::from("src/lib.rs")));
        assert_eq!(ann.line, Some(42));
        assert_eq!(ann.column, Some(5));
        assert_eq!(ann.message, "Unused variable");
    }

    #[test]
    fn parse_notice_annotation() {
        let line = "::notice::Build completed";
        let ann = parse_annotation_line(line).unwrap();
        assert_eq!(ann.level, AnnotationLevel::Notice);
        assert!(ann.actionable);
        assert_eq!(ann.file, None);
        assert_eq!(ann.message, "Build completed");
    }

    #[test]
    fn parse_annotation_with_title() {
        let line = "::error file=test.rs,line=1,title=E0001::Type mismatch";
        let ann = parse_annotation_line(line).unwrap();
        assert_eq!(ann.title, Some("E0001".to_string()));
        assert_eq!(ann.message, "Type mismatch");
    }

    #[test]
    fn parse_annotation_with_end_line() {
        let line = "::error file=test.rs,line=10,endLine=15::Multi-line error";
        let ann = parse_annotation_line(line).unwrap();
        assert_eq!(ann.line, Some(10));
        assert_eq!(ann.end_line, Some(15));
    }

    #[test]
    fn parse_ignores_non_annotation_lines() {
        assert!(parse_annotation_line("regular output").is_none());
        assert!(parse_annotation_line(":: not valid").is_none());
        assert!(parse_annotation_line("::unknown::message").is_none());
    }

    #[test]
    fn parse_case_insensitive_level() {
        assert!(parse_annotation_line("::ERROR::msg").is_some());
        assert!(parse_annotation_line("::Warning::msg").is_some());
        assert!(parse_annotation_line("::NOTICE::msg").is_some());
    }

    #[test]
    fn parse_annotations_multiple_lines() {
        let output = r#"
Building project...
::error file=a.rs,line=1::Error one
Some other output
::warning file=b.rs,line=2::Warning one
Done.
"#;
        let anns = parse_annotations(output);
        assert_eq!(anns.len(), 2);
        assert_eq!(anns[0].level, AnnotationLevel::Error);
        assert_eq!(anns[1].level, AnnotationLevel::Warning);
    }

    #[test]
    fn is_error_level_works() {
        assert!(is_error_level(AnnotationLevel::Error));
        assert!(!is_error_level(AnnotationLevel::Warning));
        assert!(!is_error_level(AnnotationLevel::Notice));
    }
}
