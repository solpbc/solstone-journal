// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Top-level native attribution orchestration for one segment.

use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_entity::{
    EntityLifecycleError, EntityResolutionError, EntityStoreError, load_all_journal_entities,
};
use solstone_core_journal_io::PathError;

use crate::admission::admissible_person_pool;
use crate::evidence::{
    CandidateEvidence, EvidenceError, EvidenceGap, extract_meeting_participants_with_gaps,
    extract_screen_participants_with_gaps, load_segment_speakers_with_gaps,
    load_setting_field_with_gaps, parse_setting_names,
};
use crate::layer1::{OwnerSeparationContext, separate_owner_statements};
use crate::layer2::{Layer2Inputs, apply_structural_heuristics};
use crate::layer3::{Layer3Inputs, apply_acoustic_matching};
use crate::owner_admission::{OwnerAdmission, admitted_owner_id};
use crate::owner_centroid::{OwnerCentroidError, load_owner_centroid};
use crate::voiceprint_centroid::{VoiceprintCentroidCache, VoiceprintLoadGap};
use solstone_core_speaker_id::embeddings::load_embeddings_file;
use solstone_core_speaker_id::transcript::{TranscriptError, read_transcript_rows};

/// Completed attribution output in the same statement order as the embedding sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolveOutput {
    pub labels: Vec<crate::layer1::Label>,
    pub unmatched: Vec<i64>,
    pub unmatched_texts: HashMap<i64, String>,
    pub source: Option<String>,
    pub candidates: Vec<String>,
    pub metadata: ResolveMetadata,
}

/// Optional diagnostics and sidecar versions emitted with successful attribution.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolveMetadata {
    pub owner_centroid_last_refreshed_at: Option<String>,
    pub voiceprint_versions: HashMap<String, usize>,
    pub candidate_evidence: Vec<CandidateEvidence>,
    /// Evidence-loader and resolution gaps. Voiceprint failures stay separate
    /// because they use a distinct type and Python has no unified gap output.
    pub candidate_evidence_gaps: Option<Vec<EvidenceGap>>,
    /// Native-only unreadable-voiceprint diagnostics, intentionally distinct
    /// from evidence gaps because Python never exposes them as evidence gaps.
    pub voiceprint_gaps: Option<Vec<VoiceprintLoadGap>>,
}

/// A terminal result that did not reach the attribution layers.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveOutcome {
    SegmentMissing,
    IdentityInvalid,
    NoOwnerCentroid,
    Empty { source: Option<String> },
    Resolved(Box<ResolveOutput>),
}

/// Failure while orchestrating attribution after prerequisite checks.
#[derive(Debug)]
pub enum ResolveError {
    Path(PathError),
    EntityStore(EntityStoreError),
    EntityLifecycle(EntityLifecycleError),
    Resolution(EntityResolutionError),
    Evidence(EvidenceError),
    Transcript(TranscriptError),
    Io { path: PathBuf, detail: String },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::EntityStore(error) => error.fmt(formatter),
            Self::EntityLifecycle(error) => error.fmt(formatter),
            Self::Resolution(error) => error.fmt(formatter),
            Self::Evidence(error) => error.fmt(formatter),
            Self::Transcript(error) => error.fmt(formatter),
            Self::Io { path, detail } => write!(formatter, "{}: {detail}", path.display()),
        }
    }
}
impl Error for ResolveError {}
impl From<PathError> for ResolveError {
    fn from(value: PathError) -> Self {
        Self::Path(value)
    }
}
impl From<EntityStoreError> for ResolveError {
    fn from(value: EntityStoreError) -> Self {
        Self::EntityStore(value)
    }
}
impl From<EntityLifecycleError> for ResolveError {
    fn from(value: EntityLifecycleError) -> Self {
        Self::EntityLifecycle(value)
    }
}
impl From<EntityResolutionError> for ResolveError {
    fn from(value: EntityResolutionError) -> Self {
        Self::Resolution(value)
    }
}
impl From<EvidenceError> for ResolveError {
    fn from(value: EvidenceError) -> Self {
        Self::Evidence(value)
    }
}
impl From<TranscriptError> for ResolveError {
    fn from(value: TranscriptError) -> Self {
        Self::Transcript(value)
    }
}

