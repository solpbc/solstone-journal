// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stateless provisional owner-centroid resolution from manual speaker labels.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::segment_path;
use serde_json::Value;
use solstone_core_entity::{
    EntityLifecycleError, VoiceprintArchive, entity_memory_path, normalize_embedding,
    try_load_entity_voiceprints_file,
};
use solstone_core_journal_io::day_path;
use solstone_core_speaker_id::calibration::{
    NOISY_FLYWHEEL_OVERLAP_MAX, OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS,
};
use solstone_core_speaker_id::embeddings::{EmbeddingsFile, load_embeddings_file};

use crate::owner_admission::{OwnerAdmission, admitted_owner_id};
use crate::owner_centroid::{OwnerCentroid, OwnerCentroidError, load_owner_centroid};

const MANUAL_OWNER_METHODS: [&str; 3] = ["user_assigned", "user_corrected", "user_confirmed"];

/// Terminal and continuation reasons for owner-tier resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerTierReason {
    ConfirmedAbsent,
    ConfirmedUnreadable,
    ConfirmedIncomplete,
    ConfirmedZeroNorm,
    VoiceprintsAbsent,
    VoiceprintsUnreadable,
    BelowRowFloor,
    BelowEmbeddingFloor,
    ProvisionalZeroNorm,
}

impl OwnerTierReason {
    pub const ALL: [Self; 9] = [
        Self::ConfirmedAbsent,
        Self::ConfirmedUnreadable,
        Self::ConfirmedIncomplete,
        Self::ConfirmedZeroNorm,
        Self::VoiceprintsAbsent,
        Self::VoiceprintsUnreadable,
        Self::BelowRowFloor,
        Self::BelowEmbeddingFloor,
        Self::ProvisionalZeroNorm,
    ];
}

/// The resolved confirmed or provisional owner tier, or why neither applies.
#[derive(Debug, Clone, PartialEq)]
pub enum OwnerTierOutcome {
    Confirmed(OwnerCentroid),
    Provisional(Vec<f32>),
    IdentityInvalid,
    None(OwnerTierReason),
}

/// Unrecoverable setup failure while resolving an owner tier.
#[derive(Debug)]
pub enum OwnerProvisionalError {
    EntityPath(EntityLifecycleError),
}

impl fmt::Display for OwnerProvisionalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityPath(error) => error.fmt(formatter),
        }
    }
}

impl Error for OwnerProvisionalError {}

#[derive(Debug)]
struct ManualTagCandidate {
    day: String,
    stream: Option<String>,
    segment_key: String,
    source: String,
    sentence_id: i64,
    added_at: i64,
    index: usize,
}

#[derive(Debug)]
struct ResolvedManualTag {
    day: String,
    stream: String,
    segment_key: String,
    source: String,
    sentence_id: i64,
    segment_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManualTagRow {
    day: String,
    stream: String,
    segment_key: String,
    source: String,
    sentence_id: i64,
    method: String,
    segment_dir: PathBuf,
    jsonl_path: PathBuf,
}

/// One validated manual owner tag together with its source embedding.
///
/// The HTTP owner-write surface uses this same resolution policy for centroid
/// build and rebuild; keeping it here prevents its admission rules drifting
/// from provisional-owner resolution.
#[derive(Debug, Clone)]
pub struct ManualOwnerEmbedding {
    pub embedding: Vec<f32>,
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub source: String,
    pub sentence_id: i64,
    pub jsonl_path: PathBuf,
}

#[derive(Debug)]
struct LabelMatch {
    speaker: Option<String>,
    method: Option<String>,
}

enum ConfirmedGate {
    Absent,
    Resolved(OwnerCentroid),
    Suppressed(OwnerTierReason),
}

/// Resolve the journal owner's confirmed or provisional centroid tier.
pub fn resolve_owner_tier(journal_root: &Path) -> Result<OwnerTierOutcome, OwnerProvisionalError> {
    let principal_id = match admitted_owner_id(journal_root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => return Ok(OwnerTierOutcome::IdentityInvalid),
    };

    match evaluate_confirmed_tier(journal_root, &principal_id)? {
        ConfirmedGate::Resolved(centroid) => return Ok(OwnerTierOutcome::Confirmed(centroid)),
        ConfirmedGate::Suppressed(reason) => return Ok(OwnerTierOutcome::None(reason)),
        ConfirmedGate::Absent => {}
    }

    let directory = entity_memory_path(journal_root, &principal_id, false)
        .map_err(OwnerProvisionalError::EntityPath)?;
    let voiceprints_path = directory.join("voiceprints.npz");
    if !voiceprints_path.exists() {
        return Ok(OwnerTierOutcome::None(OwnerTierReason::VoiceprintsAbsent));
    }

    let archive = match try_load_entity_voiceprints_file(journal_root, &principal_id) {
        Ok(Some(archive)) => archive,
        Ok(None) => return Ok(OwnerTierOutcome::None(OwnerTierReason::VoiceprintsAbsent)),
        Err(_) => {
            return Ok(OwnerTierOutcome::None(
                OwnerTierReason::VoiceprintsUnreadable,
            ));
        }
    };
    let rows = collect_manual_tag_rows(journal_root, &principal_id, &archive);
    if rows.len() < OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS {
        return Ok(OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor));
    }

