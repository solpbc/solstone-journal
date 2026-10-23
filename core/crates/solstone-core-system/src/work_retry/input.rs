// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::Unit;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

mod bounded;
pub use bounded::{ActivityInputSnapshot, activity_input_bounded};

pub fn current(journal: &Path, unit: &Unit) -> Result<String, String> {
    unit.validate()?;
    match unit {
        // Sense reads the recording itself, and may never have written its output.
        Unit::Segment { day, talent, .. } if talent == "sense" => {
            crate::catchup::read_segment_input_fingerprint(
                journal,
                day,
                &unit.segment_directory(journal)?,
            )
            .map_err(|e| e.to_string())
        }
        Unit::Segment { .. } => {
            let path = unit.segment_directory(journal)?.join("talents/sense.json");
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            Ok(format!("{:x}", Sha256::digest(bytes)))
        }
        Unit::Activity {
            day,
            facet,
            activity,
            ..
        } => {
            let record = solstone_core_facets::get_activity_record(journal, facet, day, activity)
                .map_err(|e| e.to_string())?
                .ok_or("activity is missing")?;
            activity_hash(journal, day, &record).ok_or("activity input cannot be read".into())
        }
        _ => Err("not a generic talent retry".into()),
    }
}

pub fn activity_hash(journal: &Path, day: &str, record: &Map<String, Value>) -> Option<String> {
    activity_hash_layout(journal, day, record, false)
}

/// Only used to recognize legacy provenance without treating a hashing fix as
/// changed owner input. New work always uses the layout-aware fingerprint.
pub fn legacy_activity_hash(
    journal: &Path,
    day: &str,
    record: &Map<String, Value>,
) -> Option<String> {
    activity_hash_layout(journal, day, record, true)
}

fn activity_hash_layout(
    journal: &Path,
    day: &str,
    record: &Map<String, Value>,
    legacy: bool,
) -> Option<String> {
    let spans = record
        .get("segments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let day_dir = journal.join("chronicle").join(day);
    let only_stream = record.get("stream").and_then(Value::as_str);
    let mut inputs = Vec::new();
    for segment in &spans {
        let mut sense = Vec::new();
        let mut entries = match std::fs::read_dir(&day_dir) {
            Ok(entries) => entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
                .ok()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return None,
        };
        entries.sort();
        for direct in entries {
            let name = direct.file_name().and_then(|name| name.to_str());
            if legacy && only_stream.is_some_and(|stream| name != Some(stream)) {
                continue;
            }
            let metadata = match std::fs::metadata(&direct) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return None,
            };
            if !metadata.is_dir() {
                continue;
            }
            let segment_dir = if name == Some(segment) {
                if !legacy
                    && only_stream
                        .is_some_and(|stream| stream != solstone_core_journal_io::DEFAULT_STREAM)
                {
                    continue;
                }
                direct
            } else {
                if only_stream.is_some_and(|stream| name != Some(stream)) {
                    continue;
                }
                let nested = direct.join(segment);
                match std::fs::metadata(&nested) {
                    Ok(metadata) if metadata.is_dir() => {}
                    Ok(_) => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => return None,
                };
                nested
            };
            let path = segment_dir.join("talents/sense.json");
            let relative = path.strip_prefix(journal).ok()?.display().to_string();
            match std::fs::read(&path) {
                Ok(bytes) => sense.push(serde_json::json!({"path":relative,"sha256":format!("{:x}", Sha256::digest(&bytes)),"size":bytes.len()})),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => sense.push(serde_json::json!({"path":relative,"missing":true})),
                Err(_) => return None,
            }
        }
        if sense.is_empty() {
            sense.push(serde_json::json!({"segment":segment,"missing":true}));
        }
        inputs.push(serde_json::json!({"segment":segment,"sense":sense}));
    }
    activity_identity_hash(record, &spans, inputs)
}

fn activity_identity_hash(
    record: &Map<String, Value>,
    spans: &[String],
    inputs: Vec<Value>,
) -> Option<String> {
    let identity = serde_json::json!({
        "activity_id": record.get("id"), "activity": record.get("activity"),
        "facet": record.get("facet"), "segments": spans, "segment_inputs": inputs,
    });
    serde_json::to_string(&canonical_json(identity))
        .ok()
        .map(|text| format!("{:x}", Sha256::digest(text.as_bytes())))
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        primitive => primitive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_layout_tracks_sense_and_keeps_legacy_hash_separate() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("chronicle/20260101/120000_300/talents/sense.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "first").unwrap();
        let activity = serde_json::json!({"id":"work_120000", "activity":"work", "stream":"_default", "segments":["120000_300"]});
        let activity = activity.as_object().unwrap();
        let first = activity_hash(root.path(), "20260101", activity).unwrap();
        let legacy = legacy_activity_hash(root.path(), "20260101", activity).unwrap();
        std::fs::write(&path, "second").unwrap();
        assert_ne!(
            first,
            activity_hash(root.path(), "20260101", activity).unwrap()
        );
        assert_eq!(
            legacy,
            legacy_activity_hash(root.path(), "20260101", activity).unwrap()
        );
        // A directory where the expected file lived is unreadable evidence,
        // never an input-change grant.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(activity_hash(root.path(), "20260101", activity).is_none());
    }
}
