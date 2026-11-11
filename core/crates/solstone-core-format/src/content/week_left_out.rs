// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the owner left out of their week pages, and the keys a week page holds.
//!
//! The owner can leave a memory or a quote out of a week page. Those choices
//! are kept as `{"keys": [...]}` at [`LEFT_OUT_RELATIVE_PATH`], written by
//! Home's single writer. Search reads them too, so the path, the schema and a
//! strict reader live here, below both.
//!
//! [`read_week_page_keys`] reads a week page's json for its keys only: every
//! `key` string of an entry in `memories` or `said`, with no other check, so it
//! never finds fewer keys than the page's full reader.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};

/// Where the owner's week leave-outs live, relative to the journal.
pub const LEFT_OUT_RELATIVE_PATH: &str = "health/week-left-out.json";

/// The largest leave-out store or week page json either reader accepts.
pub const MAX_WEEK_FILE_BYTES: usize = 1_048_576;

/// The owner's week leave-outs, as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftOut {
    /// Nothing has been left out: the store doesn't exist.
    None,
    /// The keys the owner left out.
    Keys(BTreeSet<String>),
    /// The store exists but couldn't be read or isn't the expected shape.
    Unreadable,
}

/// A week page's keys, as read from its json.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeekPageKeys {
    /// The page's json doesn't exist.
    Absent,
    /// Every key the page's json holds.
    Keys(BTreeSet<String>),
    /// The page's json exists but couldn't be read or isn't a json object.
    Unreadable,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeftOutFile {
    keys: Vec<String>,
}

/// The leave-out store's path in `journal`.
pub fn left_out_path(journal: &Path) -> PathBuf {
    journal.join(LEFT_OUT_RELATIVE_PATH)
}

/// The keys in a leave-out store's bytes, or `None` when they aren't the
/// store's shape.
pub fn parse_left_out(bytes: &[u8]) -> Option<BTreeSet<String>> {
    serde_json::from_slice::<LeftOutFile>(bytes)
        .ok()
        .map(|file| file.keys.into_iter().collect())
}

/// The bytes the leave-out store holds for `keys`, in sorted order.
pub fn left_out_bytes(keys: &BTreeSet<String>) -> Vec<u8> {
    let keys = keys.iter().cloned().collect();
    serde_json::to_vec(&LeftOutFile { keys }).expect("leave-out keys serialize")
}

/// Read the owner's leave-outs from `journal`, telling a store that doesn't
/// exist from one that can't be read. A symlink anywhere on the path reads as
/// unreadable.
pub fn read_left_out(journal: &Path) -> LeftOut {
    match read_bounded(journal, Path::new(LEFT_OUT_RELATIVE_PATH)) {
        Read::Absent => LeftOut::None,
        Read::Bytes(bytes) => parse_left_out(&bytes).map_or(LeftOut::Unreadable, LeftOut::Keys),
        Read::Unreadable => LeftOut::Unreadable,
    }
}

