// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_journal_io::{DirEntryKind, append_jsonl, list_dir_entries};

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
}
