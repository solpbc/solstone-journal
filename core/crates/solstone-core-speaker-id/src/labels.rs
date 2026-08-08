// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read and write `speaker_labels.json` payloads.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, atomic_replace, hold_lock,
};

use crate::corrections::{CorrectionsError, read_corrections};
use crate::json::{JsonError, write_python_compatible_json};

const LABELS_FILE: &str = "speaker_labels.json";

/// Errors produced while reading or writing speaker labels.
#[derive(Debug)]
pub enum LabelsError {
    Lock(LockError),
    Write(AtomicWriteError),
    Serialize(JsonError),
    Corrections(CorrectionsError),
    SentenceIdNotFound(i64),
    InvalidCorrectionSentenceId,
}

impl fmt::Display for LabelsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => write!(f, "could not lock speaker labels: {error}"),
            Self::Write(error) => write!(f, "could not write speaker labels: {error}"),
            Self::Serialize(error) => write!(f, "could not serialize speaker labels: {error}"),
            Self::Corrections(error) => write!(f, "could not read speaker corrections: {error}"),
            Self::SentenceIdNotFound(sentence_id) => {
                write!(
                    f,
                    "speaker label for sentence ID {sentence_id} was not found"
                )
            }
            Self::InvalidCorrectionSentenceId => {
                write!(f, "speaker correction has an invalid sentence ID")
            }
        }
    }
}

impl Error for LabelsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::Corrections(error) => Some(error),
            Self::SentenceIdNotFound(_) | Self::InvalidCorrectionSentenceId => None,
        }
    }
}

