// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only speaker identify-operation ledger.
//!
//! This module is the sole Rust writer for `speakers/identify-operations.jsonl`.
//! It deliberately exposes append and read/fold operations only: no rewrite,
//! compaction, or deletion primitive exists here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{AppendError, LockError, LockOptions, append_jsonl, hold_lock};
use thiserror::Error;

use crate::owner_admission::OWNER_IDENTITY_INVALID_REASON;

pub const IDENTIFY_OPERATION_SCHEMA_VERSION: i64 = 2;
pub const FORWARD_PHASE_ORDER: [ForwardPhase; 7] = [
    ForwardPhase::Entity,
    ForwardPhase::KeepSeparate,
    ForwardPhase::DirectVoiceprints,
    ForwardPhase::Corrections,
    ForwardPhase::Labels,
    ForwardPhase::RetroTracker,
    ForwardPhase::Sentinel,
];
pub const UNDO_PHASE_ORDER: [UndoPhase; 6] = [
    UndoPhase::Labels,
    UndoPhase::Corrections,
    UndoPhase::Voiceprints,
    UndoPhase::Tracker,
    UndoPhase::Sentinel,
    UndoPhase::Entity,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ForwardPhase {
    Entity,
    KeepSeparate,
    DirectVoiceprints,
    Corrections,
    Labels,
    RetroTracker,
    Sentinel,
}

impl ForwardPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::KeepSeparate => "keep_separate",
            Self::DirectVoiceprints => "direct_voiceprints",
            Self::Corrections => "corrections",
            Self::Labels => "labels",
            Self::RetroTracker => "retro_tracker",
            Self::Sentinel => "sentinel",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "entity" => Self::Entity,
            "keep_separate" => Self::KeepSeparate,
            "direct_voiceprints" => Self::DirectVoiceprints,
            "corrections" => Self::Corrections,
            "labels" => Self::Labels,
            "retro_tracker" => Self::RetroTracker,
            "sentinel" => Self::Sentinel,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UndoPhase {
    Labels,
    Corrections,
    Voiceprints,
    Tracker,
    Sentinel,
    Entity,
}

impl UndoPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Labels => "labels",
            Self::Corrections => "corrections",
            Self::Voiceprints => "voiceprints",
            Self::Tracker => "tracker",
            Self::Sentinel => "sentinel",
            Self::Entity => "entity",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "labels" => Self::Labels,
            "corrections" => Self::Corrections,
            "voiceprints" => Self::Voiceprints,
            "tracker" => Self::Tracker,
            "sentinel" => Self::Sentinel,
            "entity" => Self::Entity,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Prepared,
    Checkpoint,
    Committed,
    RepairRequired,
    RepairResumed,
    UndoPrepared,
    UndoCheckpoint,
    UndoCommitted,
    UndoRepairRequired,
}

