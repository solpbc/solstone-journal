// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's own answers to "who said this line", one file per segment.
//!
//! Each answer is owner-authored and cannot be rebuilt from anything else in
//! the journal. The answers live in `talents/owner_line_labels.json`, apart
//! from `speaker_labels.json`, because voice, evidence and discovery code
//! reads the labels file and owner-voice mode removes audio based on voice
//! evidence. An owner's answer about who spoke is not voice evidence, so no
//! voice or evidence reader may read this file. A repository source census
//! holds that boundary.
//!
//! This module owns the file name, its path, its JSON form and its read. The
//! validated set and remove live in `solstone-core-speaker-resolve`.
//!
//! ```json
//! {
//!   "version": 1,
//!   "entries": [
//!     {
//!       "source": "audio",
//!       "sentence_id": 3,
//!       "start": "00:00:05",
//!       "text_sha256": "<sha-256 hex of the line's text>",
//!       "speaker": "ada_lovelace",
//!       "set_at": "2026-10-31T12:00:00Z"
//!     }
//!   ]
//! }
//! ```
//!
//! `sentence_id` is the line's statement ordinal, the coordinate speaker
//! label rows use for that line. `speaker` is an entity id, or `null` for
//! someone the owner doesn't know. Entries are keyed by `(source,
//! sentence_id)` and kept in that order.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::durability::{ArtifactId, DurableObservation, observe_json_durable};

use crate::json::write_python_compatible_json;

/// File name of a segment's owner line labels, inside its `talents/` directory.
pub const OWNER_LINE_LABELS_FILE: &str = "owner_line_labels.json";

/// The one JSON form this module reads and writes.
pub const OWNER_LINE_LABELS_VERSION: u64 = 1;

const ENTRY_FIELDS: [&str; 6] = [
    "source",
    "sentence_id",
    "start",
    "text_sha256",
    "speaker",
    "set_at",
];

/// Path of a segment's owner line labels.
pub fn owner_line_labels_path(segment_dir: &Path) -> PathBuf {
    segment_dir.join("talents").join(OWNER_LINE_LABELS_FILE)
}

/// What identifies a transcript line's content: its start time and a hash of
/// its text. A reader compares it with the current transcript to tell when a
/// re-transcribed source no longer has the line an answer was given for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineFingerprint {
    /// The line's `start` value exactly as the transcript stores it.
    pub start: Value,
    /// SHA-256 hex of the line's transcribed `text` (empty when it has none).
    pub text_sha256: String,
}

impl LineFingerprint {
    /// Fingerprint one transcript statement record.
    pub fn of_record(record: &Map<String, Value>) -> Self {
        let text = record.get("text").and_then(Value::as_str).unwrap_or("");
        Self {
            start: record.get("start").cloned().unwrap_or(Value::Null),
            text_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
        }
    }
}

/// One owner answer for one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerLineEntry {
    /// The audio source stem the line belongs to, such as `audio`.
    pub source: String,
    /// The line's statement ordinal within that source.
    pub sentence_id: i64,
    pub fingerprint: LineFingerprint,
    /// The entity the owner named, or `None` for someone they don't know.
    pub speaker: Option<String>,
    /// When the owner gave this answer (RFC 3339).
    pub set_at: String,
}

impl OwnerLineEntry {
    /// The key entries are unique by and ordered by.
    pub fn key(&self) -> (&str, i64) {
        (&self.source, self.sentence_id)
    }
}

/// What a segment's owner line labels file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerLineLabels {
    /// No file: the owner has given no answers for this segment.
    Absent,
    /// The file's entries.
    Present(Vec<OwnerLineEntry>),
    /// The file exists but can't be read or isn't a valid record. It is left
    /// exactly as found; nothing sets it aside or rewrites it.
    Unreadable(String),
}

/// Read a segment's owner line labels without changing the file.
pub fn read_owner_line_labels(segment_dir: &Path) -> OwnerLineLabels {
    let path = owner_line_labels_path(segment_dir);
    match observe_json_durable::<Value>(ArtifactId::SegmentOwnerLineLabels, &path) {
        DurableObservation::Absent => OwnerLineLabels::Absent,
        DurableObservation::Present(value) => match entries_from_value(&value) {
            Ok(entries) => OwnerLineLabels::Present(entries),
            Err(detail) => OwnerLineLabels::Unreadable(format!("{}: {detail}", path.display())),
        },
        DurableObservation::Malformed { path, source } => {
            OwnerLineLabels::Unreadable(format!("{}: {source}", path.display()))
        }
        DurableObservation::Unreadable { path, source } => {
            OwnerLineLabels::Unreadable(format!("{}: {source}", path.display()))
        }
    }
}

/// Decode the bytes of an owner line labels file. Empty bytes hold no
/// entries, as an absent file does; anything else must be a valid record.
pub fn decode_owner_line_labels(bytes: &[u8]) -> Result<Vec<OwnerLineEntry>, String> {
    if bytes.trim_ascii().is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    entries_from_value(&value)
}

/// Encode entries as the file's bytes, sorted by key.
pub fn encode_owner_line_labels(entries: &[OwnerLineEntry]) -> Vec<u8> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| left.key().cmp(&right.key()));
    let entries = sorted
        .iter()
        .map(|entry| {
            json!({
                "source": entry.source,
                "sentence_id": entry.sentence_id,
                "start": entry.fingerprint.start,
                "text_sha256": entry.fingerprint.text_sha256,
                "speaker": entry.speaker,
                "set_at": entry.set_at,
            })
        })
        .collect::<Vec<_>>();
    let value = json!({"version": OWNER_LINE_LABELS_VERSION, "entries": entries});
    let mut bytes = write_python_compatible_json(&value, 2)
        .expect("owner line labels serialize")
        .into_bytes();
    bytes.push(b'\n');
    bytes
}

