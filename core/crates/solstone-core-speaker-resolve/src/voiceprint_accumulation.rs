// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native high-confidence voiceprint accumulation.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::segment_path;
use chrono::{NaiveDate, TimeZone};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_entity::{
    EncoderIdentity, VoiceprintItem, is_admissible_person, load_all_journal_entities,
    load_entity_voiceprints_file, normalize_embedding, save_voiceprints_batch,
};
use solstone_core_journal_config::read_journal_config;
use solstone_core_speaker_id::calibration::{
    NOISY_FLYWHEEL_OVERLAP_MAX, VP_OUTLIER_MIN_SAMPLES, VP_OUTLIER_MIN_SIMILARITY,
};

use crate::owner_admission::{OwnerAdmission, admitted_owner_id};
use crate::owner_centroid::{OwnerCentroidError, load_owner_centroid};

const METHODS: [&str; 4] = [
    "structural_single_speaker",
    "structural_setting",
    "acoustic",
    "acoustic_cluster",
];

#[derive(Debug, Clone, PartialEq)]
pub struct AccumulationRequest {
    pub journal_root: std::path::PathBuf,
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub source: String,
    pub now_ms: i64,
    pub encoder: EncoderIdentity,
    pub labels: Vec<AccumulationLabel>,
    pub embeddings: Vec<AccumulationEmbedding>,
    pub entity_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccumulationLabel {
    pub sentence_id: i64,
    pub speaker: Option<String>,
    pub confidence: Option<String>,
    pub method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccumulationEmbedding {
    pub sentence_id: i64,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum AccumulationSkipReason {
    NoisyOverlap,
    LowConfidence,
    UnsupportedMethod,
    OwnerEntity,
    UnknownEntity,
    MissingEmbedding,
    ZeroNormEmbedding,
    OwnerContamination,
    ExistingVoiceprint,
    DuplicateInBatch,
    Outlier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EntityWriteStatus {
    NotAttempted,
    Written,
    Failed { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntityAccumulationReport {
    pub candidate_rows: usize,
    pub written_rows: usize,
    pub skipped_rows: BTreeMap<AccumulationSkipReason, usize>,
    pub write_status: EntityWriteStatus,
}

impl Default for EntityAccumulationReport {
    fn default() -> Self {
        Self {
            candidate_rows: 0,
            written_rows: 0,
            skipped_rows: BTreeMap::new(),
            write_status: EntityWriteStatus::NotAttempted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum AccumulationOutcome {
    IdentityInvalid {
        entity_reports: BTreeMap<String, EntityAccumulationReport>,
    },
    NoOwnerCentroid {
        entity_reports: BTreeMap<String, EntityAccumulationReport>,
    },
    NothingEligible {
        skipped_rows: BTreeMap<AccumulationSkipReason, usize>,
        entity_reports: BTreeMap<String, EntityAccumulationReport>,
    },
    Completed {
        written_rows: usize,
        written_entities: usize,
        skipped_rows: BTreeMap<AccumulationSkipReason, usize>,
        entity_reports: BTreeMap<String, EntityAccumulationReport>,
    },
}

#[derive(Debug)]
pub enum AccumulationError {
    Invalid(String),
    Owner(crate::owner_centroid::OwnerCentroidError),
    Entity(solstone_core_entity::EntityStoreError),
    Path(solstone_core_journal_io::PathError),
    Config(solstone_core_journal_config::ConfigLoadError),
}

impl fmt::Display for AccumulationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(detail) => formatter.write_str(detail),
            Self::Owner(error) => error.fmt(formatter),
            Self::Entity(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Config(error) => error.fmt(formatter),
        }
    }
}
impl Error for AccumulationError {}

/// Validate and append eligible high-confidence voiceprints for one segment.
pub fn accumulate_voiceprints(
    request: &AccumulationRequest,
) -> Result<AccumulationOutcome, AccumulationError> {
    validate_request(request)?;
    let owner_id = match admitted_owner_id(&request.journal_root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => {
            return Ok(AccumulationOutcome::IdentityInvalid {
                entity_reports: BTreeMap::new(),
            });
        }
    };
    let owner = match load_owner_centroid(&request.journal_root, &owner_id) {
        Ok(Some(owner)) => owner,
        Ok(None) => {
            return Ok(AccumulationOutcome::NoOwnerCentroid {
                entity_reports: BTreeMap::new(),
            });
        }
        Err(OwnerCentroidError::IdentityInvalid | OwnerCentroidError::TargetMismatch { .. }) => {
            return Ok(AccumulationOutcome::IdentityInvalid {
                entity_reports: BTreeMap::new(),
            });
        }
        Err(error) => return Err(AccumulationError::Owner(error)),
    };
    let mut skipped = BTreeMap::new();
    let mut reports = BTreeMap::new();
    let segment_dir = segment_path(
        &request.journal_root,
        &request.day,
        &request.segment_key,
        &request.stream,
        false,
    )
    .map_err(AccumulationError::Path)?;
    let overlap = read_overlap_fraction(&segment_dir.join(format!("{}.jsonl", request.source)));
    if overlap > NOISY_FLYWHEEL_OVERLAP_MAX {
        increment(&mut skipped, AccumulationSkipReason::NoisyOverlap);
        return Ok(AccumulationOutcome::NothingEligible {
            skipped_rows: skipped,
            entity_reports: reports,
        });
    }
    let last_seen_ts =
        segment_start_ts_ms(&request.journal_root, &request.day, &request.segment_key)?;
    let embeddings = request
        .embeddings
        .iter()
        .map(|row| (row.sentence_id, &row.values))
        .collect::<HashMap<_, _>>();
    let allowed = request.entity_ids.iter().cloned().collect::<HashSet<_>>();
    let entities =
        load_all_journal_entities(&request.journal_root).map_err(AccumulationError::Entity)?;
    let entity_types = entities
        .iter()
        .map(|entity| (entity.id.as_str(), entity))
        .collect::<HashMap<_, _>>();
    let mut existing = HashMap::<String, ExistingVoiceprints>::new();
    let mut pending = BTreeMap::<String, Vec<VoiceprintItem>>::new();
    let mut pending_keys = HashSet::<(String, String, String, String, i64)>::new();

    for label in &request.labels {
        if label.confidence.as_deref() != Some("high") {
            increment(&mut skipped, AccumulationSkipReason::LowConfidence);
            continue;
        }
        if !label
            .method
            .as_deref()
            .is_some_and(|method| METHODS.contains(&method))
        {
            increment(&mut skipped, AccumulationSkipReason::UnsupportedMethod);
            continue;
        }
        let Some(speaker) = label
            .speaker
            .as_deref()
            .filter(|speaker| !speaker.is_empty())
        else {
            increment(&mut skipped, AccumulationSkipReason::UnknownEntity);
            continue;
        };
        if speaker == owner_id {
            increment(&mut skipped, AccumulationSkipReason::OwnerEntity);
            continue;
        }
        if !allowed.contains(speaker) {
            increment(&mut skipped, AccumulationSkipReason::UnknownEntity);
            continue;
        }
        let report = reports.entry(speaker.to_owned()).or_default();
        report.candidate_rows += 1;
        let Some(values) = embeddings.get(&label.sentence_id) else {
            skip_entity(
                report,
                &mut skipped,
                AccumulationSkipReason::MissingEmbedding,
            );
            continue;
        };
        let Some(normalized) = normalize_embedding(values) else {
            skip_entity(
                report,
                &mut skipped,
                AccumulationSkipReason::ZeroNormEmbedding,
            );
            continue;
        };
        if dot(&normalized, &owner.centroid) >= owner.threshold {
            skip_entity(
                report,
                &mut skipped,
                AccumulationSkipReason::OwnerContamination,
            );
            continue;
        }
        let state = existing
            .entry(speaker.to_owned())
            .or_insert_with(|| ExistingVoiceprints::load(&request.journal_root, speaker));
        let key = (
            request.day.clone(),
            request.segment_key.clone(),
            request.source.clone(),
            label.sentence_id,
        );
        if state.keys.contains(&key) {
            skip_entity(
                report,
                &mut skipped,
                AccumulationSkipReason::ExistingVoiceprint,
            );
            continue;
        }
        if pending_keys.contains(&(
            speaker.to_owned(),
            request.day.clone(),
            request.segment_key.clone(),
            request.source.clone(),
            label.sentence_id,
        )) {
            skip_entity(
                report,
                &mut skipped,
                AccumulationSkipReason::DuplicateInBatch,
            );
            continue;
        }
        if state.count >= VP_OUTLIER_MIN_SAMPLES
            && state
                .centroid
                .as_ref()
                .is_some_and(|centroid| dot(&normalized, centroid) < VP_OUTLIER_MIN_SIMILARITY)
        {
            skip_entity(report, &mut skipped, AccumulationSkipReason::Outlier);
            continue;
        }
        pending_keys.insert((
            speaker.to_owned(),
            request.day.clone(),
            request.segment_key.clone(),
            request.source.clone(),
            label.sentence_id,
        ));
        pending
            .entry(speaker.to_owned())
            .or_default()
            .push(VoiceprintItem {
                embedding: normalized,
                metadata: json!({
                    "day": request.day,
                    "segment_key": request.segment_key,
                    "source": request.source,
                    "stream": request.stream,
                    "sentence_id": label.sentence_id,
                    "added_at": request.now_ms,
                    "last_seen_ts": last_seen_ts,
                }),
            });
    }
    let mut written_rows = 0;
    let mut written_entities = 0;
    for (entity_id, items) in pending {
        let report = reports.entry(entity_id.clone()).or_default();
        let Some(entity) = entity_types.get(entity_id.as_str()) else {
            skip_entity(report, &mut skipped, AccumulationSkipReason::UnknownEntity);
            continue;
        };
        if !is_admissible_person(entity) {
            skip_entity(report, &mut skipped, AccumulationSkipReason::UnknownEntity);
            continue;
        }
        match save_voiceprints_batch(&request.journal_root, &entity_id, &items, &request.encoder) {
            Ok(count) => {
                report.written_rows = count;
                report.write_status = EntityWriteStatus::Written;
                written_rows += count;
                written_entities += 1;
            }
            Err(error) => {
                report.write_status = EntityWriteStatus::Failed {
                    detail: error.to_string(),
                }
            }
        }
    }
    if written_rows == 0 {
        Ok(AccumulationOutcome::NothingEligible {
            skipped_rows: skipped,
            entity_reports: reports,
        })
    } else {
        Ok(AccumulationOutcome::Completed {
            written_rows,
            written_entities,
            skipped_rows: skipped,
            entity_reports: reports,
        })
    }
}

struct ExistingVoiceprints {
    count: usize,
    keys: HashSet<(String, String, String, i64)>,
    centroid: Option<Vec<f32>>,
}

impl ExistingVoiceprints {
    fn load(journal_root: &Path, entity_id: &str) -> Self {
        let Some(archive) = load_entity_voiceprints_file(journal_root, entity_id) else {
            return Self {
                count: 0,
                keys: HashSet::new(),
                centroid: None,
            };
        };
        let keys = archive
            .metadata
            .iter()
            .filter_map(|metadata| {
                let value = serde_json::from_str::<Value>(metadata).ok()?;
                Some((
                    value.get("day")?.as_str()?.to_owned(),
                    value.get("segment_key")?.as_str()?.to_owned(),
                    value.get("source")?.as_str()?.to_owned(),
                    value.get("sentence_id")?.as_i64()?,
                ))
            })
            .collect();
        let normalized = archive
            .embeddings
            .chunks_exact(256)
            .filter_map(normalize_embedding)
            .collect::<Vec<_>>();
        Self {
            count: archive.rows,
            keys,
            centroid: plain_mean_normalized(&normalized),
        }
    }
}

fn plain_mean_normalized(rows: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = rows.first()?;
    let mut mean = vec![0.0; first.len()];
    for row in rows {
        for (sum, value) in mean.iter_mut().zip(row) {
            *sum += value;
        }
    }
    for value in &mut mean {
        *value /= rows.len() as f32;
    }
    normalize_embedding(&mean)
}

pub(crate) fn read_overlap_fraction(path: &Path) -> f32 {
    let Ok(contents) = fs::read_to_string(path) else {
        return 0.0;
    };
    let Some(line) = contents.lines().next() else {
        return 0.0;
    };
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|header| header.get("overlap_fraction").cloned())
        .and_then(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(value) => value.parse::<f64>().ok(),
            _ => None,
        })
        .map(|value| value as f32)
        .unwrap_or(0.0)
}

fn segment_start_ts_ms(
    journal_root: &Path,
    day: &str,
    segment_key: &str,
) -> Result<i64, AccumulationError> {
    if day.len() != 8 || !day.bytes().all(|byte| byte.is_ascii_digit()) || segment_key.len() < 6 {
        return Err(AccumulationError::Invalid(
            "invalid segment day or key".to_owned(),
        ));
    }
    let date = NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| AccumulationError::Invalid("invalid segment day or key".to_owned()))?;
    let hour = segment_key[0..2]
        .parse()
        .map_err(|_| AccumulationError::Invalid("invalid segment day or key".to_owned()))?;
    let minute = segment_key[2..4]
        .parse()
        .map_err(|_| AccumulationError::Invalid("invalid segment day or key".to_owned()))?;
    let second = segment_key[4..6]
        .parse()
        .map_err(|_| AccumulationError::Invalid("invalid segment day or key".to_owned()))?;
    let local = date
        .and_hms_opt(hour, minute, second)
        .ok_or_else(|| AccumulationError::Invalid("invalid segment day or key".to_owned()))?;
    let config = read_journal_config(journal_root).map_err(AccumulationError::Config)?;
    let timezone = config
        .config
        .as_ref()
        .and_then(|value| value.get("identity"))
        .and_then(Value::as_object)
        .and_then(|identity| identity.get("timezone"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(Tz::UTC);
    timezone
        .from_local_datetime(&local)
        .single()
        .or_else(|| timezone.from_local_datetime(&local).earliest())
        .map(|value| value.timestamp_millis())
        .ok_or_else(|| AccumulationError::Invalid("invalid segment day or key".to_owned()))
}

fn validate_request(request: &AccumulationRequest) -> Result<(), AccumulationError> {
    if request.encoder.id.is_empty()
        || request.encoder.sha256.len() != 64
        || !request
            .encoder
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || request.encoder.width == 0
    {
        return Err(AccumulationError::Invalid(
            "invalid encoder identity".to_owned(),
        ));
    }
    let mut ids = HashSet::new();
    if request
        .entity_ids
        .iter()
        .any(|id| id.is_empty() || !ids.insert(id))
    {
        return Err(AccumulationError::Invalid(
            "duplicate or empty entity id".to_owned(),
        ));
    }
    let mut labels = HashSet::new();
    if request
        .labels
        .iter()
        .any(|label| !labels.insert(label.sentence_id))
    {
        return Err(AccumulationError::Invalid(
            "duplicate label sentence id".to_owned(),
        ));
    }
    let mut embeddings = HashSet::new();
    for embedding in &request.embeddings {
        if !embeddings.insert(embedding.sentence_id)
            || embedding.values.len() != request.encoder.width
            || embedding.values.iter().any(|value| !value.is_finite())
        {
            return Err(AccumulationError::Invalid(
                "invalid embedding rows".to_owned(),
            ));
        }
    }
    Ok(())
}

fn increment(counts: &mut BTreeMap<AccumulationSkipReason, usize>, reason: AccumulationSkipReason) {
    *counts.entry(reason).or_default() += 1;
}
fn skip_entity(
    report: &mut EntityAccumulationReport,
    total: &mut BTreeMap<AccumulationSkipReason, usize>,
    reason: AccumulationSkipReason,
) {
    increment(&mut report.skipped_rows, reason.clone());
    increment(total, reason);
}
fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