impl EventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Checkpoint => "checkpoint",
            Self::Committed => "committed",
            Self::RepairRequired => "repair_required",
            Self::RepairResumed => "repair_resumed",
            Self::UndoPrepared => "undo_prepared",
            Self::UndoCheckpoint => "undo_checkpoint",
            Self::UndoCommitted => "undo_committed",
            Self::UndoRepairRequired => "undo_repair_required",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "prepared" => Self::Prepared,
            "checkpoint" => Self::Checkpoint,
            "committed" => Self::Committed,
            "repair_required" => Self::RepairRequired,
            "repair_resumed" => Self::RepairResumed,
            "undo_prepared" => Self::UndoPrepared,
            "undo_checkpoint" => Self::UndoCheckpoint,
            "undo_committed" => Self::UndoCommitted,
            "undo_repair_required" => Self::UndoRepairRequired,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventPayload {
    Prepared {
        request_fingerprint: String,
        prepared_plan: Value,
    },
    Checkpoint {
        phase: ForwardPhase,
        checkpoint: Value,
    },
    Committed {
        result: Value,
    },
    RepairRequired {
        phase: ForwardPhase,
        repair_code: String,
        repair_categories: Value,
        partial_report: Value,
    },
    RepairResumed {
        repair_event_id: String,
        phase: ForwardPhase,
    },
    UndoPrepared {
        undo_started_at: String,
    },
    UndoCheckpoint {
        phase: UndoPhase,
        undo_report_delta: Value,
    },
    UndoCommitted {
        undo_report: Value,
    },
    UndoRepairRequired {
        phase: UndoPhase,
        repair_code: String,
        repair_categories: Value,
        undo_report: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdentifyOperationEvent {
    pub schema_version: i64,
    pub event_id: String,
    pub operation_id: String,
    pub request_id: String,
    pub ts: String,
    pub caller: String,
    pub actor: Option<String>,
    pub payload: EventPayload,
}

impl IdentifyOperationEvent {
    #[must_use]
    pub fn event_kind(&self) -> EventKind {
        match self.payload {
            EventPayload::Prepared { .. } => EventKind::Prepared,
            EventPayload::Checkpoint { .. } => EventKind::Checkpoint,
            EventPayload::Committed { .. } => EventKind::Committed,
            EventPayload::RepairRequired { .. } => EventKind::RepairRequired,
            EventPayload::RepairResumed { .. } => EventKind::RepairResumed,
            EventPayload::UndoPrepared { .. } => EventKind::UndoPrepared,
            EventPayload::UndoCheckpoint { .. } => EventKind::UndoCheckpoint,
            EventPayload::UndoCommitted { .. } => EventKind::UndoCommitted,
            EventPayload::UndoRepairRequired { .. } => EventKind::UndoRepairRequired,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut row = Map::new();
        row.insert(
            "schema_version".to_owned(),
            Value::from(self.schema_version),
        );
        row.insert("event_id".to_owned(), Value::String(self.event_id.clone()));
        row.insert(
            "operation_id".to_owned(),
            Value::String(self.operation_id.clone()),
        );
        row.insert(
            "request_id".to_owned(),
            Value::String(self.request_id.clone()),
        );
        row.insert(
            "event_kind".to_owned(),
            Value::String(self.event_kind().as_str().to_owned()),
        );
        row.insert("ts".to_owned(), Value::String(self.ts.clone()));
        row.insert("caller".to_owned(), Value::String(self.caller.clone()));
        row.insert(
            "actor".to_owned(),
            self.actor.clone().map_or(Value::Null, Value::String),
        );
        match &self.payload {
            EventPayload::Prepared {
                request_fingerprint,
                prepared_plan,
            } => {
                row.insert(
                    "request_fingerprint".to_owned(),
                    Value::String(request_fingerprint.clone()),
                );
                row.insert("prepared_plan".to_owned(), prepared_plan.clone());
            }
            EventPayload::Checkpoint { phase, checkpoint } => {
                row.insert("phase".to_owned(), Value::String(phase.as_str().to_owned()));
                row.insert("checkpoint".to_owned(), checkpoint.clone());
            }
            EventPayload::Committed { result } => {
                row.insert("result".to_owned(), result.clone());
            }
            EventPayload::RepairRequired {
                phase,
                repair_code,
                repair_categories,
                partial_report,
            } => {
                row.insert("phase".to_owned(), Value::String(phase.as_str().to_owned()));
                row.insert("repair_code".to_owned(), Value::String(repair_code.clone()));
                row.insert("repair_categories".to_owned(), repair_categories.clone());
                row.insert("partial_report".to_owned(), partial_report.clone());
            }
            EventPayload::RepairResumed {
                repair_event_id,
                phase,
            } => {
                row.insert(
                    "repair_event_id".to_owned(),
                    Value::String(repair_event_id.clone()),
                );
                row.insert("phase".to_owned(), Value::String(phase.as_str().to_owned()));
            }
            EventPayload::UndoPrepared { undo_started_at } => {
                row.insert(
                    "undo_started_at".to_owned(),
                    Value::String(undo_started_at.clone()),
                );
            }
            EventPayload::UndoCheckpoint {
                phase,
                undo_report_delta,
            } => {
                row.insert("phase".to_owned(), Value::String(phase.as_str().to_owned()));
                row.insert("undo_report_delta".to_owned(), undo_report_delta.clone());
            }
            EventPayload::UndoCommitted { undo_report } => {
                row.insert("undo_report".to_owned(), undo_report.clone());
            }
            EventPayload::UndoRepairRequired {
                phase,
                repair_code,
                repair_categories,
                undo_report,
            } => {
                row.insert("phase".to_owned(), Value::String(phase.as_str().to_owned()));
                row.insert("repair_code".to_owned(), Value::String(repair_code.clone()));
                row.insert("repair_categories".to_owned(), repair_categories.clone());
                row.insert("undo_report".to_owned(), undo_report.clone());
            }
        }
        Value::Object(row)
    }
}

/// One validated ledger line retaining the exact JSON bytes used for deduplication.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRow {
    pub event: IdentifyOperationEvent,
    raw_json: String,
}

impl LedgerRow {
    fn parse(path: &Path, line: usize, raw_json: &str) -> Result<Self, IdentifyOperationError> {
        let value: Value = serde_json::from_str(raw_json).map_err(|source| {
            IdentifyOperationError::MalformedJson {
                path: path.to_path_buf(),
                line,
                source,
            }
        })?;
        if !value.is_object() {
            return Err(IdentifyOperationError::NonObjectRow {
                path: path.to_path_buf(),
                line,
            });
        }
        let event = validate_row(&value).map_err(|source| IdentifyOperationError::InvalidRow {
            path: path.to_path_buf(),
            line,
            source: Box::new(source),
        })?;
        Ok(Self {
            event,
            raw_json: raw_json.to_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalStatus {
    InProgress,
    Committed,
    RepairRequired,
    Undoing,
    Undone,
    UndoRepairRequired,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperationState {
    pub operation_id: String,
    pub request_id: String,
    pub request_fingerprint: String,
    pub cluster_member_set: BTreeSet<MemberProvenance>,
    pub target_entity_id: Option<String>,
    pub target_entity_name: Option<String>,
    pub will_create: bool,
    pub entity_type: Option<String>,
    pub reviewed_near_match_entity_ids: Vec<String>,
    pub completed_phases: Vec<ForwardPhase>,
    pub pending_phases: Vec<String>,
    pub terminal_status: TerminalStatus,
    pub result: Option<Value>,
    pub undo_report: Option<Value>,
    pub undo_started_at: Option<String>,
    pub undo_committed_count: usize,
    pub phase_checkpoints: BTreeMap<ForwardPhase, Value>,
    pub prepared_plan: Value,
    pub repair_required: Option<IdentifyOperationEvent>,
    pub undo_repair_required: Option<IdentifyOperationEvent>,
    pub undo_phase_checkpoints: BTreeMap<UndoPhase, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MemberProvenance {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub source: String,
    pub sentence_id: i64,
}

/// Exact correction-row signature used when proving an identify operation was restored.
pub type CorrectionArtifactSignature = Vec<Value>;

/// Correction artifacts expected after a fully restored undo. Repeated entries
/// deliberately retain their multiplicity, matching Python's `Counter` input.
pub type CorrectionArtifactSignatures = Vec<CorrectionArtifactSignature>;

const UNDO_CATEGORY_KEYS: [(&str, &[&str]); 6] = [
    (
        "labels",
        &[
            "restored_count",
            "skipped_count",
            "skipped_reasons",
            "removed_inserted_count",
            "patched_existing_count",
        ],
    ),
    (
        "corrections",
        &[
            "restored_count",
            "skipped_count",
            "skipped_reasons",
            "appended_count",
            "already_present_count",
        ],
    ),
    (
        "voiceprints",
        &[
            "restored_count",
            "skipped_count",
            "skipped_reasons",
            "removed_count",
            "missing_count",
            "metadata_mismatch_count",
        ],
    ),
    (
        "tracker",
        &[
            "restored_count",
            "skipped_count",
            "skipped_reasons",
            "restored_candidate_count",
        ],
    ),
    (
        "sentinel",
        &[
            "restored_count",
            "skipped_count",
            "skipped_reasons",
            "removed_count",
            "restored_prior_count",
        ],
    ),
    (
        "entity",
        &[
            "restored_count",
            "skipped_count",
            "skipped_reasons",
            "deleted",
            "blocked_categories",
            "keep_separate_sources_removed_count",
        ],
    ),
];

/// Return whether every durable undo artifact proves a complete restoration.
#[must_use]
pub fn is_fully_restored_identify_operation(state: &OperationState) -> bool {
    if state.terminal_status != TerminalStatus::Undone
        || state.repair_required.is_some()
        || state.undo_repair_required.is_some()
        || state.undo_committed_count != 1
        || state.undo_phase_checkpoints.len() != UNDO_PHASE_ORDER.len()
    {
        return false;
    }
    let Some(report) = state.undo_report.as_ref().and_then(Value::as_object) else {
        return false;
    };
    if report.len() != 3
        || report.get("status").and_then(Value::as_str) != Some("undone")
        || report.get("operation_id").and_then(Value::as_str) != Some(state.operation_id.as_str())
    {
        return false;
    }
    let Some(categories) = report.get("undo_report").and_then(Value::as_object) else {
        return false;
    };
    if categories.len() != UNDO_CATEGORY_KEYS.len() {
        return false;
    }
    for phase in UNDO_PHASE_ORDER {
        let name = phase.as_str();
        let Some(category) = categories.get(name) else {
            return false;
        };
        if !valid_undo_category_shape(name, category) {
            return false;
        }
        let Some(phase_delta) = state
            .undo_phase_checkpoints
            .get(&phase)
            .and_then(Value::as_object)
        else {
            return false;
        };
        if phase_delta.len() != 1 || phase_delta.get(name) != Some(category) {
            return false;
        }
    }
    restored_counts_match_forward_artifacts(state, categories)
}

/// Return the artifact signature for one correction row at its segment provenance.
#[must_use]
pub fn identify_correction_artifact_signature(
    row: &Value,
    day: &str,
    stream: &str,
    segment_key: &str,
) -> CorrectionArtifactSignature {
    let object = row.as_object();
    vec![
        Value::String(day.to_owned()),
        Value::String(stream.to_owned()),
        Value::String(segment_key.to_owned()),
        value_at(object, "sentence_id"),
        value_at(object, "correction_kind"),
        value_at(object, "operation_id"),
        value_at(object, "undo_of_operation_id"),
        value_at(object, "original_speaker"),
        value_at(object, "corrected_speaker"),
        value_at(object, "original_method"),
        value_at(object, "timestamp"),
    ]
}

/// Return the forward and undo correction artifacts expected for one segment.
#[must_use]
pub fn expected_restored_correction_artifact_signatures(
    state: &OperationState,
    day: &str,
    stream: &str,
    segment_key: &str,
) -> CorrectionArtifactSignatures {
    if !is_fully_restored_identify_operation(state) {
        return Vec::new();
    }
    let Some(undo_started_at) = state.undo_started_at.as_ref() else {
        return Vec::new();
    };
    let Some(appended_keys) = state
        .phase_checkpoints
        .get(&ForwardPhase::Corrections)
        .and_then(|checkpoint| list_field(checkpoint, "appended_keys"))
    else {
        return Vec::new();
    };
    let Some(corrections) = prepared_correction_rows(&state.prepared_plan) else {
        return Vec::new();
    };
    let Some(labels) = prepared_label_entries(&state.prepared_plan) else {
        return Vec::new();
    };
    let Some(target) = state.target_entity_id.as_ref() else {
        return Vec::new();
    };
    let mut expected = Vec::new();
    for key in appended_keys {
        let Some(provenance) = sentence_key(key) else {
            return Vec::new();
        };
        if provenance.0 != day || provenance.1 != stream || provenance.2 != segment_key {
            continue;
        }
        let Some(forward) = corrections.get(&provenance) else {
            return Vec::new();
        };
        let Some(label) = labels.get(&provenance) else {
            return Vec::new();
        };
        let Some(prior_speaker) = prior_speaker(label) else {
            return Vec::new();
        };
        expected.push(vec![
            Value::String(provenance.0.clone()),
            Value::String(provenance.1.clone()),
            Value::String(provenance.2.clone()),
            Value::from(provenance.3),
            Value::String("identify".into()),
            Value::String(state.operation_id.clone()),
            Value::Null,
            value_at(forward.as_object(), "original_speaker"),
            value_at(forward.as_object(), "corrected_speaker"),
            value_at(forward.as_object(), "original_method"),
            value_at(forward.as_object(), "timestamp"),
        ]);
        expected.push(vec![
            Value::String(provenance.0),
            Value::String(provenance.1),
            Value::String(provenance.2),
            Value::from(provenance.3),
            Value::String("identify_undo".into()),
            Value::String(state.operation_id.clone()),
            Value::String(state.operation_id.clone()),
            Value::String(target.clone()),
            prior_speaker,
            Value::String("user_identified".into()),
            Value::String(undo_started_at.clone()),
        ]);
    }
    expected
}

fn valid_undo_category_shape(name: &str, value: &Value) -> bool {
    let Some(expected) = UNDO_CATEGORY_KEYS
        .iter()
        .find_map(|(current, keys)| (*current == name).then_some(*keys))
    else {
        return false;
    };
    let Some(category) = value.as_object() else {
        return false;
    };
    if category.len() != expected.len() || expected.iter().any(|key| !category.contains_key(*key)) {
        return false;
    }
    for (key, value) in category {
        if key.ends_with("_count") && !non_negative_int(value) {
            return false;
        }
    }
    if category.get("skipped_count").and_then(Value::as_i64) != Some(0)
        || !zero_skipped_reasons(category.get("skipped_reasons"))
    {
        return false;
    }
    if name == "entity"
        && (!category.get("deleted").is_some_and(Value::is_boolean)
            || category
                .get("blocked_categories")
                .and_then(Value::as_array)
                .is_none_or(|items| !items.is_empty()))
    {
        return false;
    }
    true
}

fn zero_skipped_reasons(value: Option<&Value>) -> bool {
    value.and_then(Value::as_object).is_some_and(|reasons| {
        reasons
            .values()
            .all(|count| non_negative_int(count) && count.as_i64() == Some(0))
    })
}
fn non_negative_int(value: &Value) -> bool {
    value.as_i64().is_some_and(|value| value >= 0)
}

fn restored_counts_match_forward_artifacts(
    state: &OperationState,
    categories: &Map<String, Value>,
) -> bool {
    let Some(labels) = state.phase_checkpoints.get(&ForwardPhase::Labels) else {
        return false;
    };
    let (Some(patched), Some(inserted)) = (
        list_field(labels, "patched_sentence_keys"),
        list_field(labels, "inserted_sentence_keys"),
    ) else {
        return false;
    };
    let Some(label_report) = categories.get("labels").and_then(Value::as_object) else {
        return false;
    };
    if integer_field(label_report, "patched_existing_count") != Some(patched.len() as i64)
        || integer_field(label_report, "removed_inserted_count") != Some(inserted.len() as i64)
        || integer_field(label_report, "restored_count")
            != Some((patched.len() + inserted.len()) as i64)
    {
        return false;
    }
    let Some(corrections) = state
        .phase_checkpoints
        .get(&ForwardPhase::Corrections)
        .and_then(|checkpoint| list_field(checkpoint, "appended_keys"))
    else {
        return false;
    };
    let Some(correction_report) = categories.get("corrections").and_then(Value::as_object) else {
        return false;
    };
    if integer_field(correction_report, "appended_count") != Some(corrections.len() as i64)
        || integer_field(correction_report, "restored_count") != Some(corrections.len() as i64)
        || integer_field(correction_report, "already_present_count") != Some(0)
    {
        return false;
    }
    let Some(voiceprint_count) = expected_voiceprint_removal_count(state) else {
        return false;
    };
    let Some(voiceprint_report) = categories.get("voiceprints").and_then(Value::as_object) else {
        return false;
    };
    if integer_field(voiceprint_report, "removed_count") != Some(voiceprint_count as i64)
        || integer_field(voiceprint_report, "restored_count") != Some(voiceprint_count as i64)
        || integer_field(voiceprint_report, "missing_count") != Some(0)
        || integer_field(voiceprint_report, "metadata_mismatch_count") != Some(0)
    {
        return false;
    }
    let Some(tracker) = state
        .phase_checkpoints
        .get(&ForwardPhase::RetroTracker)
        .and_then(Value::as_object)
    else {
        return false;
    };
    let tracker_expected = usize::from(
        tracker.get("matched").and_then(Value::as_bool) == Some(true)
            && tracker.get("candidate_id").is_some_and(non_negative_int),
    );
    let Some(tracker_report) = categories.get("tracker").and_then(Value::as_object) else {
        return false;
    };
    if integer_field(tracker_report, "restored_candidate_count") != Some(tracker_expected as i64)
        || integer_field(tracker_report, "restored_count") != Some(tracker_expected as i64)
    {
        return false;
    }
    let Some(sentinel) = state
        .phase_checkpoints
        .get(&ForwardPhase::Sentinel)
        .and_then(Value::as_object)
    else {
        return false;
    };
    let Some(written) = sentinel.get("written").and_then(Value::as_bool) else {
        return false;
    };
    let sentinel_expected = usize::from(written);
    let Some(sentinel_report) = categories.get("sentinel").and_then(Value::as_object) else {
        return false;
    };
    if integer_field(sentinel_report, "restored_count") != Some(sentinel_expected as i64)
        || integer_field(sentinel_report, "removed_count").unwrap_or(-1)
            + integer_field(sentinel_report, "restored_prior_count").unwrap_or(-1)
            != sentinel_expected as i64
    {
        return false;
    }
    let Some(entity) = state
        .phase_checkpoints
        .get(&ForwardPhase::Entity)
        .and_then(Value::as_object)
    else {
        return false;
    };
    let Some(created) = entity.get("entity_created").and_then(Value::as_bool) else {
        return false;
    };
    let entity_expected = usize::from(created);
    let Some(entity_report) = categories.get("entity").and_then(Value::as_object) else {
        return false;
    };
    if integer_field(entity_report, "restored_count") != Some(entity_expected as i64)
        || entity_report.get("deleted").and_then(Value::as_bool) != Some(created)
    {
        return false;
    }
    let Some(pair_keys) = state
        .phase_checkpoints
        .get(&ForwardPhase::KeepSeparate)
        .and_then(|checkpoint| list_field(checkpoint, "pair_keys"))
    else {
        return false;
    };
    integer_field(entity_report, "keep_separate_sources_removed_count")
        == Some(if state.will_create {
            pair_keys.len() as i64
        } else {
            0
        })
}

fn expected_voiceprint_removal_count(state: &OperationState) -> Option<usize> {
    let direct = list_field(
        state
            .phase_checkpoints
            .get(&ForwardPhase::DirectVoiceprints)?,
        "saved_keys",
    )?;
    let retro = list_field(
        state.phase_checkpoints.get(&ForwardPhase::RetroTracker)?,
        "saved_keys",
    )?;
    let mut keys = BTreeSet::new();
    for key in direct.iter().chain(retro.iter()) {
        keys.insert(voiceprint_key(key)?);
    }
    Some(keys.len())
}

type SentenceKey = (String, String, String, i64);
type VoiceprintKey = (String, String, String, i64);
fn list_field<'a>(value: &'a Value, field: &str) -> Option<&'a Vec<Value>> {
    value.as_object()?.get(field)?.as_array()
}
fn voiceprint_key(value: &Value) -> Option<VoiceprintKey> {
    let object = value.as_object()?;
    Some((
        nonempty(object.get("day"))?,
        nonempty(object.get("segment_key"))?,
        nonempty(object.get("source"))?,
        object
            .get("sentence_id")?
            .as_i64()
            .filter(|value| *value >= 0)?,
    ))
}
fn sentence_key(value: &Value) -> Option<SentenceKey> {
    let object = value.as_object()?;
    Some((
        nonempty(object.get("day"))?,
        nonempty(object.get("stream"))?,
        nonempty(object.get("segment_key"))?,
        object
            .get("sentence_id")?
            .as_i64()
            .filter(|value| *value >= 0)?,
    ))
}
fn nonempty(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
fn prepared_correction_rows(plan: &Value) -> Option<HashMap<SentenceKey, Value>> {
    let mut rows = HashMap::new();
    for segment in prepared_segments(plan) {
        let object = segment.as_object()?;
        for row in list_field(object.get("corrections")?, "rows_to_append")? {
            let key = segment_sentence_key(object, row)?;
            if rows.insert(key, row.clone()).is_some() {
                return None;
            }
        }
    }
    Some(rows)
}
fn prepared_label_entries(plan: &Value) -> Option<HashMap<SentenceKey, Value>> {
    let mut rows = HashMap::new();
    for segment in prepared_segments(plan) {
        let object = segment.as_object()?;
        for row in object.get("labels")?.as_array()? {
            let key = segment_sentence_key(object, row)?;
            if rows.insert(key, row.clone()).is_some() {
                return None;
            }
        }
    }
    Some(rows)
}
fn prepared_segments(plan: &Value) -> Vec<&Value> {
    plan.get("segments")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter(|segment| segment.is_object())
                .collect()
        })
        .unwrap_or_default()
}
fn segment_sentence_key(segment: &Map<String, Value>, row: &Value) -> Option<SentenceKey> {
    Some((
        nonempty(segment.get("day"))?,
        nonempty(segment.get("stream"))?,
        nonempty(segment.get("segment_key"))?,
        row.get("sentence_id")?
            .as_i64()
            .filter(|value| *value >= 0)?,
    ))
}
fn prior_speaker(label: &Value) -> Option<Value> {
    let object = label.as_object()?;
    match object.get("prior_state")?.as_str()? {
        "absent" => Some(Value::Null),
        "present" => {
            let prior = object.get("prior_label")?.as_object()?;
            match prior.get("speaker") {
                Some(Value::Null) => Some(Value::Null),
                Some(Value::String(value)) => Some(Value::String(value.clone())),
                _ => None,
            }
        }
        _ => None,
    }
}
fn value_at(object: Option<&Map<String, Value>>, field: &str) -> Value {
    object
        .and_then(|object| object.get(field))
        .cloned()
        .unwrap_or(Value::Null)
}
fn integer_field(object: &Map<String, Value>, field: &str) -> Option<i64> {
    object.get(field).and_then(Value::as_i64)
}

#[derive(Debug, Error)]
pub enum IdentifyOperationError {
    #[error("failed to read identify operation ledger {path}: {source}")]
    ReadIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed identify operation JSONL at {path}:{line}: {source}")]
    MalformedJson {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("non-object identify operation JSONL at {path}:{line}")]
    NonObjectRow { path: PathBuf, line: usize },
    #[error("invalid identify operation row at {path}:{line}: {source}")]
    InvalidRow {
        path: PathBuf,
        line: usize,
        #[source]
        source: Box<Self>,
    },
    #[error("invalid schema_version")]
    InvalidSchemaVersion,
    #[error("repair_resumed requires schema_version 2")]
    RepairResumeRequiresSchemaVersion2,
    #[error("missing or invalid {field}")]
    MissingOrInvalidField { field: &'static str },
    #[error("unknown event_kind: {event_kind}")]
    UnknownEventKind { event_kind: String },
    #[error("missing actor")]
    MissingActor,
    #[error("actor must be a string or null")]
    InvalidActor,
    #[error("request_fingerprint must be a sha256 hex digest")]
    InvalidRequestFingerprint,
    #[error("prepared_plan.plan_schema_version must be 1")]
    InvalidPlanSchemaVersion,
    #[error("prepared_plan operation_id mismatch")]
    PreparedOperationIdMismatch,
    #[error("prepared_plan request_id mismatch")]
    PreparedRequestIdMismatch,
    #[error("prepared_plan missing {field}")]
    PreparedPlanMissing { field: &'static str },
    #[error("prepared_plan.request missing {field}")]
    PreparedRequestMissing { field: &'static str },
    #[error("prepared_plan cluster member_count mismatch")]
    ClusterMemberCountMismatch,
    #[error("prepared_plan cluster member is not object")]
    ClusterMemberNotObject,
    #[error("invalid cluster member provenance")]
    InvalidClusterMemberProvenance,
    #[error("prepared_plan target.will_create must be bool")]
    InvalidTargetWillCreate,
    #[error("invalid checkpoint phase: {phase}")]
    InvalidCheckpointPhase { phase: String },
    #[error("invalid undo checkpoint phase: {phase}")]
    InvalidUndoCheckpointPhase { phase: String },
    #[error("checkpoint.phase_status must be complete")]
    IncompleteCheckpoint,
    #[error("retro checkpoint candidate_id invalid")]
    InvalidRetroCandidateId,
    #[error("invalid repair phase: {phase}")]
    InvalidRepairPhase { phase: String },
    #[error("invalid undo repair phase: {phase}")]
    InvalidUndoRepairPhase { phase: String },
    #[error("conflicting duplicate event_id {event_id}")]
    ConflictingDuplicateEventId { event_id: String },
    #[error("repair resume {event_id} does not name the latest outstanding identity repair")]
    InvalidRepairResume { event_id: String },
    #[error("operation must have exactly one prepared event")]
    PreparedEventCount,
    #[error("conflicting checkpoint for phase {phase}")]
    ConflictingCheckpoint { phase: String },
    #[error("conflicting undo checkpoint for phase {phase}")]
    ConflictingUndoCheckpoint { phase: String },
    #[error("failed to create identify operation ledger directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("identify operation ledger lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("identify operation ledger append failed: {0}")]
    Append(#[from] AppendError),
}

pub fn operation_id_for_request(request_id: &str) -> Result<String, IdentifyOperationError> {
    if request_id.is_empty() {
        return Err(IdentifyOperationError::MissingOrInvalidField {
            field: "request_id",
        });
    }
    let digest = Sha256::digest(request_id.as_bytes());
    Ok(format!("idop_{:x}", digest)[..29].to_owned())
}

/// Python's canonical JSON escapes non-ASCII while `serde_json` does not;
/// these structural ledger identifiers are ASCII in practice.
#[must_use]
pub fn request_fingerprint(
    members: &[MemberProvenance],
    target_entity_id: &str,
    will_create: bool,
    entity_type: &str,
    reviewed_ids: &[String],
) -> String {
    let mut members = members.to_vec();
    members.sort();
    let mut reviewed = reviewed_ids.to_vec();
    reviewed.sort();
    reviewed.dedup();
    let mut payload = BTreeMap::new();
    payload.insert(
        "cluster_members",
        Value::Array(
            members
                .iter()
                .map(|member| {
                    Value::Array(vec![
                        Value::String(member.day.clone()),
                        Value::String(member.stream.clone()),
                        Value::String(member.segment_key.clone()),
                        Value::String(member.source.clone()),
                        Value::from(member.sentence_id),
                    ])
                })
                .collect(),
        ),
    );
    payload.insert("entity_type", Value::String(entity_type.to_owned()));
    payload.insert(
        "reviewed_near_match_entity_ids",
        Value::Array(reviewed.into_iter().map(Value::String).collect()),
    );
    payload.insert(
        "target_entity_id",
        Value::String(target_entity_id.to_owned()),
    );
    payload.insert("will_create", Value::Bool(will_create));
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_string(&payload)
                .expect("JSON value serializes")
                .as_bytes()
        )
    )
}

/// Validate one JSON row and return its typed event representation.
pub fn validate_row(row: &Value) -> Result<IdentifyOperationEvent, IdentifyOperationError> {
    let object = row
        .as_object()
        .ok_or(IdentifyOperationError::MissingOrInvalidField { field: "row" })?;
    let schema_version = object.get("schema_version").and_then(Value::as_i64);
    if !matches!(schema_version, Some(1 | IDENTIFY_OPERATION_SCHEMA_VERSION)) {
        return Err(IdentifyOperationError::InvalidSchemaVersion);
    }
    let schema_version = schema_version.expect("validated schema version");
    let event_kind_value = required_str(object, "event_kind")?;
    let kind =
        EventKind::parse(&event_kind_value).ok_or(IdentifyOperationError::UnknownEventKind {
            event_kind: event_kind_value,
        })?;
    let event_id = required_str(object, "event_id")?;
    let operation_id = required_str(object, "operation_id")?;
    let request_id = required_str(object, "request_id")?;
    let ts = required_str(object, "ts")?;
    let caller = required_str(object, "caller")?;
    let actor = match object.get("actor") {
        None => return Err(IdentifyOperationError::MissingActor),
        Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(IdentifyOperationError::InvalidActor),
    };
    let payload = match kind {
        EventKind::Prepared => {
            let request_fingerprint = required_str(object, "request_fingerprint")?;
            if request_fingerprint.len() != 64 {
                return Err(IdentifyOperationError::InvalidRequestFingerprint);
            }
            let prepared_plan = required_object_value(object, "prepared_plan")?;
            validate_prepared(&prepared_plan, &operation_id, &request_id)?;
            EventPayload::Prepared {
                request_fingerprint,
                prepared_plan,
            }
        }
        EventKind::Checkpoint => {
            let phase_text = required_str(object, "phase")?;
            let phase = ForwardPhase::parse(&phase_text)
                .ok_or(IdentifyOperationError::InvalidCheckpointPhase { phase: phase_text })?;
            let checkpoint = required_object_value(object, "checkpoint")?;
            validate_checkpoint(phase, &checkpoint)?;
            EventPayload::Checkpoint { phase, checkpoint }
        }
        EventKind::Committed => EventPayload::Committed {
            result: required_object_value(object, "result")?,
        },
        EventKind::RepairRequired => {
            let (phase_text, repair_code, repair_categories, partial_report) =
                validate_repair(object, false)?;
            let phase = ForwardPhase::parse(&phase_text).expect("validated forward repair phase");
            EventPayload::RepairRequired {
                phase,
                repair_code,
                repair_categories,
                partial_report,
            }
        }
        EventKind::RepairResumed => {
            if schema_version != IDENTIFY_OPERATION_SCHEMA_VERSION {
                return Err(IdentifyOperationError::RepairResumeRequiresSchemaVersion2);
            }
            let repair_event_id = required_str(object, "repair_event_id")?;
            let phase_text = required_str(object, "phase")?;
            let phase = ForwardPhase::parse(&phase_text)
                .ok_or(IdentifyOperationError::InvalidRepairPhase { phase: phase_text })?;
            EventPayload::RepairResumed {
                repair_event_id,
                phase,
            }
        }
        EventKind::UndoPrepared => EventPayload::UndoPrepared {
            undo_started_at: required_str(object, "undo_started_at")?,
        },
        EventKind::UndoCheckpoint => {
            let phase_text = required_str(object, "phase")?;
            let phase = UndoPhase::parse(&phase_text)
                .ok_or(IdentifyOperationError::InvalidUndoCheckpointPhase { phase: phase_text })?;
            EventPayload::UndoCheckpoint {
                phase,
                undo_report_delta: required_object_value(object, "undo_report_delta")?,
            }
        }
        EventKind::UndoCommitted => EventPayload::UndoCommitted {
            undo_report: required_object_value(object, "undo_report")?,
        },
        EventKind::UndoRepairRequired => {
            let (phase_text, repair_code, repair_categories, undo_report) =
                validate_repair(object, true)?;
            let phase = UndoPhase::parse(&phase_text).expect("validated undo repair phase");
            EventPayload::UndoRepairRequired {
                phase,
                repair_code,
                repair_categories,
                undo_report,
            }
        }
    };
    Ok(IdentifyOperationEvent {
        schema_version,
        event_id,
        operation_id,
        request_id,
        ts,
        caller,
        actor,
        payload,
    })
}

pub fn load_operations(path: &Path) -> Result<Vec<LedgerRow>, IdentifyOperationError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|source| IdentifyOperationError::ReadIo {
        path: path.to_path_buf(),
        source,
    })?;
    contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| (!line.trim().is_empty()).then_some((index + 1, line)))
        .map(|(line, raw)| LedgerRow::parse(path, line, raw))
        .collect()
}

/// Append one validated event. This is intentionally the only write operation.
pub fn append_event(
    path: &Path,
    event: &IdentifyOperationEvent,
) -> Result<(), IdentifyOperationError> {
    let value = event.to_json();
    let _ = validate_row(&value)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| IdentifyOperationError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let _lock = hold_lock(path, LockOptions::default())?;
    append_jsonl(path, &value)?;
    Ok(())
}

pub fn fold_operation(
    rows: &[LedgerRow],
    operation_id: &str,
) -> Result<Option<OperationState>, IdentifyOperationError> {
    let events = rows
        .iter()
        .filter(|row| row.event.operation_id == operation_id)
        .collect::<Vec<_>>();
    if events.is_empty() {
        return Ok(None);
    }
    fold_events(&events).map(Some)
}

pub fn fold_all_operations(
    rows: &[LedgerRow],
) -> Result<Vec<OperationState>, IdentifyOperationError> {
    let mut ids = BTreeSet::new();
    for row in rows {
        ids.insert(row.event.operation_id.clone());
    }
    ids.into_iter()
        .map(|id| fold_operation(rows, &id).map(|state| state.expect("known operation id")))
        .collect()
}

fn validate_prepared(
    plan: &Value,
    operation_id: &str,
    request_id: &str,
) -> Result<(), IdentifyOperationError> {
    let plan = plan
        .as_object()
        .ok_or(IdentifyOperationError::MissingOrInvalidField {
            field: "prepared_plan",
        })?;
    if plan.get("plan_schema_version").and_then(Value::as_i64) != Some(1) {
        return Err(IdentifyOperationError::InvalidPlanSchemaVersion);
    }
    if plan.get("operation_id").and_then(Value::as_str) != Some(operation_id) {
        return Err(IdentifyOperationError::PreparedOperationIdMismatch);
    }
    if plan.get("request_id").and_then(Value::as_str) != Some(request_id) {
        return Err(IdentifyOperationError::PreparedRequestIdMismatch);
    }
    for field in [
        "planned_at",
        "request",
        "cluster",
        "target",
        "entity_identity",
        "direct_voiceprints",
        "segments",
        "retro_confirm",
        "sentinel",
        "keep_separate_assertions",
    ] {
        if !plan.contains_key(field) {
            return Err(IdentifyOperationError::PreparedPlanMissing { field });
        }
    }
    let request = required_object(plan, "request")?;
    for field in [
        "cluster_id",
        "name",
        "entity_id",
        "resolve_only",
        "create_new",
        "entity_type",
        "reviewed_near_match_entity_ids",
    ] {
        if !request.contains_key(field) {
            return Err(IdentifyOperationError::PreparedRequestMissing { field });
        }
    }
    let cluster = required_object(plan, "cluster")?;
    let members = required_array(cluster, "members")?;
    if cluster.get("member_count").and_then(Value::as_i64) != Some(members.len() as i64) {
        return Err(IdentifyOperationError::ClusterMemberCountMismatch);
    }
    for member in members {
        let member = member
            .as_object()
            .ok_or(IdentifyOperationError::ClusterMemberNotObject)?;
        let _ = member_provenance(member)?;
    }
    let target = required_object(plan, "target")?;
    let _ = required_str(target, "entity_id")?;
    let _ = required_str(target, "entity_name")?;
    if !target.get("will_create").is_some_and(Value::is_boolean) {
        return Err(IdentifyOperationError::InvalidTargetWillCreate);
    }
    let _ = required_object(plan, "entity_identity")?;
    let _ = required_object(plan, "direct_voiceprints")?;
    let _ = required_array(plan, "segments")?;
    let _ = required_object(plan, "retro_confirm")?;
    let _ = required_object(plan, "sentinel")?;
    let _ = required_array(plan, "keep_separate_assertions")?;
    Ok(())
}

fn validate_checkpoint(
    phase: ForwardPhase,
    checkpoint: &Value,
) -> Result<(), IdentifyOperationError> {
    let checkpoint =
        checkpoint
            .as_object()
            .ok_or(IdentifyOperationError::MissingOrInvalidField {
                field: "checkpoint",
            })?;
    if checkpoint.get("phase_status").and_then(Value::as_str) != Some("complete") {
        return Err(IdentifyOperationError::IncompleteCheckpoint);
    }
    let _ = required_str(checkpoint, "completed_at")?;
    let _ = required_object(checkpoint, "counts")?;
    let _ = required_object(checkpoint, "skipped_reasons")?;
    match phase {
        ForwardPhase::Entity => {
            let _ = required_str(checkpoint, "entity_id")?;
            required_bool(checkpoint, "entity_created")?;
            let _ = required_str(checkpoint, "identity_after_hash")?;
            let _ = required_array(checkpoint, "history_event_refs")?;
        }
        ForwardPhase::KeepSeparate => {
            let _ = required_array(checkpoint, "pair_keys")?;
            required_int(checkpoint, "recorded_count")?;
            required_int(checkpoint, "already_present_count")?;
        }
        ForwardPhase::DirectVoiceprints => {
            let _ = required_array(checkpoint, "saved_keys")?;
            required_int(checkpoint, "saved_count")?;
            required_int(checkpoint, "skipped_existing_count")?;
        }
        ForwardPhase::Corrections => {
            let _ = required_array(checkpoint, "appended_keys")?;
            required_int(checkpoint, "appended_count")?;
            required_int(checkpoint, "skipped_existing_count")?;
            required_int(checkpoint, "segment_count")?;
        }
        ForwardPhase::Labels => {
            let _ = required_array(checkpoint, "patched_sentence_keys")?;
            let _ = required_array(checkpoint, "inserted_sentence_keys")?;
            required_int(checkpoint, "patched_count")?;
            required_int(checkpoint, "inserted_count")?;
            required_int(checkpoint, "skipped_already_intended_count")?;
            required_int(checkpoint, "segment_count")?;
        }
        ForwardPhase::RetroTracker => {
            required_bool(checkpoint, "matched")?;
            if checkpoint
                .get("candidate_id")
                .is_some_and(|value| !value.is_null() && !value.is_i64())
            {
                return Err(IdentifyOperationError::InvalidRetroCandidateId);
            }
            let _ = required_array(checkpoint, "saved_keys")?;
            required_int(checkpoint, "voiceprints_saved_count")?;
            required_int(checkpoint, "voiceprints_skipped_existing_count")?;
            required_bool(checkpoint, "tracker_updated")?;
        }
        ForwardPhase::Sentinel => {
            let _ = required_str(checkpoint, "cluster_key")?;
            required_bool(checkpoint, "written")?;
        }
    }
    Ok(())
}

fn validate_repair(
    object: &Map<String, Value>,
    undo: bool,
) -> Result<(String, String, Value, Value), IdentifyOperationError> {
    let phase_text = required_str(object, "phase")?;
    if undo {
        if UndoPhase::parse(&phase_text).is_none() {
            return Err(IdentifyOperationError::InvalidUndoRepairPhase { phase: phase_text });
        }
    } else if ForwardPhase::parse(&phase_text).is_none() {
        return Err(IdentifyOperationError::InvalidRepairPhase { phase: phase_text });
    }
    let repair_code = required_str(object, "repair_code")?;
    let repair_categories = required_object_value(object, "repair_categories")?;
    let report = required_object_value(
        object,
        if undo {
            "undo_report"
        } else {
            "partial_report"
        },
    )?;
    Ok((phase_text, repair_code, repair_categories, report))
}

fn fold_events(rows: &[&LedgerRow]) -> Result<OperationState, IdentifyOperationError> {
    let mut seen = HashMap::<&str, &LedgerRow>::new();
    let mut events = Vec::new();
    for row in rows {
        if let Some(existing) = seen.get(row.event.event_id.as_str()) {
            if existing.raw_json != row.raw_json {
                return Err(IdentifyOperationError::ConflictingDuplicateEventId {
                    event_id: row.event.event_id.clone(),
                });
            }
        } else {
            seen.insert(&row.event.event_id, row);
            events.push(*row);
        }
    }
    let prepared = events
        .iter()
        .filter(|row| matches!(row.event.payload, EventPayload::Prepared { .. }))
        .collect::<Vec<_>>();
    if prepared.len() != 1 {
        return Err(IdentifyOperationError::PreparedEventCount);
    }
    let prepared_event = &prepared[0].event;
    let (request_fingerprint, prepared_plan) = match &prepared_event.payload {
        EventPayload::Prepared {
            request_fingerprint,
            prepared_plan,
        } => (request_fingerprint.clone(), prepared_plan.clone()),
        _ => unreachable!(),
    };
    let mut phase_checkpoints = BTreeMap::new();
    let mut undo_phase_checkpoints = BTreeMap::new();
    for row in &events {
        match &row.event.payload {
            EventPayload::Checkpoint { phase, checkpoint } => {
                if let Some(previous) = phase_checkpoints.insert(*phase, checkpoint.clone())
                    && previous != *checkpoint
                {
                    return Err(IdentifyOperationError::ConflictingCheckpoint {
                        phase: phase.as_str().to_owned(),
                    });
                }
            }
            EventPayload::UndoCheckpoint {
                phase,
                undo_report_delta,
            } => {
                if let Some(previous) =
                    undo_phase_checkpoints.insert(*phase, undo_report_delta.clone())
                    && previous != *undo_report_delta
                {
                    return Err(IdentifyOperationError::ConflictingUndoCheckpoint {
                        phase: phase.as_str().to_owned(),
                    });
                }
            }
            _ => {}
        }
    }
    let completed_phases = FORWARD_PHASE_ORDER
        .iter()
        .copied()
        .filter(|phase| phase_checkpoints.contains_key(phase))
        .collect::<Vec<_>>();
    let lifecycle = lifecycle(&events)?;
    let terminal_status = lifecycle.terminal_status;
    let plan = prepared_plan.as_object().expect("validated prepared plan");
    let request = plan["request"].as_object().expect("validated request");
    let target = plan["target"].as_object().expect("validated target");
    let members = plan["cluster"]["members"]
        .as_array()
        .expect("validated members")
        .iter()
        .map(|member| member_provenance(member.as_object().expect("validated member")))
        .collect::<Result<_, _>>()?;
    let pending_phases = pending_phases(
        terminal_status,
        &completed_phases,
        lifecycle.repair_required,
        &events,
    );
    Ok(OperationState {
        operation_id: prepared_event.operation_id.clone(),
        request_id: prepared_event.request_id.clone(),
        request_fingerprint,
        cluster_member_set: members,
        target_entity_id: target
            .get("entity_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        target_entity_name: target
            .get("entity_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        will_create: target
            .get("will_create")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        entity_type: request
            .get("entity_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reviewed_near_match_entity_ids: request
            .get("reviewed_near_match_entity_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.to_string().trim_matches('"').to_owned())
                    .collect()
            })
            .unwrap_or_default(),
        completed_phases,
        pending_phases,
        terminal_status,
        result: last_object_payload(&events, EventKind::Committed, "result"),
        undo_report: last_object_payload(&events, EventKind::UndoCommitted, "undo_report"),
        undo_started_at: events
            .iter()
            .rev()
            .find_map(|row| match &row.event.payload {
                EventPayload::UndoPrepared { undo_started_at } => Some(undo_started_at.clone()),
                _ => None,
            }),
        undo_committed_count: events
            .iter()
            .filter(|row| row.event.event_kind() == EventKind::UndoCommitted)
            .count(),
        phase_checkpoints,
        prepared_plan,
        repair_required: lifecycle.repair_required.cloned(),
        undo_repair_required: events
            .iter()
            .rev()
            .find(|row| row.event.event_kind() == EventKind::UndoRepairRequired)
            .map(|row| row.event.clone()),
        undo_phase_checkpoints,
    })
}

struct Lifecycle<'a> {
    terminal_status: TerminalStatus,
    repair_required: Option<&'a IdentifyOperationEvent>,
}

fn lifecycle<'a>(events: &[&'a LedgerRow]) -> Result<Lifecycle<'a>, IdentifyOperationError> {
    let mut terminal_status = TerminalStatus::InProgress;
    let mut repair_required = None;
    let mut committed_or_undo_started = false;

    for row in events {
        match &row.event.payload {
            EventPayload::Prepared { .. } | EventPayload::Checkpoint { .. } => {}
            EventPayload::RepairRequired { .. } => {
                terminal_status = TerminalStatus::RepairRequired;
                repair_required = Some(&row.event);
            }
            EventPayload::RepairResumed {
                repair_event_id,
                phase,
            } => {
                let Some(outstanding) = repair_required else {
                    return Err(IdentifyOperationError::InvalidRepairResume {
                        event_id: row.event.event_id.clone(),
                    });
                };
                let EventPayload::RepairRequired {
                    phase: repair_phase,
                    repair_code,
                    ..
                } = &outstanding.payload
                else {
                    unreachable!("outstanding repair has repair payload");
                };
                if terminal_status != TerminalStatus::RepairRequired
                    || committed_or_undo_started
                    || outstanding.event_id != *repair_event_id
                    || repair_phase != phase
                    || repair_code != OWNER_IDENTITY_INVALID_REASON
                {
                    return Err(IdentifyOperationError::InvalidRepairResume {
                        event_id: row.event.event_id.clone(),
                    });
                }
                terminal_status = TerminalStatus::InProgress;
                repair_required = None;
            }
            EventPayload::Committed { .. } => {
                terminal_status = TerminalStatus::Committed;
                repair_required = None;
                committed_or_undo_started = true;
            }
            EventPayload::UndoPrepared { .. } | EventPayload::UndoCheckpoint { .. } => {
                terminal_status = TerminalStatus::Undoing;
                repair_required = None;
                committed_or_undo_started = true;
            }
            EventPayload::UndoCommitted { .. } => {
                terminal_status = TerminalStatus::Undone;
                repair_required = None;
                committed_or_undo_started = true;
            }
            EventPayload::UndoRepairRequired { .. } => {
                terminal_status = TerminalStatus::UndoRepairRequired;
                repair_required = None;
                committed_or_undo_started = true;
            }
        }
    }

    Ok(Lifecycle {
        terminal_status,
        repair_required,
    })
}

fn pending_phases(
    terminal: TerminalStatus,
    completed: &[ForwardPhase],
    repair_required: Option<&IdentifyOperationEvent>,
    events: &[&LedgerRow],
) -> Vec<String> {
    match terminal {
        TerminalStatus::InProgress => FORWARD_PHASE_ORDER
            .iter()
            .filter(|phase| !completed.contains(phase))
            .map(|phase| phase.as_str().to_owned())
            .collect(),
        TerminalStatus::RepairRequired => repair_required
            .and_then(|event| match &event.payload {
                EventPayload::RepairRequired { partial_report, .. } => partial_report
                    .get("pending_phases")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .map(Value::to_string)
                            .map(|value| value.trim_matches('"').to_owned())
                            .collect()
                    }),
                _ => None,
            })
            .unwrap_or_default(),
        TerminalStatus::Undoing => UNDO_PHASE_ORDER
            .iter()
            .filter(|phase| {
                !events.iter().any(|row| {
                    matches!(
                        &row.event.payload,
                        EventPayload::UndoCheckpoint {
                            phase: completed_phase,
                            ..
                        } if completed_phase == *phase
                    )
                })
            })
            .map(|phase| phase.as_str().to_owned())
            .collect(),
        _ => Vec::new(),
    }
}
fn last_object_payload(events: &[&LedgerRow], kind: EventKind, field: &str) -> Option<Value> {
    events
        .iter()
        .rev()
        .find(|row| row.event.event_kind() == kind)
        .and_then(|row| row.event.to_json().get(field).cloned())
        .filter(Value::is_object)
}