/// Attribute one segment through native Layers 1–3.
pub fn resolve(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    read_only: bool,
    now_ms: i64,
) -> Result<ResolveOutcome, ResolveError> {
    let segment_dir = crate::segment_path(journal_root, day, segment_key, stream, false)?;
    if read_only && !segment_dir.is_dir() {
        return Ok(ResolveOutcome::SegmentMissing);
    }
    let owner_entity_id = match admitted_owner_id(journal_root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => return Ok(ResolveOutcome::IdentityInvalid),
    };
    let owner = match load_owner_centroid(journal_root, &owner_entity_id) {
        Ok(Some(owner)) => owner,
        Ok(None) => return Ok(ResolveOutcome::NoOwnerCentroid),
        Err(OwnerCentroidError::IdentityInvalid | OwnerCentroidError::TargetMismatch { .. }) => {
            return Ok(ResolveOutcome::IdentityInvalid);
        }
        Err(_) => return Ok(ResolveOutcome::NoOwnerCentroid),
    };
    let Some((source, embeddings_path)) = embeddings_path(&segment_dir)? else {
        return Ok(ResolveOutcome::Empty { source: None });
    };
    let embeddings = match load_embeddings_file(&embeddings_path) {
        Ok(Some(embeddings)) => embeddings,
        Ok(None) | Err(_) => {
            return Ok(ResolveOutcome::Empty {
                source: Some(source),
            });
        }
    };
    if embeddings.statements.is_empty() {
        return Ok(ResolveOutcome::Empty {
            source: Some(source),
        });
    }

    let all_entities = load_all_journal_entities(journal_root)?;
    let entities = all_entities
        .iter()
        .filter(|entity| !entity.is_blocked())
        .cloned()
        .collect::<Vec<_>>();
    let unblocked = entities.iter().collect::<Vec<_>>();
    let margin_ids = admissible_person_pool(&unblocked)
        .into_iter()
        .filter(|entity| entity.id != owner_entity_id && !entity.is_principal())
        .map(|entity| entity.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut cache = VoiceprintCentroidCache::default();
    let mut voiceprint_gaps = Vec::new();
    let layer1 = separate_owner_statements(
        &embeddings.statements,
        OwnerSeparationContext {
            owner: &owner,
            owner_entity_id: &owner_entity_id,
            margin_non_principal_entity_ids: &margin_ids,
            journal_root,
            stream,
            now_ms,
        },
        &mut cache,
        &mut voiceprint_gaps,
    );

    let mut evidence_gaps = Vec::new();
    let (speakers, gaps) = load_segment_speakers_with_gaps(&segment_dir);
    evidence_gaps.extend(gaps);
    let (setting, gaps) = load_setting_field_with_gaps(&segment_dir);
    evidence_gaps.extend(gaps);
    let setting_names = match setting {
        Some(setting) => parse_setting_names(journal_root, &setting)?,
        None => Vec::new(),
    };
    let (screen_names, gaps) = extract_screen_participants_with_gaps(&segment_dir);
    evidence_gaps.extend(gaps);
    let (meeting_names, gaps) = extract_meeting_participants_with_gaps(journal_root, day);
    evidence_gaps.extend(gaps);
    let layer2 = apply_structural_heuristics(
        layer1.labels,
        Layer2Inputs {
            speakers: &speakers,
            setting_names: &setting_names,
            screen_names: &screen_names,
            meeting_names: &meeting_names,
            entities: &entities,
            all_entities: &all_entities,
            non_owner_sids: &layer1.non_owner_sids,
            margin_declined_sids: &layer1.margin_declined_sids,
            journal_root,
            day,
            segment_key,
            read_only,
        },
    )?;
    let transcript = transcript_data(&segment_dir.join(format!("{source}.jsonl")))?;
    let layer3 = apply_acoustic_matching(
        Layer3Inputs {
            labels: layer2.labels,
            non_owner_sids: &layer1.non_owner_sids,
            margin_declined_sids: &layer1.margin_declined_sids,
            candidate_entity_ids: &layer2.candidate_entity_ids,
            entities: &entities,
            journal_root,
            stream,
            now_ms,
            statements: &embeddings.statements,
            integer_speakers: &transcript.integer_speakers,
        },
        &mut cache,
        &mut voiceprint_gaps,
    );
    let unmatched = embeddings
        .statements
        .iter()
        .map(|(sentence_id, _)| *sentence_id)
        .filter(|sentence_id| {
            layer3
                .labels
                .get(sentence_id)
                .is_some_and(|label| label.speaker.is_none())
        })
        .collect::<Vec<_>>();
    let unmatched_texts = unmatched
        .iter()
        .filter_map(|sentence_id| {
            transcript
                .all_texts
                .get(sentence_id)
                .map(|text| (*sentence_id, text.clone()))
        })
        .collect();
    let labels = embeddings
        .statements
        .iter()
        .filter_map(|(sentence_id, _)| layer3.labels.get(sentence_id).cloned())
        .collect();
    Ok(ResolveOutcome::Resolved(Box::new(ResolveOutput {
        labels,
        unmatched,
        unmatched_texts,
        source: Some(source),
        candidates: layer2.resolved_candidate_names,
        metadata: ResolveMetadata {
            owner_centroid_last_refreshed_at: owner.last_refreshed_at,
            voiceprint_versions: layer3.voiceprint_versions,
            candidate_evidence: layer2.candidate_evidence,
            candidate_evidence_gaps: (!evidence_gaps.is_empty()).then_some(evidence_gaps),
            voiceprint_gaps: (!voiceprint_gaps.is_empty()).then_some(voiceprint_gaps),
        },
    })))
}

fn embeddings_path(segment_dir: &Path) -> Result<Option<(String, PathBuf)>, ResolveError> {
    let entries = fs::read_dir(segment_dir).map_err(|error| ResolveError::Io {
        path: segment_dir.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("npz"))
        .filter(|path| {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            stem.ends_with("_audio") || stem == "audio"
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths.into_iter().next().map(|path| {
        (
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned(),
            path,
        )
    }))
}

struct TranscriptData {
    integer_speakers: HashMap<i64, i64>,
    all_texts: HashMap<i64, String>,
}

fn transcript_data(path: &Path) -> Result<TranscriptData, ResolveError> {
    if !path.exists() {
        return Ok(TranscriptData {
            integer_speakers: HashMap::new(),
            all_texts: HashMap::new(),
        });
    }
    let bytes = fs::read(path).map_err(|error| ResolveError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let read = read_transcript_rows(&bytes)?;
    let mut integer_speakers = HashMap::new();
    let mut all_texts = HashMap::new();
    for row in read.rows {
        if let Some(speaker) = row.value.get("speaker").and_then(serde_json::Value::as_i64) {
            integer_speakers.insert(row.sentence_id, speaker);
        }
        all_texts.insert(
            row.sentence_id,
            row.value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
    }
    Ok(TranscriptData {
        integer_speakers,
        all_texts,
    })
}
