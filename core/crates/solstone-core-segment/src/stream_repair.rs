// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Tolerant stream-registry inspection and targeted stream-tail repair.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, MalformedPolicy, hold_lock, read_json,
};

use crate::stream_record::{registry_json_paths, stream_record_path, write_stream_record};
use crate::{SegmentError, StreamRecord};

/// The tail derived from validated segment markers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkerTail<'a> {
    pub last_day: &'a str,
    pub last_segment: &'a str,
    pub max_seq: u64,
}

/// Why a marker-driven repair made no change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnchangedReason {
    AlreadyCurrent,
    RecordAhead,
}

/// The result of a marker-driven stream-tail repair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairOutcome {
    Repaired,
    Unchanged(UnchangedReason),
    NoRecord,
    Malformed,
    Locked,
    WriteFailed,
}

/// A tolerant registry scan: valid object records and isolated bad files.
#[derive(Debug, Default)]
pub struct TolerantStreamRecords {
    pub records: Vec<(String, Value)>,
    pub anomalies: Vec<(PathBuf, String)>,
}

/// Read one registry record without normalizing its keys or values.
pub fn read_stream_record(journal: &Path, name: &str) -> Result<Option<Value>, SegmentError> {
    read_stream_record_value(&stream_record_path(journal, name))
}

/// Read every registry record independently, preserving valid raw objects when
/// another record is malformed.
pub fn list_stream_records_tolerant(journal: &Path) -> Result<TolerantStreamRecords, SegmentError> {
    let mut result = TolerantStreamRecords::default();
    for path in registry_json_paths(journal)? {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        match read_stream_record_value(&path) {
            Ok(Some(value)) if value.is_object() => result.records.push((name, value)),
            Ok(Some(_)) | Ok(None) | Err(_) => {
                result.anomalies.push((path, "malformed record".to_owned()));
            }
        }
    }
    Ok(result)
}

/// Repair an existing registry record from a marker-derived tail. The initial
/// existence check intentionally precedes lock acquisition so a marker-only
/// stream never creates the registry directory or a lock sidecar.
pub fn repair_stream_tail_from_markers(
    journal: &Path,
    stream: &str,
    marker_tail: &MarkerTail<'_>,
    lock_options: LockOptions,
) -> RepairOutcome {
    let path = stream_record_path(journal, stream);
    if !path.exists() {
        return RepairOutcome::NoRecord;
    }
    let _lock = match hold_lock(&path, lock_options) {
        Ok(lock) => lock,
        Err(_) => return RepairOutcome::Locked,
    };
    let value = match read_stream_record_value(&path) {
        Ok(Some(value)) if value.is_object() => value,
        Ok(Some(_)) | Ok(None) | Err(_) => return RepairOutcome::Malformed,
    };
    let record: StreamRecord = match serde_json::from_value(value.clone()) {
        Ok(record) => record,
        Err(_) => return RepairOutcome::Malformed,
    };
    if record.seq > marker_tail.max_seq {
        return RepairOutcome::Unchanged(UnchangedReason::RecordAhead);
    }
    if record.seq == marker_tail.max_seq
        && record.last_day.as_deref() == Some(marker_tail.last_day)
        && record.last_segment.as_deref() == Some(marker_tail.last_segment)
    {
        return RepairOutcome::Unchanged(UnchangedReason::AlreadyCurrent);
    }
    let mut value = value;
    let object = value.as_object_mut().expect("object checked above");
    object.insert("last_day".to_owned(), json!(marker_tail.last_day));
    object.insert("last_segment".to_owned(), json!(marker_tail.last_segment));
    object.insert("seq".to_owned(), json!(marker_tail.max_seq));
    match write_stream_record(&path, &value) {
        Ok(()) => RepairOutcome::Repaired,
        Err(_) => RepairOutcome::WriteFailed,
    }
}

/// Set a registry tail after prune has already determined that its recorded
/// tail is absent. This preserves the prune path's intentionally best-effort,
/// lock-free behavior.
pub fn set_stream_tail_unconditionally(
    journal: &Path,
    stream: &str,
    last_day: Option<&str>,
    last_segment: Option<&str>,
    max_seq: u64,
) -> StreamRecord {
    let path = stream_record_path(journal, stream);
    let existing =
        read_stream_record_value(&path).ok().flatten().and_then(
            |value| match serde_json::from_value::<StreamRecord>(value.clone()) {
                Ok(record) if value.is_object() => Some((value, record)),
                Err(_) | Ok(_) => None,
            },
        );
    if let Some((mut value, record)) = existing {
        let object = value.as_object_mut().expect("object checked above");
        object.insert("last_day".to_owned(), json!(last_day));
        object.insert("last_segment".to_owned(), json!(last_segment));
        object.insert("seq".to_owned(), json!(record.seq.max(max_seq)));
        let updated: StreamRecord = serde_json::from_value(value.clone())
            .expect("in-place mutation preserves a usable stream record");
        let _ = write_stream_record(&path, &value);
        return updated;
    }

    let state = default_stream_record(stream, last_day, last_segment, max_seq);
    let _ = write_stream_record(&path, &state);
    state
}

