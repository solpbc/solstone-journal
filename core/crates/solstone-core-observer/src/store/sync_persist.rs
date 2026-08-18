// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::append_jsonl;

use super::history::load_history;
use super::paths::history_path;
use super::reload::find_observer;
use super::write::save_observer;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SyncEventKind {
    Observed,
    Transferred,
}

impl SyncEventKind {
    fn type_name(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Transferred => "transferred",
        }
    }

    fn stat_key(self) -> &'static str {
        match self {
            Self::Observed => "segments_observed",
            Self::Transferred => "segments_transferred",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SyncPersistResult {
    Skipped,
    Applied,
    HistoryWrittenStatsFailed,
    TornHistory,
    HistoryWriteFailed,
}

pub fn persist_sync(
    journal_root: &Path,
    device_name: &str,
    day: &str,
    segment: &str,
    kind: SyncEventKind,
    now_ms: i64,
) -> SyncPersistResult {
    if device_name.is_empty() || day.is_empty() || segment.is_empty() {
        return SyncPersistResult::Skipped;
    }
    let mut record = match find_observer(journal_root, device_name) {
        Ok(Some(record)) => record,
        Ok(None) | Err(_) => return SyncPersistResult::Skipped,
    };
    let path = history_path(journal_root, &record.prefix(), day);
    let loaded = load_history(&path);
    if loaded.stopped.is_some() {
        return SyncPersistResult::TornHistory;
    }
    let row = json!({
        "type": kind.type_name(),
        "ts": now_ms,
        "segment": segment,
    });
    if append_jsonl(&path, &row).is_err() {
        return SyncPersistResult::HistoryWriteFailed;
    }
    let stats = increment_sync_stat(record.stats(), kind.stat_key());
    record.set_stats(stats);
    match save_observer(journal_root, &record) {
        Ok(()) => SyncPersistResult::Applied,
        Err(_) => SyncPersistResult::HistoryWrittenStatsFailed,
    }
}

fn increment_sync_stat(stats: Option<&Map<String, Value>>, key: &str) -> Map<String, Value> {
    let mut map = stats.cloned().unwrap_or_default();
    let next = match map.get(key) {
        Some(Value::Number(number)) => number
            .as_i64()
            .map(|value| Value::from(value.saturating_add(1)))
            .or_else(|| {
                number
                    .as_u64()
                    .map(|value| Value::from(value.saturating_add(1)))
            })
            .or_else(|| number.as_f64().map(|value| Value::from(value + 1.0)))
            .unwrap_or(Value::from(1)),
        _ => Value::from(1),
    };
    map.insert(key.to_owned(), next);
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::paths::{history_dir, history_path, observer_path};
    use crate::store::record::ObserverRecord;
    use crate::store::write::save_observer;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    const NOW: i64 = 1_767_236_400_000;
    const DAY: &str = "20260101";
    const SEGMENT: &str = "120000_1";

    fn root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("observer-sync-persist-{name}"))
    }

    fn seed(root: &Path, extra_stats: Value) -> ObserverRecord {
        let mut value = json!({
            "key": "abcdefghx",
            "name": "desk",
            "platform": "linux",
            "label": "keep",
            "stats": extra_stats,
        });
        if extra_stats.is_null() {
            value.as_object_mut().expect("object").remove("stats");
        }
        let record = ObserverRecord::from_value(value).expect("record");
        save_observer(root, &record).expect("save");
        record
    }

    fn read_record(root: &Path) -> Value {
        serde_json::from_slice(&fs::read(observer_path(root, "abcdefgh")).expect("read"))
            .expect("JSON")
    }

    fn hist_path(root: &Path) -> PathBuf {
        history_path(root, "abcdefgh", DAY)
    }

    fn restore_mode(path: &Path, mode: u32) {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }

    #[test]
    fn observed_appends_row_and_increments_only_matching_stat() {
        let root = root("ac1");
        seed(
            &root,
            json!({"bytes_received": 10, "note": "keep", "nested": {"x": 1}}),
        );
        fs::create_dir_all(history_dir(&root, "abcdefgh")).expect("hist dir");
        let before = read_record(&root);

        assert_eq!(
            persist_sync(&root, "desk", DAY, SEGMENT, SyncEventKind::Observed, NOW,),
            SyncPersistResult::Applied
        );

        let lines: Vec<String> = fs::read_to_string(hist_path(&root))
            .expect("hist")
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines.len(), 1);
        let row: Value = serde_json::from_str(&lines[0]).expect("row");
        assert_eq!(row["type"], "observed");
        assert_eq!(row["ts"], NOW);
        assert_eq!(row["segment"], SEGMENT);

        let after = read_record(&root);
        assert_eq!(after["stats"]["segments_observed"], 1);
        let mut before_rest = before.clone();
        let mut after_rest = after.clone();
        before_rest
            .as_object_mut()
            .expect("object")
            .get_mut("stats")
            .and_then(Value::as_object_mut)
            .expect("stats")
            .remove("segments_observed");
        after_rest
            .as_object_mut()
            .expect("object")
            .get_mut("stats")
            .and_then(Value::as_object_mut)
            .expect("stats")
            .remove("segments_observed");
        assert_eq!(before_rest, after_rest);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn second_observed_appends_without_rewriting_first_line() {
        let root = root("ac2");
        seed(&root, json!({}));
        assert_eq!(
            persist_sync(&root, "desk", DAY, SEGMENT, SyncEventKind::Observed, NOW,),
            SyncPersistResult::Applied
        );
        let first = fs::read_to_string(hist_path(&root)).expect("hist");
        let first_line = first.lines().next().expect("line").to_owned();

        assert_eq!(
            persist_sync(
                &root,
                "desk",
                DAY,
                "120000_2",
                SyncEventKind::Observed,
                NOW + 1,
            ),
            SyncPersistResult::Applied
        );
        let second = fs::read_to_string(hist_path(&root)).expect("hist");
        let lines: Vec<&str> = second.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], first_line);
        let row: Value = serde_json::from_str(lines[1]).expect("row");
        assert_eq!(row["type"], "observed");
        assert_eq!(row["segment"], "120000_2");
        assert_eq!(read_record(&root)["stats"]["segments_observed"], 2);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn empty_device_name_skips_without_writes() {
        let root = root("ac3");
        seed(&root, json!({"bytes_received": 4}));
        let before = fs::read(observer_path(&root, "abcdefgh")).expect("before");
        assert_eq!(
            persist_sync(&root, "", DAY, SEGMENT, SyncEventKind::Observed, NOW),
            SyncPersistResult::Skipped
        );
        assert!(!hist_path(&root).exists());
        assert_eq!(
            fs::read(observer_path(&root, "abcdefgh")).expect("after"),
            before
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn skip_cases_write_nothing() {
        let root = root("ac4");
        seed(&root, json!({"bytes_received": 4}));
        let before = fs::read(observer_path(&root, "abcdefgh")).expect("before");
        let cases = [
            ("missing", DAY, SEGMENT),
            ("desk", DAY, ""),
            ("desk", "", SEGMENT),
            ("", DAY, SEGMENT),
            ("abc/def", DAY, SEGMENT),
        ];
        for (name, day, segment) in cases {
            assert_eq!(
                persist_sync(&root, name, day, segment, SyncEventKind::Observed, NOW,),
                SyncPersistResult::Skipped,
                "{name:?} {day:?} {segment:?}"
            );
        }
        assert!(!hist_path(&root).exists());
        assert_eq!(
            fs::read(observer_path(&root, "abcdefgh")).expect("after"),
            before
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn transferred_increments_only_transferred_stat() {
        let root = root("ac5");
        seed(&root, json!({"segments_observed": 3, "bytes_received": 8}));
        assert_eq!(
            persist_sync(&root, "desk", DAY, SEGMENT, SyncEventKind::Transferred, NOW,),
            SyncPersistResult::Applied
        );
        let row: Value = serde_json::from_str(
            fs::read_to_string(hist_path(&root))
                .expect("hist")
                .lines()
                .next()
                .expect("line"),
        )
        .expect("row");
        assert_eq!(row["type"], "transferred");
        assert_eq!(row["segment"], SEGMENT);
        let stats = &read_record(&root)["stats"];
        assert_eq!(stats["segments_transferred"], 1);
        assert_eq!(stats["segments_observed"], 3);
        assert_eq!(stats["bytes_received"], 8);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn torn_history_refuses_without_mutation() {
        let root = root("ac6-torn");
        seed(&root, json!({}));
        let path = hist_path(&root);
        fs::create_dir_all(path.parent().expect("parent")).expect("hist dir");
        let before = "{\"type\":\"observed\",\"ts\":1,\"segment\":\"090000_300\"}\n{broken}\n";
        fs::write(&path, before).expect("write");
        assert_eq!(
            persist_sync(&root, "desk", DAY, SEGMENT, SyncEventKind::Observed, NOW,),
            SyncPersistResult::TornHistory
        );
        assert_eq!(fs::read_to_string(&path).expect("after"), before);
        assert!(
            read_record(&root)["stats"]
                .get("segments_observed")
                .is_none()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unwritable_hist_dir_fails_append_without_stat_change() {
        let root = root("ac6-write");
        seed(&root, json!({}));
        let hist = history_dir(&root, "abcdefgh");
        fs::create_dir_all(&hist).expect("hist dir");
        fs::set_permissions(&hist, fs::Permissions::from_mode(0o555)).expect("chmod hist");
        let result = persist_sync(&root, "desk", DAY, SEGMENT, SyncEventKind::Observed, NOW);
        restore_mode(&hist, 0o755);
        assert_eq!(result, SyncPersistResult::HistoryWriteFailed);
        assert!(!hist_path(&root).exists());
        assert!(
            read_record(&root)["stats"]
                .get("segments_observed")
                .is_none()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn hist_row_survives_failed_stats_save() {
        let root = root("ac7");
        seed(&root, json!({"bytes_received": 2}));
        fs::create_dir_all(history_dir(&root, "abcdefgh")).expect("hist dir");
        let registry = crate::store::paths::observers_dir(&root);
        let before = fs::read(observer_path(&root, "abcdefgh")).expect("before");
        fs::set_permissions(&registry, fs::Permissions::from_mode(0o555)).expect("chmod registry");
        let result = persist_sync(&root, "desk", DAY, SEGMENT, SyncEventKind::Observed, NOW);
        restore_mode(&registry, 0o755);
        assert_eq!(result, SyncPersistResult::HistoryWrittenStatsFailed);
        let lines: Vec<String> = fs::read_to_string(hist_path(&root))
            .expect("hist")
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines.len(), 1);
        let row: Value = serde_json::from_str(&lines[0]).expect("row");
        assert_eq!(row["type"], "observed");
        assert_eq!(row["segment"], SEGMENT);
        assert_eq!(
            fs::read(observer_path(&root, "abcdefgh")).expect("after"),
            before
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