    let embeddings = resolve_embeddings(&rows);
    if embeddings.len() < OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS {
        return Ok(OwnerTierOutcome::None(OwnerTierReason::BelowEmbeddingFloor));
    }
    let centroid = mean_embedding(&embeddings);
    let Some(centroid) = normalize_embedding(&centroid) else {
        return Ok(OwnerTierOutcome::None(OwnerTierReason::ProvisionalZeroNorm));
    };
    Ok(OwnerTierOutcome::Provisional(centroid))
}

/// Resolve the validated manual owner-tag embeddings used by owner build and
/// rebuild. Missing or unreadable voiceprints produce no usable evidence, just
/// as the provisional tier does.
pub fn collect_manual_owner_embeddings(
    journal_root: &Path,
    principal_id: &str,
) -> Result<Vec<ManualOwnerEmbedding>, OwnerProvisionalError> {
    let directory = entity_memory_path(journal_root, principal_id, false)
        .map_err(OwnerProvisionalError::EntityPath)?;
    if !directory.join("voiceprints.npz").exists() {
        return Ok(Vec::new());
    }
    let Ok(Some(archive)) = try_load_entity_voiceprints_file(journal_root, principal_id) else {
        return Ok(Vec::new());
    };
    let mut cache = HashMap::<PathBuf, Option<EmbeddingsFile>>::new();
    Ok(
        collect_manual_tag_rows(journal_root, principal_id, &archive)
            .into_iter()
            .filter_map(|row| {
                let path = row.segment_dir.join(format!("{}.npz", row.source));
                let embeddings = cache
                    .entry(path.clone())
                    .or_insert_with(|| load_embeddings_file(&path).ok().flatten())
                    .as_ref()?;
                let embedding = embeddings
                    .statements
                    .iter()
                    .find(|(statement_id, _)| *statement_id == row.sentence_id)
                    .map(|(_, embedding)| embedding.clone())?;
                Some(ManualOwnerEmbedding {
                    embedding,
                    day: row.day,
                    stream: row.stream,
                    segment_key: row.segment_key,
                    source: row.source,
                    sentence_id: row.sentence_id,
                    jsonl_path: row.jsonl_path,
                })
            })
            .collect(),
    )
}

fn evaluate_confirmed_tier(
    journal_root: &Path,
    principal_id: &str,
) -> Result<ConfirmedGate, OwnerProvisionalError> {
    let directory = entity_memory_path(journal_root, principal_id, false)
        .map_err(OwnerProvisionalError::EntityPath)?;
    if !directory.join("owner_centroid.npz").exists() {
        return Ok(ConfirmedGate::Absent);
    }
    let gate = match load_owner_centroid(journal_root, principal_id) {
        Ok(Some(centroid)) => ConfirmedGate::Resolved(centroid),
        Ok(None) => ConfirmedGate::Suppressed(OwnerTierReason::ConfirmedZeroNorm),
        Err(OwnerCentroidError::MissingRequiredMember(_)) => {
            ConfirmedGate::Suppressed(OwnerTierReason::ConfirmedIncomplete)
        }
        Err(_) => ConfirmedGate::Suppressed(OwnerTierReason::ConfirmedUnreadable),
    };
    Ok(gate)
}