/// Write fresh labels while preserving current user-authored labels and applying corrections.
pub fn write_full_labels(
    segment_dir: &Path,
    mut fresh_labels: Vec<Value>,
    metadata: &Map<String, Value>,
) -> Result<(), LabelsError> {
    let corrected =
        corrections_by_sentence(read_corrections(segment_dir).map_err(LabelsError::Corrections)?)?;
    let path = labels_path(segment_dir);
    let _lock = hold_lock(&path, LockOptions::default()).map_err(LabelsError::Lock)?;

    let current = read_current_labels(&path);
    let mut result = current.clone().unwrap_or_default();
    result.remove("skipped");
    result.remove("reason");

    for label in &mut fresh_labels {
        let Some(label) = label.as_object_mut() else {
            continue;
        };
        let Some(sentence_id) = label_sentence_id(label) else {
            continue;
        };
        if let Some(correction) = corrected.get(&sentence_id) {
            apply_correction_overlay(label, correction);
        }
    }

    let mut user_by_sentence = HashMap::new();
    if let Some(labels) = current
        .as_ref()
        .and_then(|current| current.get("labels"))
        .and_then(Value::as_array)
    {
        for label in labels {
            let Some(object) = label.as_object() else {
                continue;
            };
            let Some(sentence_id) = label_sentence_id(object) else {
                continue;
            };
            if is_user_label(object) {
                user_by_sentence.insert(sentence_id, label.clone());
            }
        }
    }

    let mut merged_labels = Vec::new();
    let mut fresh_sentence_ids = HashSet::new();
    for label in fresh_labels {
        let sentence_id = label.as_object().and_then(label_sentence_id);
        let Some(sentence_id) = sentence_id else {
            merged_labels.push(label);
            continue;
        };
        fresh_sentence_ids.insert(sentence_id);
        merged_labels.push(user_by_sentence.get(&sentence_id).cloned().unwrap_or(label));
    }

    let mut user_only: Vec<_> = user_by_sentence
        .into_iter()
        .filter(|(sentence_id, _)| !fresh_sentence_ids.contains(sentence_id))
        .collect();
    user_only.sort_by_key(|(sentence_id, _)| *sentence_id);
    merged_labels.extend(user_only.into_iter().map(|(_, label)| label));

    result.insert("labels".to_owned(), Value::Array(merged_labels));
    result.insert(
        "owner_centroid_last_refreshed_at".to_owned(),
        metadata
            .get("owner_centroid_last_refreshed_at")
            .cloned()
            .unwrap_or(Value::Null),
    );
    result.insert(
        "voiceprint_versions".to_owned(),
        metadata
            .get("voiceprint_versions")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    result.insert(
        "candidate_evidence".to_owned(),
        metadata
            .get("candidate_evidence")
            .filter(|value| py_truthy(value))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    if let Some(gaps) = metadata
        .get("candidate_evidence_gaps")
        .filter(|value| py_truthy(value))
    {
        result.insert("candidate_evidence_gaps".to_owned(), gaps.clone());
    } else {
        result.remove("candidate_evidence_gaps");
    }

    write_labels(&path, Value::Object(result))
}

/// Write a skipped-labels payload, replacing any existing label contents.
pub fn write_stub_labels(segment_dir: &Path, reason: &str) -> Result<(), LabelsError> {
    let path = labels_path(segment_dir);
    let _lock = hold_lock(&path, LockOptions::default()).map_err(LabelsError::Lock)?;

    let mut result = Map::new();
    result.insert("labels".to_owned(), Value::Array(Vec::new()));
    result.insert("skipped".to_owned(), Value::Bool(true));
    result.insert("reason".to_owned(), Value::String(reason.to_owned()));
    write_labels(&path, Value::Object(result))
}

/// Apply sentence-ID-addressed label patches to a segment's labels.
pub fn patch_labels(
    segment_dir: &Path,
    patches: &[(i64, Map<String, Value>)],
    allow_insert: bool,
) -> Result<(), LabelsError> {
    let path = labels_path(segment_dir);
    let _lock = hold_lock(&path, LockOptions::default()).map_err(LabelsError::Lock)?;

    let mut base = read_current_labels(&path).unwrap_or_default();
    let mut labels = base
        .get("labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut labels_by_sentence = HashMap::new();
    for (index, label) in labels.iter().enumerate() {
        if let Some(sentence_id) = label.as_object().and_then(label_sentence_id) {
            labels_by_sentence.insert(sentence_id, index);
        }
    }

    let mut deduplicated = Vec::new();
    for (sentence_id, fields) in patches {
        if let Some((_, existing_fields)) = deduplicated
            .iter_mut()
            .find(|(existing_sentence_id, _)| existing_sentence_id == sentence_id)
        {
            *existing_fields = fields.clone();
        } else {
            deduplicated.push((*sentence_id, fields.clone()));
        }
    }

    for (sentence_id, fields) in deduplicated {
        if let Some(index) = labels_by_sentence.get(&sentence_id).copied() {
            if let Some(label) = labels[index].as_object_mut() {
                for (key, value) in fields {
                    label.insert(key, value);
                }
            }
        } else if !allow_insert {
            return Err(LabelsError::SentenceIdNotFound(sentence_id));
        } else {
            let mut label = Map::new();
            label.insert("sentence_id".to_owned(), Value::from(sentence_id));
            for (key, value) in fields {
                label.insert(key, value);
            }
            labels_by_sentence.insert(sentence_id, labels.len());
            labels.push(Value::Object(label));
        }
    }

    if allow_insert {
        // Match restore-style ordering: valid sentence IDs first, sid-less rows last.
        labels.sort_by_key(|label| {
            let sentence_id = label.as_object().and_then(label_sentence_id);
            (sentence_id.is_none(), sentence_id.unwrap_or(0))
        });
    }

    base.insert("labels".to_owned(), Value::Array(labels));
    write_labels(&path, Value::Object(base))
}

fn labels_path(segment_dir: &Path) -> PathBuf {
    segment_dir.join("talents").join(LABELS_FILE)
}

fn read_current_labels(path: &Path) -> Option<Map<String, Value>> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
}

fn corrections_by_sentence(
    corrections: Vec<Value>,
) -> Result<HashMap<i64, Map<String, Value>>, LabelsError> {
    let mut corrected = HashMap::new();
    for correction in corrections {
        let correction = correction
            .as_object()
            .ok_or(LabelsError::InvalidCorrectionSentenceId)?;
        let Some(sentence_id) = correction
            .get("sentence_id")
            .filter(|value| !value.is_null())
        else {
            continue;
        };
        corrected.insert(
            coerce_correction_sentence_id(sentence_id)?,
            correction.clone(),
        );
    }
    Ok(corrected)
}

fn label_sentence_id(label: &Map<String, Value>) -> Option<i64> {
    match label.get("sentence_id") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64)),
        Some(Value::String(value)) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn coerce_correction_sentence_id(value: &Value) -> Result<i64, LabelsError> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .ok_or(LabelsError::InvalidCorrectionSentenceId),
        Value::String(value) => value
            .trim()
            .parse::<i64>()
            .map_err(|_| LabelsError::InvalidCorrectionSentenceId),
        _ => Err(LabelsError::InvalidCorrectionSentenceId),
    }
}

fn is_user_label(label: &Map<String, Value>) -> bool {
    matches!(label.get("method"), Some(Value::String(method)) if method.starts_with("user_"))
}

fn apply_correction_overlay(label: &mut Map<String, Value>, correction: &Map<String, Value>) {
    let corrected_speaker = get_present(correction, "corrected_speaker");
    match corrected_speaker {
        None => {
            let is_undo = matches!(correction.get("correction_kind"), Some(Value::String(kind)) if kind == "identify_undo");
            if is_undo {
                label.insert("speaker".to_owned(), Value::Null);
                label.insert("confidence".to_owned(), Value::Null);
                label.insert("method".to_owned(), Value::Null);
            }
        }
        Some(speaker) => {
            label.insert("speaker".to_owned(), speaker.clone());
            label.insert("confidence".to_owned(), Value::String("high".to_owned()));
            let method = match get_present(correction, "original_speaker") {
                Some(original) if original == speaker => "user_confirmed",
                None => "user_assigned",
                Some(_) => "user_corrected",
            };
            label.insert("method".to_owned(), Value::String(method.to_owned()));
        }
    }
}

fn get_present<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !value.is_null())
}

fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn write_labels(path: &Path, value: Value) -> Result<(), LabelsError> {
    let mut bytes = write_python_compatible_json(&value, 2)
        .map_err(LabelsError::Serialize)?
        .into_bytes();
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(LabelsError::Write)
}
