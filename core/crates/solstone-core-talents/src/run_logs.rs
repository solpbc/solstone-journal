// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use crate::layout::TalentStorageError;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TalentRunLogMigrationReport {
    pub moved: usize,
    pub day_index_entries: usize,
    pub skipped: usize,
    pub errors: usize,
}
pub fn migrate_agent_run_logs(
    journal: &Path,
    dry_run: bool,
) -> Result<TalentRunLogMigrationReport, TalentStorageError> {
    let agents = journal.join("agents");
    let mut report = TalentRunLogMigrationReport::default();
    if !agents.exists() {
        return Ok(report);
    }
    let mut indexes = BTreeMap::<String, Vec<Value>>::new();
    for path in sorted(&agents)? {
        if symlink(&path)
            || !path.is_file()
            || path.extension().and_then(|v| v.to_str()) != Some("jsonl")
        {
            continue;
        }
        let stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or("");
        if stem.len() == 8 && stem.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let active = stem.ends_with("_active");
        let id = stem.trim_end_matches("_active");
        let Some(first) = first_json(&path) else {
            report.skipped += 1;
            continue;
        };
        let name = first
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("talent");
        let safe = name.replace(':', "--");
        let dest = agents.join(&safe).join(path.file_name().unwrap());
        if dest.exists() {
            report.skipped += 1;
            continue;
        }
        if !dry_run {
            fs::create_dir_all(dest.parent().unwrap()).map_err(io)?;
            fs::rename(&path, &dest).map_err(io)?;
        }
        report.moved += 1;
        if !active && id.parse::<u64>().is_ok() {
            let day = first
                .get("day")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| day_from_id(id));
            if !day.is_empty() {
                let mut entry = Map::new();
                for key in ["facet", "ts", "provider", "model"] {
                    entry.insert(
                        key.to_owned(),
                        first.get(key).cloned().unwrap_or(Value::Null),
                    );
                }
                let start_ts = first.get("ts").and_then(Value::as_i64);
                let event_source = if dry_run {
                    path.as_path()
                } else {
                    dest.as_path()
                };
                let end_event = last_event(event_source, &["finish", "error"]);
                let mut status = "completed";
                let mut runtime_seconds = Value::Null;
                if let Some(end) = &end_event {
                    if end.get("event").and_then(Value::as_str) == Some("error") {
                        status = "error";
                    }
                    if let (Some(end_ts), Some(start_ts)) =
                        (end.get("ts").and_then(Value::as_i64), start_ts)
                        && end_ts != 0
                        && start_ts != 0
                    {
                        let seconds = ((end_ts - start_ts) as f64 / 1000.0 * 10.0).round() / 10.0;
                        runtime_seconds =
                            serde_json::Number::from_f64(seconds).map_or(Value::Null, Value::from);
                    }
                }
                entry.insert("agent_id".to_owned(), Value::String(id.to_owned()));
                entry.insert("name".to_owned(), Value::String(name.to_owned()));
                entry.insert("day".to_owned(), Value::String(day.clone()));
                entry.insert("status".to_owned(), Value::String(status.to_owned()));
                entry.insert("runtime_seconds".to_owned(), runtime_seconds);
                indexes.entry(day).or_default().push(Value::Object(entry));
                report.day_index_entries += 1;
            }
        }
    }
    for (day, entries) in indexes {
        if dry_run {
            continue;
        }
        let index = agents.join(format!("{day}.jsonl"));
        let existing = existing_ids(&index);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index)
            .map_err(io)?;
        use std::io::Write;
        for entry in entries {
            if !existing.contains(entry.get("agent_id").and_then(Value::as_str).unwrap_or("")) {
                writeln!(
                    file,
                    "{}",
                    serde_json::to_string(&entry).map_err(|e| TalentStorageError(e.to_string()))?
                )
                .map_err(io)?;
            }
        }
    }
    Ok(report)
}
fn sorted(path: &Path) -> Result<Vec<std::path::PathBuf>, TalentStorageError> {
    let mut paths = fs::read_dir(path)
        .map_err(io)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(io)?;
    paths.sort();
    Ok(paths)
}
fn first_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(text.lines().next()?.trim()).ok()
}
/// Return the last matching `event` among the file's final ten lines,
/// scanning most-recent-first (mirrors Python's `_read_last_event`).
fn last_event(path: &Path, event_types: &[&str]) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    let lines = text.lines().collect::<Vec<_>>();
    let tail_start = lines.len().saturating_sub(10);
    lines[tail_start..].iter().rev().find_map(|line| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let value = serde_json::from_str::<Value>(line).ok()?;
        let matches = value
            .get("event")
            .and_then(Value::as_str)
            .is_some_and(|event| event_types.contains(&event));
        matches.then_some(value)
    })
}
fn day_from_id(id: &str) -> String {
    let Ok(ms) = id.parse::<i64>() else {
        return String::new();
    };
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y%m%d").to_string())
        .unwrap_or_default()
}
fn existing_ids(path: &Path) -> HashSet<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return HashSet::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            value
                .get("agent_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}
#[cfg(unix)]
fn symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn symlink(_: &Path) -> bool {
    false
}
fn io(error: std::io::Error) -> TalentStorageError {
    TalentStorageError(error.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn moves_log_and_creates_day_index() {
        let temp = tempdir().unwrap();
        let agents = temp.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("1700000000000.jsonl"),
            "{\"name\":\"chat\",\"ts\":1700000000000}\n{\"event\":\"finish\"}\n",
        )
        .unwrap();
        let report = migrate_agent_run_logs(temp.path(), false).unwrap();
        assert_eq!(report.moved, 1);
        assert_eq!(report.day_index_entries, 1);
        assert!(agents.join("chat/1700000000000.jsonl").exists());
        assert!(!agents.join("chat.log").exists());
    }
    #[test]
    fn day_index_reports_error_status_and_runtime_from_the_tail_event() {
        let temp = tempdir().unwrap();
        let agents = temp.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("1700000000000.jsonl"),
            "{\"name\":\"chat\",\"ts\":1700000000000,\"day\":\"20231114\"}\n\
             {\"event\":\"error\",\"ts\":1700000005500}\n",
        )
        .unwrap();
        migrate_agent_run_logs(temp.path(), false).unwrap();
        let index = fs::read_to_string(agents.join("20231114.jsonl")).unwrap();
        let entry: Value = serde_json::from_str(index.lines().next().unwrap()).unwrap();
        assert_eq!(entry["status"], "error");
        assert_eq!(entry["runtime_seconds"], 5.5);
        assert_eq!(entry["facet"], Value::Null);
    }
}
