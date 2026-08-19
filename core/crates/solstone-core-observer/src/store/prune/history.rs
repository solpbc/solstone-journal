// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, DirEntryKind, append_jsonl, atomic_replace, contained_path,
    list_dir_entries,
};
use solstone_core_segment::is_safe_stream_component;

use super::super::history::load_history;
use super::super::paths::{history_dir, history_path};
use super::super::reload::load_observers;
use super::types::Refusal;

pub(super) fn torn_history_refusal(day: &str, stream: &str) -> Refusal {
    Refusal::new(
        format!("{day}/{stream}"),
        "sync-history",
        Some(format!("{day}.jsonl")),
        "restore a readable history file for this day, then rerun prune",
    )
}

/// Append a `pruned` history record for `segment`, unless the latest record
/// already on file for that `(stream, segment)` is already `pruned` -- the
/// dedupe that makes an interrupted, re-run prune converge instead of
/// appending a duplicate.
pub fn append_pruned_once(
    journal: &Path,
    prefix: &str,
    day: &str,
    stream: &str,
    segment: &str,
    duplicate_of: &str,
    now_ms: i64,
) -> Result<(), Refusal> {
    let path = history_path(journal, prefix, day);
    let loaded = load_history(&path);
    if loaded.stopped.is_some() {
        return Err(torn_history_refusal(day, stream));
    }
    let latest = loaded.records.iter().rfind(|record| {
        record.get("stream").and_then(Value::as_str) == Some(stream)
            && record.get("segment").and_then(Value::as_str) == Some(segment)
    });
    if latest
        .and_then(|record| record.get("type"))
        .and_then(Value::as_str)
        == Some("pruned")
    {
        return Ok(());
    }
    let record = json!({
        "type": "pruned",
        "ts": now_ms,
        "segment": segment,
        "stream": stream,
        "duplicate_of": duplicate_of,
    });
    let _ = append_jsonl(&path, &record);
    Ok(())
}

/// Every `pruned` history record for `stream`, across every observer
/// (including revoked ones -- a revoked observer's prior deletions still
/// justify chain repair), keyed by `(day, segment)`.
pub fn pruned_records_by_stream(
    journal: &Path,
    stream: &str,
) -> Result<BTreeMap<(String, String), Value>, Refusal> {
    let mut records_by_segment = BTreeMap::new();
    for observer in load_observers(journal).unwrap_or_default() {
        let prefix = observer.prefix();
        let hist_dir = history_dir(journal, &prefix);
        let Ok(entries) = list_dir_entries(&hist_dir) else {
            continue;
        };
        for entry in entries {
            if entry.kind != DirEntryKind::File {
                continue;
            }
            let Some(day) = entry
                .name
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let loaded = load_history(&hist_dir.join(format!("{day}.jsonl")));
            if loaded.stopped.is_some() {
                return Err(torn_history_refusal(day, stream));
            }
            for record in loaded.records {
                if record.get("type").and_then(Value::as_str) != Some("pruned") {
                    continue;
                }
                if record.get("stream").and_then(Value::as_str) != Some(stream) {
                    continue;
                }
                let Some(segment) = record.get("segment").and_then(Value::as_str) else {
                    continue;
                };
                records_by_segment.insert((day.to_owned(), segment.to_owned()), record);
            }
        }
    }
    Ok(records_by_segment)
}

/// One history file this prune could not rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPruneFailure {
    pub entry: String,
    pub reason: String,
    pub torn: bool,
}

/// Rows dropped from observer history, and files that could not be rewritten.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryPruneReport {
    pub removed: usize,
    pub failures: Vec<HistoryPruneFailure>,
}

fn history_entry(prefix: &str, day: &str) -> String {
    format!("apps/observer/observers/{prefix}/hist/{day}.jsonl")
}

