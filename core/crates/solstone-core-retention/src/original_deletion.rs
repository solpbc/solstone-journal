// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Original-media deletion provenance and reader.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use crate::marks::RemovalClass;

const EVENTS_FILE: &str = "events.jsonl";
const RETENTION_TRACT: &str = "retention";
const ORIGINAL_DELETED_EVENT: &str = "original_deleted";

/// Provenance of a released original media file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum RawReleaseClass {
    Policy,
    Offload,
    Owner,
}

impl RawReleaseClass {
    /// Return the wire/storage tag for this release class.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Policy => "policy_raw_release",
            Self::Offload => "offload_raw_release",
            Self::Owner => "owner_raw_release",
        }
    }

    /// Parse a wire/storage tag into a raw release class.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "policy_raw_release" => Some(Self::Policy),
            "offload_raw_release" => Some(Self::Offload),
            "owner_raw_release" => Some(Self::Owner),
            _ => None,
        }
    }

    /// Convert a general removal class to a raw release class if it applies to raw media.
    #[must_use]
    pub fn from_removal_class(class: RemovalClass) -> Option<Self> {
        match class {
            RemovalClass::PolicyRawRelease => Some(Self::Policy),
            RemovalClass::OffloadRawRelease => Some(Self::Offload),
            RemovalClass::OwnerRawRelease => Some(Self::Owner),
            RemovalClass::OwnerSegmentRemoval => None,
        }
    }

    /// Convert to the matching general removal class.
    #[must_use]
    pub fn to_removal_class(self) -> RemovalClass {
        match self {
            Self::Policy => RemovalClass::PolicyRawRelease,
            Self::Offload => RemovalClass::OffloadRawRelease,
            Self::Owner => RemovalClass::OwnerRawRelease,
        }
    }
}

/// Read and return all recorded original-media deletions from a segment's event log.
///
/// Refuses symlinked leaves and directory collisions without following links.
#[must_use]
pub fn recorded_original_deletions(segment_dir: &Path) -> BTreeMap<String, RawReleaseClass> {
    let mut map = BTreeMap::new();
    let entries = match list_dir_entries(segment_dir) {
        Ok(entries) => entries,
        Err(_) => return map,
    };

    let has_regular_events_file = entries.iter().any(|entry| {
        entry.name.as_os_str() == std::ffi::OsStr::new(EVENTS_FILE)
            && entry.kind == DirEntryKind::File
    });
    if !has_regular_events_file {
        return map;
    }

    let path = segment_dir.join(EVENTS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return map,
    };

    for raw_line in bytes.split(|&b| b == b'\n') {
        let trimmed = match std::str::from_utf8(raw_line) {
            Ok(s) => s.trim(),
            Err(_) => continue,
        };
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(obj) = parsed.as_object() else {
            continue;
        };
        let Some(tract) = obj.get("tract").and_then(Value::as_str) else {
            continue;
        };
        if tract != RETENTION_TRACT {
            continue;
        }
        let Some(event) = obj.get("event").and_then(Value::as_str) else {
            continue;
        };
        if event != ORIGINAL_DELETED_EVENT {
            continue;
        }
        let Some(name) = obj.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(class_str) = obj.get("class").and_then(Value::as_str) else {
            continue;
        };
        let Some(class) = RawReleaseClass::from_tag(class_str) else {
            continue;
        };
        map.insert(name.to_owned(), class);
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_utf8_line_between_valid_rows_is_skipped_and_both_names_survive() {
        let temp = tempfile::tempdir().unwrap();
        let segment_dir = temp.path();
        let events_path = segment_dir.join(EVENTS_FILE);
        let mut data = Vec::new();
        data.extend_from_slice(
            br#"{"tract":"retention","event":"original_deleted","ts":1000,"name":"raw.flac","class":"policy_raw_release"}"#,
        );
        data.push(b'\n');
        data.extend_from_slice(b"\xff\xfe\xfd\x80\n");
        data.extend_from_slice(
            br#"{"tract":"retention","event":"original_deleted","ts":2000,"name":"screen.webm","class":"offload_raw_release"}"#,
        );
        data.push(b'\n');
        fs::write(&events_path, data).unwrap();

        let records = recorded_original_deletions(segment_dir);
        assert_eq!(records.len(), 2);
        assert_eq!(records.get("raw.flac"), Some(&RawReleaseClass::Policy));
        assert_eq!(records.get("screen.webm"), Some(&RawReleaseClass::Offload));
    }

    #[test]
    fn later_row_for_a_name_replaces_earlier() {
        let temp = tempfile::tempdir().unwrap();
        let segment_dir = temp.path();
        let events_path = segment_dir.join(EVENTS_FILE);
        let content = concat!(
            r#"{"tract":"retention","event":"original_deleted","ts":1000,"name":"raw.flac","class":"policy_raw_release"}"#,
            "\n",
            r#"{"tract":"retention","event":"original_deleted","ts":2000,"name":"raw.flac","class":"owner_raw_release"}"#,
            "\n"
        );
        fs::write(&events_path, content).unwrap();

        let records = recorded_original_deletions(segment_dir);
        assert_eq!(records.get("raw.flac"), Some(&RawReleaseClass::Owner));
    }

    #[test]
    fn unparseable_and_foreign_tract_and_invalid_classes_are_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let segment_dir = temp.path();
        let events_path = segment_dir.join(EVENTS_FILE);
        let content = concat!(
            r#"{"tract":"observe","event":"capture_started","ts":500}"#,
            "\n",
            "not a json line\n",
            r#"{"tract":"retention","event":"original_deleted","ts":1000,"name":"screen.webm","class":"owner_segment_removal"}"#,
            "\n",
            r#"{"tract":"retention","event":"original_deleted","ts":1000,"name":"screen.webm","class":"unknown_class"}"#,
            "\n",
            r#"{"tract":"retention","event":"original_deleted","ts":1000,"name":123,"class":"policy_raw_release"}"#,
            "\n",
            r#"{"tract":"retention","event":"other_event","ts":1000,"name":"screen.webm","class":"policy_raw_release"}"#,
            "\n",
            r#"{"tract":"retention","event":"original_deleted","ts":1000,"name":"raw.flac","class":"policy_raw_release"}"#,
            "\n",
            r#"{"tract":"retention","event":"original_deleted","ts":1000,"name":"screen.webm","class":"offload_raw_release"}"#,
            "\n"
        );
        fs::write(&events_path, content).unwrap();

        let records = recorded_original_deletions(segment_dir);
        assert_eq!(records.len(), 2);
        assert_eq!(records.get("raw.flac"), Some(&RawReleaseClass::Policy));
        assert_eq!(records.get("screen.webm"), Some(&RawReleaseClass::Offload));
    }

    #[test]
    fn missing_events_file_returns_empty_map() {
        let temp = tempfile::tempdir().unwrap();
        let records = recorded_original_deletions(temp.path());
        assert!(records.is_empty());
    }

    #[test]
    fn events_file_as_directory_returns_empty_map_without_panic() {
        let temp = tempfile::tempdir().unwrap();
        let segment_dir = temp.path();
        let events_dir = segment_dir.join(EVENTS_FILE);
        fs::create_dir(&events_dir).unwrap();

        let records = recorded_original_deletions(segment_dir);
        assert!(records.is_empty());
    }
}
