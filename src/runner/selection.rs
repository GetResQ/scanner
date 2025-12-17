use std::collections::HashSet;

use crate::config::{Check, Config};

pub(crate) fn select_checks(config: &Config, filters: &[String], force: bool) -> Vec<Check> {
    if filters.is_empty() {
        return config
            .checks
            .iter()
            .filter(|c| c.enabled)
            .cloned()
            .collect();
    }

    let filter_set: HashSet<String> = filters.iter().map(|s| s.to_ascii_lowercase()).collect();

    config
        .checks
        .iter()
        .filter(|check| {
            let name_match = filter_set.contains(&check.name.to_ascii_lowercase());
            let tag_match = check
                .tags
                .iter()
                .any(|t| filter_set.contains(&t.to_ascii_lowercase()));

            // Force only applies to explicit name matches; tag matches still honor enabled.
            (name_match && (check.enabled || force)) || (tag_match && check.enabled)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CommandSpec;
    use std::collections::HashMap;

    fn make_check(name: &str, enabled: bool, tags: Vec<&str>) -> Check {
        Check {
            name: name.to_string(),
            command: CommandSpec {
                program: "echo".to_string(),
                args: vec![],
            },
            formatter: None,
            fixer: None,
            env: HashMap::new(),
            timeout: None,
            enabled,
            tags: tags.into_iter().map(String::from).collect(),
            description: None,
            cwd: None,
            lock: None,
        }
    }

    fn make_config(checks: Vec<Check>) -> Config {
        Config {
            setup: Vec::new(),
            checks,
            agents: Default::default(),
        }
    }

    #[test]
    fn empty_filters_returns_enabled_checks() {
        let config = make_config(vec![
            make_check("lint", true, vec![]),
            make_check("test", true, vec![]),
            make_check("disabled", false, vec![]),
        ]);

        let selected = select_checks(&config, &[], false);
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().any(|c| c.name == "lint"));
        assert!(selected.iter().any(|c| c.name == "test"));
    }

    #[test]
    fn filter_by_name() {
        let config = make_config(vec![
            make_check("lint", true, vec![]),
            make_check("test", true, vec![]),
        ]);

        let selected = select_checks(&config, &["lint".to_string()], false);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "lint");
    }

    #[test]
    fn filter_by_tag() {
        let config = make_config(vec![
            make_check("lint", true, vec!["rust"]),
            make_check("test", true, vec!["rust", "unit"]),
            make_check("fmt", true, vec!["format"]),
        ]);

        let selected = select_checks(&config, &["rust".to_string()], false);
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn filter_is_case_insensitive() {
        let config = make_config(vec![make_check("Lint", true, vec!["Rust"])]);

        let selected = select_checks(&config, &["lint".to_string()], false);
        assert_eq!(selected.len(), 1);

        let selected = select_checks(&config, &["RUST".to_string()], false);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn force_includes_disabled_by_name() {
        let config = make_config(vec![make_check("disabled", false, vec!["slow"])]);

        // Without force - not selected
        let selected = select_checks(&config, &["disabled".to_string()], false);
        assert!(selected.is_empty());

        // With force - selected by name
        let selected = select_checks(&config, &["disabled".to_string()], true);
        assert_eq!(selected.len(), 1);

        // Force does NOT apply to tag matches
        let selected = select_checks(&config, &["slow".to_string()], true);
        assert!(selected.is_empty());
    }
}