/// Every `key` string of an entry in a week page json's `memories` and `said`
/// lists, or `None` when the bytes aren't a json object. Nothing else is
/// checked: an entry without a string `key`, or a list that isn't a list,
/// adds no key.
pub fn week_page_keys(bytes: &[u8]) -> Option<BTreeSet<String>> {
    let Ok(Value::Object(page)) = serde_json::from_slice::<Value>(bytes) else {
        return None;
    };
    let keys = ["memories", "said"]
        .into_iter()
        .filter_map(|list| page.get(list).and_then(Value::as_array))
        .flatten()
        .filter_map(|entry| entry.get("key").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    Some(keys)
}

/// Read the keys of the week page starting on `start` (`YYYYMMDD`), from
/// `reflections/weekly/<start>.json`, telling a page json that doesn't exist
/// from one that can't be read.
pub fn read_week_page_keys(journal: &Path, start: &str) -> WeekPageKeys {
    let relative = Path::new("reflections")
        .join("weekly")
        .join(format!("{start}.json"));
    match read_bounded(journal, &relative) {
        Read::Absent => WeekPageKeys::Absent,
        Read::Bytes(bytes) => {
            week_page_keys(&bytes).map_or(WeekPageKeys::Unreadable, WeekPageKeys::Keys)
        }
        Read::Unreadable => WeekPageKeys::Unreadable,
    }
}

enum Read {
    Absent,
    Bytes(Vec<u8>),
    Unreadable,
}

fn read_bounded(journal: &Path, relative: &Path) -> Read {
    let Ok(root) = JournalRoot::open(journal) else {
        return if journal.exists() {
            Read::Unreadable
        } else {
            Read::Absent
        };
    };
    match read_relative_file_bounded(&root, relative, MAX_WEEK_FILE_BYTES) {
        Ok(Some(observation)) => Read::Bytes(observation.bytes),
        Ok(None) => Read::Absent,
        Err(_) => Read::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn write(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn keys(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn the_store_lives_under_config() {
        let journal = TempDir::new().unwrap();
        assert_eq!(
            left_out_path(journal.path()),
            journal.path().join("health/week-left-out.json")
        );
    }

    #[test]
    fn a_missing_store_reads_as_nothing_left_out() {
        let journal = TempDir::new().unwrap();
        assert_eq!(read_left_out(journal.path()), LeftOut::None);
        let gone = journal.path().join("not-here");
        assert_eq!(read_left_out(&gone), LeftOut::None);
    }

    #[test]
    fn the_store_reads_back_what_was_written() {
        let journal = TempDir::new().unwrap();
        let written = keys(&["k2", "k1"]);
        write(
            journal.path(),
            LEFT_OUT_RELATIVE_PATH,
            &left_out_bytes(&written),
        );
        assert_eq!(read_left_out(journal.path()), LeftOut::Keys(written));
        let on_disk: Value =
            serde_json::from_slice(&fs::read(left_out_path(journal.path())).unwrap()).unwrap();
        assert_eq!(on_disk, json!({"keys": ["k1", "k2"]}));
    }

    #[test]
    fn a_damaged_store_reads_as_unreadable() {
        for damaged in [
            &b"garbage"[..],
            br#"{"keys":"k1"}"#,
            br#"{"keys":["k1"],"extra":1}"#,
            br#"{}"#,
            br#"["k1"]"#,
        ] {
            let journal = TempDir::new().unwrap();
            write(journal.path(), LEFT_OUT_RELATIVE_PATH, damaged);
            assert_eq!(
                read_left_out(journal.path()),
                LeftOut::Unreadable,
                "{}",
                String::from_utf8_lossy(damaged)
            );
        }
    }

    #[test]
    fn a_store_too_large_reads_as_unreadable() {
        let journal = TempDir::new().unwrap();
        let mut bytes = br#"{"keys":[""#.to_vec();
        bytes.resize(MAX_WEEK_FILE_BYTES + 1, b'k');
        bytes.extend_from_slice(br#""]}"#);
        write(journal.path(), LEFT_OUT_RELATIVE_PATH, &bytes);
        assert_eq!(read_left_out(journal.path()), LeftOut::Unreadable);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_store_reads_as_unreadable() {
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("left-out.json");
        fs::write(&target, left_out_bytes(&keys(&["outside"]))).unwrap();
        let journal = TempDir::new().unwrap();
        fs::create_dir_all(journal.path().join("health")).unwrap();
        std::os::unix::fs::symlink(&target, left_out_path(journal.path())).unwrap();
        assert_eq!(read_left_out(journal.path()), LeftOut::Unreadable);
    }

    #[test]
    fn page_keys_are_every_key_under_memories_and_said() {
        let page = json!({
            "version": 1,
            "memories": [
                {"id": "m1", "key": "k_memory", "day": "20260308", "text": "a"},
                {"key": ""},
                {"id": "m3"},
                {"key": 7},
                "not an entry",
            ],
            "said": [
                {"id": "s1", "key": "k_said", "source": {"kind": "voice", "key": "nested"}},
            ],
            "days": [{"key": "k_day"}],
            "key": "k_top",
        });
        assert_eq!(
            week_page_keys(page.to_string().as_bytes()),
            Some(keys(&["", "k_memory", "k_said"]))
        );
    }

    #[test]
    fn page_keys_need_only_a_json_object() {
        assert_eq!(week_page_keys(br#"{}"#), Some(BTreeSet::new()));
        assert_eq!(
            week_page_keys(br#"{"memories": {"key": "k"}, "said": "k"}"#),
            Some(BTreeSet::new())
        );
        assert_eq!(week_page_keys(b"garbage"), None);
        assert_eq!(week_page_keys(br#"[{"key": "k"}]"#), None);
    }

    #[test]
    fn a_page_json_reads_as_absent_keys_or_unreadable() {
        let journal = TempDir::new().unwrap();
        assert_eq!(
            read_week_page_keys(journal.path(), "20260308"),
            WeekPageKeys::Absent
        );
        write(
            journal.path(),
            "reflections/weekly/20260308.json",
            json!({"memories": [{"key": "k1"}], "said": [{"key": "k2"}]})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(
            read_week_page_keys(journal.path(), "20260308"),
            WeekPageKeys::Keys(keys(&["k1", "k2"]))
        );
        assert_eq!(
            read_week_page_keys(journal.path(), "20260315"),
            WeekPageKeys::Absent
        );
        write(journal.path(), "reflections/weekly/20260315.json", b"{");
        assert_eq!(
            read_week_page_keys(journal.path(), "20260315"),
            WeekPageKeys::Unreadable
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_page_json_reads_as_unreadable() {
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("page.json");
        fs::write(&target, br#"{"memories":[{"key":"outside"}]}"#).unwrap();
        let journal = TempDir::new().unwrap();
        let weekly = journal.path().join("reflections/weekly");
        fs::create_dir_all(&weekly).unwrap();
        std::os::unix::fs::symlink(&target, weekly.join("20260308.json")).unwrap();
        assert_eq!(
            read_week_page_keys(journal.path(), "20260308"),
            WeekPageKeys::Unreadable
        );
    }
}