fn parse_python_int(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(value) => Some(i64::from(*value)),
        Value::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| {
                let value = value.as_f64()?;
                (value.is_finite() && value >= i64::MIN as f64 && value < i64::MAX as f64)
                    .then_some(value.trunc() as i64)
            }),
        Value::String(value) => {
            let value = value.trim();
            let bytes = value.as_bytes();
            if bytes.contains(&b'_') {
                for (index, byte) in bytes.iter().enumerate() {
                    if *byte == b'_'
                        && (index == 0
                            || index + 1 == bytes.len()
                            || !bytes[index - 1].is_ascii_digit()
                            || !bytes[index + 1].is_ascii_digit())
                    {
                        return None;
                    }
                }
                value.replace('_', "").parse().ok()
            } else {
                value.parse().ok()
            }
        }
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn parse_voiceprint_candidates(metadata: &[String]) -> Vec<ManualTagCandidate> {
    metadata
        .iter()
        .enumerate()
        .filter_map(|(index, raw)| {
            let row = serde_json::from_str::<Value>(raw).ok()?;
            let row = row.as_object()?;
            let day = row.get("day")?.as_str()?.to_owned();
            let segment_key = row.get("segment_key")?.as_str()?.to_owned();
            let source = row.get("source")?.as_str()?.to_owned();
            let sentence_id = parse_python_int(row.get("sentence_id")?)?;
            let added_at = row.get("added_at").and_then(parse_python_int).unwrap_or(-1);
            let stream = row
                .get("stream")
                .and_then(Value::as_str)
                .filter(|stream| !stream.is_empty())
                .map(ToOwned::to_owned);
            Some(ManualTagCandidate {
                day,
                stream,
                segment_key,
                source,
                sentence_id,
                added_at,
                index,
            })
        })
        .collect()
}

fn dedupe_candidates(candidates: Vec<ManualTagCandidate>) -> Vec<ManualTagCandidate> {
    let mut latest = HashMap::new();
    for candidate in candidates {
        let key = (
            candidate.day.clone(),
            candidate.segment_key.clone(),
            candidate.source.clone(),
            candidate.sentence_id,
        );
        let replace = latest.get(&key).is_none_or(|current: &ManualTagCandidate| {
            (candidate.added_at, candidate.index) > (current.added_at, current.index)
        });
        if replace {
            latest.insert(key, candidate);
        }
    }
    let mut candidates = latest.into_values().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (
            &left.day,
            left.stream.as_deref().unwrap_or_default(),
            &left.segment_key,
            &left.source,
            left.sentence_id,
        )
            .cmp(&(
                &right.day,
                right.stream.as_deref().unwrap_or_default(),
                &right.segment_key,
                &right.source,
                right.sentence_id,
            ))
    });
    candidates
}

fn resolve_segment(
    journal_root: &Path,
    candidate: ManualTagCandidate,
) -> Option<ResolvedManualTag> {
    let (stream, segment_dir) = match candidate.stream {
        Some(stream) => {
            let segment_dir = segment_path(
                journal_root,
                &candidate.day,
                &candidate.segment_key,
                &stream,
                false,
            )
            .ok()?;
            segment_dir.is_dir().then_some((stream, segment_dir))?
        }
        None => {
            let day_dir = day_path(journal_root, Some(&candidate.day), false).ok()?;
            let matches = fs::read_dir(day_dir)
                .ok()?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let stream = entry.file_name().to_str()?.to_owned();
                    let segment_dir = entry.path().join(&candidate.segment_key);
                    segment_dir.is_dir().then_some((stream, segment_dir))
                })
                .collect::<Vec<_>>();
            (matches.len() == 1).then(|| matches.into_iter().next())??
        }
    };
    Some(ResolvedManualTag {
        day: candidate.day,
        stream,
        segment_key: candidate.segment_key,
        source: candidate.source,
        sentence_id: candidate.sentence_id,
        segment_dir,
    })
}