fn member_provenance(
    member: &Map<String, Value>,
) -> Result<MemberProvenance, IdentifyOperationError> {
    Ok(MemberProvenance {
        day: required_str(member, "day")
            .map_err(|_| IdentifyOperationError::InvalidClusterMemberProvenance)?,
        stream: required_str(member, "stream")
            .map_err(|_| IdentifyOperationError::InvalidClusterMemberProvenance)?,
        segment_key: required_str(member, "segment_key")
            .map_err(|_| IdentifyOperationError::InvalidClusterMemberProvenance)?,
        source: required_str(member, "source")
            .map_err(|_| IdentifyOperationError::InvalidClusterMemberProvenance)?,
        sentence_id: member
            .get("sentence_id")
            .and_then(Value::as_i64)
            .ok_or(IdentifyOperationError::InvalidClusterMemberProvenance)?,
    })
}
fn required_str(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, IdentifyOperationError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(IdentifyOperationError::MissingOrInvalidField { field })
}
fn required_object<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Map<String, Value>, IdentifyOperationError> {
    object
        .get(field)
        .and_then(Value::as_object)
        .ok_or(IdentifyOperationError::MissingOrInvalidField { field })
}
fn required_object_value(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Value, IdentifyOperationError> {
    Ok(Value::Object(required_object(object, field)?.clone()))
}
fn required_array<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Vec<Value>, IdentifyOperationError> {
    object
        .get(field)
        .and_then(Value::as_array)
        .ok_or(IdentifyOperationError::MissingOrInvalidField { field })
}
fn required_int(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<i64, IdentifyOperationError> {
    object
        .get(field)
        .and_then(Value::as_i64)
        .ok_or(IdentifyOperationError::MissingOrInvalidField { field })
}
fn required_bool(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<bool, IdentifyOperationError> {
    object
        .get(field)
        .and_then(Value::as_bool)
        .ok_or(IdentifyOperationError::MissingOrInvalidField { field })
}