/// True when any readable history file still has a row whose `stream` field
/// equals `stream`.
///
/// Torn files are skipped: their unread tail is not evidence. `observed` /
/// `transferred` rows carry no `stream` field and do not satisfy this check.
pub fn has_history_for_stream(journal: &Path, stream: &str) -> bool {
    for observer in load_observers(journal).unwrap_or_default() {
        let prefix = observer.prefix();
        let hist_dir = history_dir(journal, &prefix);
        let Ok(entries) = list_dir_entries(&hist_dir) else {
            continue;
        };
        for entry in entries {
            if entry.kind != DirEntryKind::File {
                continue;
            }
            let Some(day) = entry
                .name
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let loaded = load_history(&hist_dir.join(format!("{day}.jsonl")));
            if loaded.stopped.is_some() {
                continue;
            }
            if loaded
                .records
                .iter()
                .any(|record| record.get("stream").and_then(Value::as_str) == Some(stream))
            {
                return true;
            }
        }
    }
    false
}

/// Drop JSONL rows whose `stream` field equals `stream`.
///
/// ⚠ `observed` / `transferred` rows have no `stream` field and must stay —
/// deleting by `segment` name would over-delete ingest records that are not
/// location-source rows. Torn files are skipped and not rewritten: the
/// tolerant reader stops at the first malformed line, and rewriting the
/// parsed prefix would drop the unread tail.
pub fn remove_history_rows_for_stream(journal: &Path, stream: &str) -> HistoryPruneReport {
    let mut report = HistoryPruneReport::default();
    for observer in load_observers(journal).unwrap_or_default() {
        let prefix = observer.prefix();
        if !is_safe_stream_component(&prefix) {
            report.failures.push(HistoryPruneFailure {
                entry: format!("apps/observer/observers/{prefix}"),
                reason: "this observer's history path is not a safe journal location \
                         and was left unchanged"
                    .to_owned(),
                torn: false,
            });
            continue;
        }
        let hist_dir = history_dir(journal, &prefix);
        let Ok(entries) = list_dir_entries(&hist_dir) else {
            continue;
        };
        for entry in entries {
            if entry.kind != DirEntryKind::File {
                continue;
            }
            let Some(day) = entry
                .name
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let rel = history_entry(&prefix, day);
            let path = match contained_path(journal, &rel) {
                Ok(path) => path,
                Err(_) => {
                    report.failures.push(HistoryPruneFailure {
                        entry: rel,
                        reason: "this observer's history path is not a safe journal location \
                                 and was left unchanged"
                            .to_owned(),
                        torn: false,
                    });
                    continue;
                }
            };
            let loaded = load_history(&path);
            if loaded.stopped.is_some() {
                report.failures.push(HistoryPruneFailure {
                    entry: history_entry(&prefix, day),
                    reason: "this history file is unreadable and was left unchanged".to_owned(),
                    torn: true,
                });
                continue;
            }
            let kept: Vec<Value> = loaded
                .records
                .iter()
                .filter(|record| record.get("stream").and_then(Value::as_str) != Some(stream))
                .cloned()
                .collect();
            let dropped = loaded.records.len().saturating_sub(kept.len());
            if dropped == 0 {
                continue;
            }
            let mut bytes = Vec::new();
            for record in &kept {
                match serde_json::to_vec(record) {
                    Ok(mut line) => {
                        line.push(b'\n');
                        bytes.extend_from_slice(&line);
                    }
                    Err(_) => {
                        report.failures.push(HistoryPruneFailure {
                            entry: history_entry(&prefix, day),
                            reason: "the observer history file could not be updated".to_owned(),
                            torn: false,
                        });
                        bytes.clear();
                        break;
                    }
                }
            }
            if bytes.is_empty() && !kept.is_empty() {
                continue;
            }
            if atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }).is_err() {
                report.failures.push(HistoryPruneFailure {
                    entry: history_entry(&prefix, day),
                    reason: "the observer history file could not be updated".to_owned(),
                    torn: false,
                });
                continue;
            }
            report.removed = report.removed.saturating_add(dropped);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::record::ObserverRecord;
    use crate::store::write::save_observer;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;
    use std::fs;

    fn root(name: &str) -> std::path::PathBuf {
        reserve_temp_path(&format!("observer-prune-history-{name}"))
    }

    fn torn_contents() -> String {
        "{\"segment\":\"090000_300\",\"type\":\"pruned\",\"stream\":\"workstation\"}\n{broken}\n{\"segment\":\"after\",\"type\":\"pruned\",\"stream\":\"workstation\"}\n".to_owned()
    }

    #[test]
    fn rerun_after_prune_does_not_append_a_duplicate() {
        let root = root("dedupe");
        append_pruned_once(
            &root,
            "abcdefgh",
            "20260101",
            "workstation",
            "090000_301",
            "090000_300",
            1,
        )
        .unwrap();
        let first = load_history(&history_path(&root, "abcdefgh", "20260101")).records;
        assert_eq!(first.len(), 1);
        append_pruned_once(
            &root,
            "abcdefgh",
            "20260101",
            "workstation",
            "090000_301",
            "090000_300",
            2,
        )
        .unwrap();
        let second = load_history(&history_path(&root, "abcdefgh", "20260101")).records;
        assert_eq!(second, first);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn append_pruned_once_refuses_torn_file() {
        let root = root("append-torn");
        let path = history_path(&root, "abcdefgh", "20260101");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let before = torn_contents();
        fs::write(&path, &before).unwrap();
        let error = append_pruned_once(
            &root,
            "abcdefgh",
            "20260101",
            "workstation",
            "090000_301",
            "090000_300",
            1,
        )
        .unwrap_err();
        assert_eq!(error.gate, "sync-history");
        assert_eq!(error.subject, "20260101/workstation");
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pruned_records_by_stream_refuses_torn_day() {
        let root = root("collector-torn");
        let record = ObserverRecord::from_value(json!({"key":"abcdefghx","name":"one"})).unwrap();
        save_observer(&root, &record).unwrap();
        let path = history_path(&root, "abcdefgh", "20260101");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, torn_contents()).unwrap();
        let error = pruned_records_by_stream(&root, "workstation").unwrap_err();
        assert_eq!(error.gate, "sync-history");
        assert_eq!(error.subject, "20260101/workstation");

        fs::write(
            &path,
            "{\"segment\":\"090000_300\",\"type\":\"pruned\",\"stream\":\"workstation\"}\n",
        )
        .unwrap();
        let clean = pruned_records_by_stream(&root, "workstation").unwrap();
        assert_eq!(clean.len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn remove_history_rows_keeps_observed_and_skips_torn() {
        let root = root("remove-stream");
        let record = ObserverRecord::from_value(json!({"key":"abcdefghx","name":"one"})).unwrap();
        save_observer(&root, &record).unwrap();
        let path = history_path(&root, "abcdefgh", "20260101");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"type\":\"observed\",\"ts\":1,\"segment\":\"090000_300\"}\n",
                "{\"type\":\"pruned\",\"ts\":2,\"segment\":\"090000_301\",\"stream\":\"location\",\"duplicate_of\":\"090000_300\"}\n",
                "{\"type\":\"transferred\",\"ts\":3,\"segment\":\"090000_302\"}\n",
            ),
        )
        .unwrap();
        let report = remove_history_rows_for_stream(&root, "location");
        assert_eq!(report.removed, 1);
        assert!(report.failures.is_empty());
        let kept = load_history(&path).records;
        assert_eq!(kept.len(), 2);
        assert_eq!(
            kept.iter()
                .map(|row| row["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["observed", "transferred"]
        );
        assert!(!has_history_for_stream(&root, "location"));

        let torn = history_path(&root, "abcdefgh", "20260102");
        fs::write(
            &torn,
            "{\"stream\":\"location\"}\n{broken}\n{\"stream\":\"location\"}\n",
        )
        .unwrap();
        let report = remove_history_rows_for_stream(&root, "location");
        assert_eq!(report.removed, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures.iter().all(|failure| failure.torn));
        assert_eq!(
            fs::read_to_string(&torn).unwrap(),
            "{\"stream\":\"location\"}\n{broken}\n{\"stream\":\"location\"}\n"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn remove_history_skips_unsafe_prefix() {
        let root = root("unsafe-prefix");
        let record = ObserverRecord::from_value(json!({"key":"ABCDEFGHx","name":"one"})).unwrap();
        save_observer(&root, &record).unwrap();
        let path = history_path(&root, "ABCDEFGH", "20260101");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"type\":\"pruned\",\"stream\":\"location\",\"segment\":\"a\"}\n",
        )
        .unwrap();
        let before = fs::read(&path).unwrap();
        let report = remove_history_rows_for_stream(&root, "location");
        assert_eq!(report.removed, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(!report.failures[0].torn);
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(&root).ok();
    }
}
