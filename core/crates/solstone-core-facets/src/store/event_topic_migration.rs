// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{AtomicWriteOptions, write_text};

use super::error::FacetWriteError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventTopicMigrationReport {
    pub files_changed: usize,
    pub records_changed: usize,
    pub errors: usize,
}

pub fn migrate_event_topic_keys(
    journal_root: &Path,
    dry_run: bool,
) -> Result<EventTopicMigrationReport, FacetWriteError> {
    let mut report = EventTopicMigrationReport::default();
    let root = journal_root.join("facets");
    let Ok(facets) = fs::read_dir(root) else {
        return Ok(report);
    };
    for facet in facets.flatten() {
        let events = facet.path().join("events.jsonl");
        let Ok(text) = fs::read_to_string(&events) else {
            continue;
        };
        let mut changed = false;
        let mut rows = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(mut value) = serde_json::from_str::<Value>(line) else {
                rows.push(line.to_owned());
                continue;
            };
            if let Some(object) = value.as_object_mut()
                && let Some(topic) = object.remove("topic")
            {
                object.entry("agent").or_insert(topic);
                changed = true;
                report.records_changed += 1;
            }
            rows.push(serde_json::to_string(&value).expect("JSON value"));
        }
        if changed {
            report.files_changed += 1;
            if !dry_run {
                write_text(
                    &events,
                    &format!("{}\n", rows.join("\n")),
                    AtomicWriteOptions { mode: Some(0o600) },
                )
                .map_err(FacetWriteError::ContentWrite)?;
            }
        }
    }
    Ok(report)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn migrates_topic_without_overwriting_agent() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("facets/work/events.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"topic\":\"old\"}\n{\"topic\":\"old\",\"agent\":\"new\"}\n",
        )
        .unwrap();
        let report = migrate_event_topic_keys(temp.path(), false).unwrap();
        assert_eq!(report.records_changed, 2);
        let written = fs::read_to_string(path).unwrap();
        assert!(written.contains("\"agent\":\"old\""));
        assert!(!written.contains("topic"));
    }
}