/// Touch a chronicle day's stream health marker.
pub fn touch_stream_health_marker(
    journal: &Path,
    day: &str,
) -> Result<(), solstone_core_journal_io::AtomicWriteError> {
    solstone_core_journal_io::atomic_replace(
        journal
            .join("chronicle")
            .join(day)
            .join("health")
            .join("stream.updated"),
        b"",
        AtomicWriteOptions::default(),
    )
}

fn read_stream_record_value(path: &Path) -> Result<Option<Value>, SegmentError> {
    match read_json(path, None, MalformedPolicy::Raise) {
        Ok(value) => Ok(value),
        Err(error @ solstone_core_journal_io::ReadError::Malformed(_)) => {
            Err(SegmentError::MalformedStreamRecord {
                path: path.to_path_buf(),
                source: error,
            })
        }
        Err(error) => Err(SegmentError::Read(error)),
    }
}

fn default_stream_record(
    stream: &str,
    last_day: Option<&str>,
    last_segment: Option<&str>,
    max_seq: u64,
) -> StreamRecord {
    StreamRecord {
        name: stream.to_owned(),
        kind: "unknown".to_owned(),
        host: None,
        platform: None,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
        last_day: last_day.map(ToOwned::to_owned),
        last_segment: last_segment.map(ToOwned::to_owned),
        seq: max_seq,
        did: None,
        source: None,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use solstone_core_journal_io::{LockOptions, hold_lock};

    use crate::test_support::TempDir;

    use super::*;

    fn record_path(root: &Path, name: &str) -> PathBuf {
        root.join("streams").join(format!("{name}.json"))
    }

    #[test]
    fn tolerant_list_keeps_valid_records_and_reports_bad_paths() {
        let temporary = TempDir::new();
        let valid = record_path(temporary.path(), "valid");
        fs::create_dir_all(valid.parent().unwrap()).unwrap();
        fs::write(&valid, br#"{"name":"valid","type":"observer"}"#).unwrap();
        let broken = record_path(temporary.path(), "broken");
        fs::write(&broken, b"{not json").unwrap();

        let listed = list_stream_records_tolerant(temporary.path()).unwrap();
        assert_eq!(listed.records.len(), 1);
        assert_eq!(listed.records[0].0, "valid");
        assert_eq!(listed.records[0].1["type"], "observer");
        assert_eq!(
            listed.anomalies,
            vec![(broken, "malformed record".to_owned())]
        );
    }

    #[test]
    fn marker_repair_preserves_legacy_and_unknown_fields() {
        let temporary = TempDir::new();
        let path = record_path(temporary.path(), "workstation");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":"20260101","last_segment":"090000_300","seq":1,"legacy":"kept"}"#,
        )
        .unwrap();

        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "workstation",
                &MarkerTail {
                    last_day: "20260102",
                    last_segment: "100000_300",
                    max_seq: 2,
                },
                LockOptions::default(),
            ),
            RepairOutcome::Repaired
        );
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["type"], "observer");
        assert_eq!(value["legacy"], "kept");
        assert_eq!(value["last_day"], "20260102");
        assert_eq!(value["seq"], 2);
    }

    #[test]
    fn marker_repair_missing_record_creates_no_directory_or_lock() {
        let temporary = TempDir::new();
        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "missing",
                &MarkerTail {
                    last_day: "20260101",
                    last_segment: "090000_300",
                    max_seq: 1,
                },
                LockOptions::default(),
            ),
            RepairOutcome::NoRecord
        );
        assert!(!temporary.path().join("streams").exists());
    }

    #[test]
    fn marker_repair_does_not_touch_an_already_current_record() {
        let temporary = TempDir::new();
        let path = record_path(temporary.path(), "workstation");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":"20260101","last_segment":"090000_300","seq":2}"#;
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "workstation",
                &MarkerTail {
                    last_day: "20260101",
                    last_segment: "090000_300",
                    max_seq: 2,
                },
                LockOptions::default(),
            ),
            RepairOutcome::Unchanged(UnchangedReason::AlreadyCurrent)
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn marker_repair_reports_a_held_lock() {
        let temporary = TempDir::new();
        let path = record_path(temporary.path(), "workstation");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":null,"last_segment":null,"seq":0}"#).unwrap();
        let _held = hold_lock(&path, LockOptions::default()).unwrap();
        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "workstation",
                &MarkerTail {
                    last_day: "20260101",
                    last_segment: "090000_300",
                    max_seq: 1,
                },
                LockOptions {
                    timeout: Duration::from_millis(10),
                    ..LockOptions::default()
                },
            ),
            RepairOutcome::Locked
        );
    }

    #[test]
    fn health_marker_creates_its_parent_and_surfaces_failure() {
        let temporary = TempDir::new();
        touch_stream_health_marker(temporary.path(), "20260101").unwrap();
        assert!(
            temporary
                .path()
                .join("chronicle/20260101/health/stream.updated")
                .is_file()
        );

        let blocked = TempDir::new();
        let health = blocked.path().join("chronicle/20260102/health");
        fs::create_dir_all(health.parent().unwrap()).unwrap();
        fs::write(&health, b"not a directory").unwrap();
        assert!(touch_stream_health_marker(blocked.path(), "20260102").is_err());
    }
}