fn entries_from_value(value: &Value) -> Result<Vec<OwnerLineEntry>, String> {
    let root = value.as_object().ok_or("not a JSON object")?;
    if root.get("version").and_then(Value::as_u64) != Some(OWNER_LINE_LABELS_VERSION) {
        return Err(format!(
            "version is not {OWNER_LINE_LABELS_VERSION}: {:?}",
            root.get("version")
        ));
    }
    if let Some(key) = root
        .keys()
        .find(|key| !matches!(key.as_str(), "version" | "entries"))
    {
        return Err(format!("unknown field {key:?}"));
    }
    let rows = root
        .get("entries")
        .and_then(Value::as_array)
        .ok_or("entries is not a list")?;
    let mut seen = BTreeSet::new();
    let mut entries = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let entry = entry_from_value(row).map_err(|detail| format!("entry {index}: {detail}"))?;
        if !seen.insert((entry.source.clone(), entry.sentence_id)) {
            return Err(format!("entry {index}: duplicate line"));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn entry_from_value(value: &Value) -> Result<OwnerLineEntry, String> {
    let row = value.as_object().ok_or("not an object")?;
    if let Some(key) = row.keys().find(|key| !ENTRY_FIELDS.contains(&key.as_str())) {
        return Err(format!("unknown field {key:?}"));
    }
    let text = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is not a non-empty string"))
    };
    let source = text("source")?;
    let sentence_id = row
        .get("sentence_id")
        .and_then(Value::as_i64)
        .filter(|id| *id >= 1)
        .ok_or("sentence_id is not a positive integer")?;
    let start = row.get("start").cloned().ok_or("start is missing")?;
    let text_sha256 = text("text_sha256")?;
    if text_sha256.len() != 64
        || !text_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("text_sha256 is not a SHA-256 hex digest".to_owned());
    }
    let speaker = match row.get("speaker") {
        Some(Value::Null) => None,
        Some(Value::String(id)) if !id.is_empty() => Some(id.clone()),
        _ => return Err("speaker is not an entity id or null".to_owned()),
    };
    Ok(OwnerLineEntry {
        source,
        sentence_id,
        fingerprint: LineFingerprint { start, text_sha256 },
        speaker,
        set_at: text("set_at")?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)]

    use super::*;

    fn entry(sentence_id: i64, speaker: Option<&str>) -> OwnerLineEntry {
        let record = json!({"start": "00:00:01", "text": "hello"});
        OwnerLineEntry {
            source: "audio".to_owned(),
            sentence_id,
            fingerprint: LineFingerprint::of_record(record.as_object().unwrap()),
            speaker: speaker.map(str::to_owned),
            set_at: "2026-10-31T12:00:00+00:00".to_owned(),
        }
    }

    #[test]
    fn entries_round_trip_sorted_with_unknown_speakers_kept() {
        let entries = vec![entry(3, None), entry(1, Some("ada"))];
        let decoded = decode_owner_line_labels(&encode_owner_line_labels(&entries)).unwrap();
        assert_eq!(decoded, vec![entry(1, Some("ada")), entry(3, None)]);
    }

    #[test]
    fn a_fingerprint_hashes_the_transcribed_text_and_keeps_the_start() {
        let record = json!({"start": 2.5, "text": "abc", "corrected": "abd"});
        let fingerprint = LineFingerprint::of_record(record.as_object().unwrap());
        assert_eq!(fingerprint.start, json!(2.5));
        assert_eq!(
            fingerprint.text_sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn damaged_records_are_refused_rather_than_partly_read() {
        let good = String::from_utf8(encode_owner_line_labels(&[entry(1, None)])).unwrap();
        for bad in [
            "[]".to_owned(),
            "{\"entries\":[]}".to_owned(),
            "{\"version\":2,\"entries\":[]}".to_owned(),
            "{\"version\":1,\"entries\":{}}".to_owned(),
            "{\"version\":1,\"entries\":[],\"extra\":1}".to_owned(),
            good.replace("\"sentence_id\": 1", "\"sentence_id\": 0"),
            good.replace("\"speaker\": null", "\"speaker\": 7"),
            good.replace("\"source\": \"audio\"", "\"source\": \"\""),
            good.replace("\"set_at\"", "\"when\""),
            good.replace("\"entries\": [", "\"entries\": [{\"source\":\"audio\"},"),
            good.replace("}\n  ]", "},{\"source\":\"audio\",\"sentence_id\":1,\"start\":null,\"text_sha256\":\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\"speaker\":null,\"set_at\":\"x\"}]"),
        ] {
            assert!(decode_owner_line_labels(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn reading_tells_absent_present_and_unreadable_apart_and_never_changes_the_file() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read_owner_line_labels(root.path()), OwnerLineLabels::Absent);
        let path = owner_line_labels_path(root.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, encode_owner_line_labels(&[entry(2, Some("bo"))])).unwrap();
        assert_eq!(
            read_owner_line_labels(root.path()),
            OwnerLineLabels::Present(vec![entry(2, Some("bo"))])
        );
        std::fs::write(&path, b"{\"version\":1,\"entries\":[").unwrap();
        assert!(matches!(
            read_owner_line_labels(root.path()),
            OwnerLineLabels::Unreadable(_)
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"{\"version\":1,\"entries\":["
        );
        let names = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![OWNER_LINE_LABELS_FILE]);
    }
}
