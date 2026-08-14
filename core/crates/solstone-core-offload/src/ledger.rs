// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable JSONL pre-mark witness for media offload.
//!
//! This is JSONL rather than SQLite because backups exclude `*.sqlite*` but
//! include `health/`. Appends are fsynced by journal-io: this record exists
//! before a pending-release mark is minted. Folding is append order, never
//! timestamp order; malformed reads degrade rather than creating a trusted zero.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::append_jsonl;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffloadFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentOffloadSummary {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub currently_offloaded: bool,
    pub snapshot_id: Option<String>,
    pub files: Vec<OffloadFile>,
    pub offloaded_bytes: u64,
    pub offloaded_file_count: u64,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayOffloadSummary {
    pub day: String,
    pub segments: Vec<SegmentOffloadSummary>,
    pub offloaded_bytes: u64,
    pub offloaded_file_count: u64,
    pub offloaded_segments: u64,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalOffloadSummary {
    pub days: Vec<DayOffloadSummary>,
    pub offloaded_bytes: u64,
    pub offloaded_file_count: u64,
    pub offloaded_segments: u64,
    pub offloaded_days: u64,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
impl SegmentOffloadSummary {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}
impl DayOffloadSummary {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}
impl JournalOffloadSummary {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}

#[derive(Clone)]
struct Event {
    stream: String,
    segment: String,
    snapshot: Option<String>,
    files: Vec<OffloadFile>,
    offload: bool,
}
fn valid_day(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
}
fn valid_file(file: &OffloadFile) -> bool {
    !file.name.is_empty()
        && !file.name.contains('/')
        && file.name != "."
        && file.name != ".."
        && file.sha256.len() == 64
        && file
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !(byte as char).is_ascii_uppercase())
}
fn path(journal: &Path, day: &str) -> PathBuf {
    journal.join("health/offload").join(format!("{day}.jsonl"))
}
pub fn ledger_path_for_day(journal: &Path, day: &str) -> Result<PathBuf, String> {
    if !valid_day(day) {
        return Err("day must be in YYYYMMDD format".into());
    }
    Ok(path(journal, day))
}
pub fn append_offload_event(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    snapshot_id: &str,
    files: &[OffloadFile],
    time: u64,
) -> Result<(), String> {
    if !valid_day(day)
        || stream.is_empty()
        || segment.is_empty()
        || snapshot_id.is_empty()
        || files.is_empty()
        || files.iter().any(|file| !valid_file(file))
    {
        return Err("invalid offload ledger event".into());
    }
    let record = serde_json::json!({"event_kind":"offload","time":time,"day":day,"stream":stream,"segment":segment,"snapshot_id":snapshot_id,"files":files});
    append_jsonl(path(journal, day), &record).map_err(|error| error.to_string())
}
pub fn append_restore_event(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    time: u64,
) -> Result<(), String> {
    if !valid_day(day) || stream.is_empty() || segment.is_empty() {
        return Err("invalid restore ledger event".into());
    }
    append_jsonl(path(journal,day),&serde_json::json!({"event_kind":"restore","time":time,"day":day,"stream":stream,"segment":segment})).map_err(|error|error.to_string())
}
fn read(journal: &Path, day: &str) -> (Vec<Event>, u64, Vec<String>) {
    let file = path(journal, day);
    let raw = match fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (vec![], 0, vec![]),
        Err(_) => return (vec![], 0, vec![file.display().to_string()]),
    };
    let mut events = vec![];
    let mut skipped = 0;
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
            skipped += 1;
            continue;
        };
        let day_value = record.get("day").and_then(Value::as_str);
        let stream = record.get("stream").and_then(Value::as_str);
        let segment = record.get("segment").and_then(Value::as_str);
        if day_value != Some(day)
            || stream.is_none_or(str::is_empty)
            || segment.is_none_or(str::is_empty)
        {
            skipped += 1;
            continue;
        }
        match record.get("event_kind").and_then(Value::as_str) {
            Some("restore") if record.len() == 5 => events.push(Event {
                stream: stream.unwrap().into(),
                segment: segment.unwrap().into(),
                snapshot: None,
                files: vec![],
                offload: false,
            }),
            Some("offload") => {
                let snapshot = record
                    .get("snapshot_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let files = record
                    .get("files")
                    .and_then(Value::as_array)
                    .and_then(|files| {
                        files
                            .iter()
                            .map(|value| serde_json::from_value::<OffloadFile>(value.clone()).ok())
                            .collect::<Option<Vec<_>>>()
                    });
                if record.len() != 7
                    || snapshot.is_none()
                    || files.as_ref().is_none_or(Vec::is_empty)
                    || files
                        .as_ref()
                        .is_some_and(|files| files.iter().any(|file| !valid_file(file)))
                {
                    skipped += 1
                } else {
                    events.push(Event {
                        stream: stream.unwrap().into(),
                        segment: segment.unwrap().into(),
                        snapshot: snapshot.map(str::to_owned),
                        files: files.unwrap(),
                        offload: true,
                    })
                }
            }
            _ => skipped += 1,
        }
    }
    (events, skipped, vec![])
}
pub fn summarize_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<SegmentOffloadSummary, String> {
    if !valid_day(day) {
        return Err("day must be in YYYYMMDD format".into());
    }
    let (events, skipped, unreadable) = read(journal, day);
    let current = events
        .into_iter()
        .filter(|event| event.stream == stream && event.segment == segment)
        .fold(
            None,
            |_state, event| if event.offload { Some(event) } else { None },
        );
    Ok(summary(day, stream, segment, current, skipped, unreadable))
}
fn summary(
    day: &str,
    stream: &str,
    segment: &str,
    current: Option<Event>,
    skipped: u64,
    unreadable: Vec<String>,
) -> SegmentOffloadSummary {
    let files = current
        .as_ref()
        .map(|event| event.files.clone())
        .unwrap_or_default();
    SegmentOffloadSummary {
        day: day.into(),
        stream: stream.into(),
        segment: segment.into(),
        currently_offloaded: current.is_some(),
        snapshot_id: current.and_then(|event| event.snapshot),
        offloaded_bytes: files.iter().map(|file| file.bytes).sum(),
        offloaded_file_count: files.len() as u64,
        files,
        skipped_records: skipped,
        unreadable_ledgers: unreadable,
    }
}
pub fn summarize_day(journal: &Path, day: &str) -> Result<DayOffloadSummary, String> {
    if !valid_day(day) {
        return Err("day must be in YYYYMMDD format".into());
    }
    let (events, skipped, unreadable) = read(journal, day);
    let mut current = BTreeMap::new();
    for event in events {
        let key = (event.stream.clone(), event.segment.clone());
        if event.offload {
            current.insert(key, event);
        } else {
            current.remove(&key);
        }
    }
    let segments = current
        .into_values()
        .map(|event| {
            let stream = event.stream.clone();
            let segment = event.segment.clone();
            summary(
                day,
                &stream,
                &segment,
                Some(event),
                skipped,
                unreadable.clone(),
            )
        })
        .collect::<Vec<_>>();
    Ok(DayOffloadSummary {
        day: day.into(),
        offloaded_bytes: segments.iter().map(|item| item.offloaded_bytes).sum(),
        offloaded_file_count: segments.iter().map(|item| item.offloaded_file_count).sum(),
        offloaded_segments: segments.len() as u64,
        segments,
        skipped_records: skipped,
        unreadable_ledgers: unreadable,
    })
}
pub fn summarize_journal(journal: &Path) -> JournalOffloadSummary {
    let directory = journal.join("health/offload");
    let mut days = fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()?
                .to_str()
                .filter(|day| valid_day(day))
                .map(str::to_owned)
        })
        .filter_map(|day| summarize_day(journal, &day).ok())
        .collect::<Vec<_>>();
    days.sort_by(|left, right| left.day.cmp(&right.day));
    JournalOffloadSummary {
        offloaded_bytes: days.iter().map(|day| day.offloaded_bytes).sum(),
        offloaded_file_count: days.iter().map(|day| day.offloaded_file_count).sum(),
        offloaded_segments: days.iter().map(|day| day.offloaded_segments).sum(),
        offloaded_days: days.iter().filter(|day| day.offloaded_segments > 0).count() as u64,
        skipped_records: days.iter().map(|day| day.skipped_records).sum(),
        unreadable_ledgers: days
            .iter()
            .flat_map(|day| day.unreadable_ledgers.clone())
            .collect(),
        days,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn append_order_beats_timestamp_and_read_is_read_only() {
        let journal = tempfile::tempdir().unwrap();
        let file = OffloadFile {
            name: "raw.webm".into(),
            bytes: 3,
            sha256: "a".repeat(64),
        };
        append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000",
            "snapshot",
            &[file],
            20,
        )
        .unwrap();
        append_restore_event(journal.path(), "20260101", "_default", "010000", 10).unwrap();
        let before = fs::read(path(journal.path(), "20260101")).unwrap();
        assert!(
            !summarize_segment(journal.path(), "20260101", "_default", "010000")
                .unwrap()
                .currently_offloaded
        );
        assert_eq!(before, fs::read(path(journal.path(), "20260101")).unwrap());
    }

    #[test]
    fn duplicate_and_reordered_events_fold_in_append_order() {
        let journal = tempfile::tempdir().unwrap();
        let file = OffloadFile {
            name: "raw.webm".into(),
            bytes: 7,
            sha256: "b".repeat(64),
        };
        append_restore_event(journal.path(), "20260102", "_default", "020000_001", 99).unwrap();
        append_offload_event(
            journal.path(),
            "20260102",
            "_default",
            "020000_001",
            "first",
            std::slice::from_ref(&file),
            1,
        )
        .unwrap();
        append_offload_event(
            journal.path(),
            "20260102",
            "_default",
            "020000_001",
            "last",
            &[file],
            2,
        )
        .unwrap();

        let summary =
            summarize_segment(journal.path(), "20260102", "_default", "020000_001").unwrap();
        assert!(summary.currently_offloaded);
        assert_eq!(summary.snapshot_id.as_deref(), Some("last"));
    }

    #[test]
    fn malformed_and_wrong_identity_records_degrade_without_summary_writes() {
        let journal = tempfile::tempdir().unwrap();
        let ledger = path(journal.path(), "20260103");
        fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        fs::write(
            &ledger,
            concat!(
                "{\"event_kind\":\"offload\",\"time\":1,\"day\":\"20260103\",\"stream\":\"_default\",\"segment\":\"030000_001\",\"snapshot_id\":\"snapshot\",\"files\":[{\"name\":\"raw.webm\",\"bytes\":3,\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}]}\n",
                "{\"event_kind\":\"offload\",\"time\":2,\"day\":\"wrong\",\"stream\":\"_default\",\"segment\":\"030000_001\",\"snapshot_id\":\"snapshot\",\"files\":[]}\n",
                "{\"event_kind\":\"offload\",\"time\":3,\"day\":\"20260103\",\"stream\":\"_default\",\"segment\":\"030000_001\",\"snapshot_id\":\"snapshot\",\"files\":[{\"name\":\"../wrong.webm\",\"bytes\":3,\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}]}\n",
                "{\"event_kind\":\"offload\"\n"
            ),
        )
        .unwrap();
        let bytes = fs::read(&ledger).unwrap();
        let modified = fs::metadata(&ledger).unwrap().modified().unwrap();

        for _ in 0..2 {
            let summary =
                summarize_segment(journal.path(), "20260103", "_default", "030000_001").unwrap();
            assert!(summary.currently_offloaded);
            assert_eq!(summary.skipped_records, 3);
            assert!(summary.degraded());
        }
        assert_eq!(fs::read(&ledger).unwrap(), bytes);
        assert_eq!(fs::metadata(&ledger).unwrap().modified().unwrap(), modified);
    }
}
