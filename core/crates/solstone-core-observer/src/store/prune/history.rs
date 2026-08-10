// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_journal_io::{DirEntryKind, append_jsonl, list_dir_entries};

use super::super::history::load_history;
use super::super::paths::{history_dir, history_path};
use super::super::reload::load_observers;

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
) {
    let path = history_path(journal, prefix, day);
    let records = load_history(&path).records;
    let latest = records
        .iter()
        .filter(|record| {
            record.get("stream").and_then(Value::as_str) == Some(stream)
                && record.get("segment").and_then(Value::as_str) == Some(segment)
        })
        .next_back();
    if latest
        .and_then(|record| record.get("type"))
        .and_then(Value::as_str)
        == Some("pruned")
    {
        return;
    }
    let record = json!({
        "type": "pruned",
        "ts": now_ms,
        "segment": segment,
        "stream": stream,
        "duplicate_of": duplicate_of,
    });
    let _ = append_jsonl(&path, &record);
}

/// Every `pruned` history record for `stream`, across every observer
/// (including revoked ones -- a revoked observer's prior deletions still
/// justify chain repair), keyed by `(day, segment)`.
pub fn pruned_records_by_stream(journal: &Path, stream: &str) -> BTreeMap<(String, String), Value> {
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
            for record in load_history(&hist_dir.join(format!("{day}.jsonl"))).records {
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
    records_by_segment
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "observer-prune-history-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
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
        );
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
        );
        let second = load_history(&history_path(&root, "abcdefgh", "20260101")).records;
        assert_eq!(second, first);
        fs::remove_dir_all(&root).ok();
    }
}
