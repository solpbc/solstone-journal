// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatsTopicMigrationReport {
    pub files_changed: usize,
}

pub fn migrate_stats_topic_keys(
    journal: &Path,
    dry_run: bool,
) -> Result<StatsTopicMigrationReport, String> {
    let mut report = StatsTopicMigrationReport::default();
    migrate_dir(journal, dry_run, &mut report)?;
    Ok(report)
}
fn migrate_dir(
    path: &Path,
    dry_run: bool,
    report: &mut StatsTopicMigrationReport,
) -> Result<(), String> {
    let entries = fs::read_dir(path).map_err(|error| error.to_string())?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            migrate_dir(&path, dry_run, report)?;
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) != Some("stats.json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let Some(object) = value.as_object_mut() else {
            continue;
        };
        let mut changed = false;
        for (old, new) in [
            ("topic_data", "agent_data"),
            ("topic_counts", "agent_counts"),
            ("topic_minutes", "agent_minutes"),
            ("topic_counts_by_day", "agent_counts_by_day"),
        ] {
            if let Some(old_value) = object.remove(old) {
                object.entry(new.to_owned()).or_insert(old_value);
                changed = true;
            }
        }
        if changed {
            report.files_changed += 1;
            if !dry_run {
                atomic_replace(
                    &path,
                    &serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
                    AtomicWriteOptions { mode: Some(0o600) },
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn renames_known_stats_keys() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("chronicle/20260101/stats.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"topic_counts": {"a": 1}}"#).unwrap();
        migrate_stats_topic_keys(temp.path(), false).unwrap();
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(value.get("agent_counts").is_some());
    }
}
