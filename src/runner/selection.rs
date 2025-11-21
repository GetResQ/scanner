use std::collections::HashSet;

use crate::config::{Check, Config};

pub(crate) fn select_checks<'a>(config: &'a Config, filters: &[String], force: bool) -> Vec<Check> {
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