fn read_matching_label(segment_dir: &Path, sentence_id: i64) -> Option<LabelMatch> {
    let bytes = fs::read(segment_dir.join("talents/speaker_labels.json")).ok()?;
    let labels = serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("labels")?
        .as_array()?
        .clone();
    for label in labels {
        let Some(label) = label.as_object() else {
            continue;
        };
        let label_sentence_id = match label.get("sentence_id") {
            Some(value) => match parse_python_int(value) {
                Some(value) => value,
                None => continue,
            },
            None => -1,
        };
        if label_sentence_id == sentence_id {
            return Some(LabelMatch {
                speaker: label
                    .get("speaker")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                method: label
                    .get("method")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            });
        }
    }
    None
}

fn validate_label_and_overlap(
    principal_id: &str,
    resolved: ResolvedManualTag,
) -> Option<ManualTagRow> {
    let label = read_matching_label(&resolved.segment_dir, resolved.sentence_id)?;
    let method = label.method?;
    if label.speaker.as_deref() != Some(principal_id)
        || !MANUAL_OWNER_METHODS.contains(&method.as_str())
    {
        return None;
    }
    let jsonl_path = resolved
        .segment_dir
        .join(format!("{}.jsonl", resolved.source));
    if read_overlap_fraction(&jsonl_path) > f64::from(NOISY_FLYWHEEL_OVERLAP_MAX) {
        return None;
    }
    Some(ManualTagRow {
        day: resolved.day,
        stream: resolved.stream,
        segment_key: resolved.segment_key,
        source: resolved.source,
        sentence_id: resolved.sentence_id,
        method,
        segment_dir: resolved.segment_dir,
        jsonl_path,
    })
}

fn collect_manual_tag_rows(
    journal_root: &Path,
    principal_id: &str,
    archive: &VoiceprintArchive,
) -> Vec<ManualTagRow> {
    dedupe_candidates(parse_voiceprint_candidates(&archive.metadata))
        .into_iter()
        .filter_map(|candidate| resolve_segment(journal_root, candidate))
        .filter_map(|resolved| validate_label_and_overlap(principal_id, resolved))
        .collect()
}

fn read_overlap_fraction(path: &Path) -> f64 {
    let Ok(contents) = fs::read_to_string(path) else {
        return 0.0;
    };
    let Some(line) = contents.lines().next() else {
        return 0.0;
    };
    let Ok(header) = serde_json::from_str::<Value>(line) else {
        return 0.0;
    };
    header
        .get("overlap_fraction")
        .and_then(parse_python_float)
        .unwrap_or(0.0)
}

fn parse_python_float(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(value) => Some(f64::from(*value)),
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn resolve_embeddings(rows: &[ManualTagRow]) -> Vec<Vec<f32>> {
    let mut cache = HashMap::<PathBuf, Option<EmbeddingsFile>>::new();
    rows.iter()
        .filter_map(|row| {
            let path = row.segment_dir.join(format!("{}.npz", row.source));
            let embeddings = cache
                .entry(path.clone())
                .or_insert_with(|| load_embeddings_file(&path).ok().flatten())
                .as_ref()?;
            embeddings
                .statements
                .iter()
                .find(|(statement_id, _)| *statement_id == row.sentence_id)
                .map(|(_, embedding)| embedding.clone())
        })
        .collect()
}

fn mean_embedding(embeddings: &[Vec<f32>]) -> Vec<f32> {
    let mut mean = vec![0.0; embeddings[0].len()];
    for embedding in embeddings {
        for (mean_value, value) in mean.iter_mut().zip(embedding) {
            *mean_value += value;
        }
    }
    let count = embeddings.len() as f32;
    mean.iter_mut().for_each(|value| *value /= count);
    mean
}
