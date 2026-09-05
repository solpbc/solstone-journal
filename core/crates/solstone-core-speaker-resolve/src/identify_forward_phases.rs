// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Replay-safe forward phases used by the identify operation executor.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::segment_path;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_entity::{
    EncoderIdentity, EntityOperationContext, EntityOperationKind, JournalEntity, VoiceprintItem,
    create_journal_entity, load_entity_voiceprints_file, normalize_embedding, read_entity_identity,
    read_visible_history,
};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use solstone_core_speaker_id::corrections::append_correction;
use solstone_core_speaker_id::labels::patch_labels;
use thiserror::Error;

use crate::candidate_tracker::{CandidateTracker, CandidateTrackerError};
use crate::direct_voiceprints::DirectVoiceprintKey;
use crate::identify_operations::ForwardPhase;
use crate::keep_separate::{KeepSeparateError, find_assertion, record_keep_separate_assertion};
use crate::owner_admission::{OWNER_IDENTITY_INVALID_REASON, OwnerAdmission, admitted_owner_id};
use crate::owner_centroid::{OwnerCentroidError, load_owner_centroid};
use crate::retroactive_confirm::{
    RetroactiveConfirmError, RetroactiveConfirmPlan, apply_retroactive_confirm_plan,
};

#[derive(Debug, Clone, PartialEq)]
pub struct EntityPhasePlan {
    pub target_entity_id: String,
    pub will_create: bool,
    pub intended_identity: Value,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepSeparatePhaseEntry {
    pub pair_key: String,
    pub entity_id_a: String,
    pub entity_id_b: String,
    pub source_kind: String,
    pub detection_count_used: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentCorrectionPlan {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub rows_to_append: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelPlanItem {
    pub sentence_id: i64,
    pub intended_label: Value,
    pub prior_state: String,
    pub prior_label: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentLabelPlan {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub labels: Vec<LabelPlanItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetroVoiceprintEntry {
    pub key: DirectVoiceprintKey,
    pub metadata: Value,
    pub item: VoiceprintItem,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetroTrackerPhasePlan {
    pub matched: bool,
    pub candidate_id: Option<i64>,
    pub target_entity_id: String,
    pub planning_owner_entity_id: Option<String>,
    pub candidate_before: Option<Value>,
    pub candidate_after: Option<Value>,
    pub voiceprints_to_add: Vec<RetroVoiceprintEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SentinelPhasePlan {
    pub cluster_key: String,
    pub prior_entry: Option<Value>,
    pub intended_entry: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseResult {
    pub fields: Value,
}

#[derive(Debug, Error)]
pub enum ForwardPhaseError {
    #[error("entity operation failed: {0}")]
    Entity(#[from] solstone_core_entity::EntityLifecycleError),
    #[error("entity read failed: {0}")]
    EntityStore(#[from] solstone_core_entity::EntityStoreError),
    #[error("keep-separate operation failed: {0}")]
    KeepSeparate(#[from] KeepSeparateError),
    #[error("speaker correction operation failed: {0}")]
    Corrections(#[from] solstone_core_speaker_id::corrections::CorrectionsError),
    #[error("speaker label operation failed: {0}")]
    Labels(#[from] solstone_core_speaker_id::labels::LabelsError),
    #[error("candidate tracker operation failed: {0}")]
    Tracker(#[from] CandidateTrackerError),
    #[error("retroactive confirmation failed: {0}")]
    Retroactive(#[from] RetroactiveConfirmError),
    #[error("segment path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    #[error("resolved-cluster cache I/O failed at {path}: {source}")]
    SentinelIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("repair required during {phase:?}: {code}")]
    RepairRequired {
        phase: ForwardPhase,
        code: &'static str,
        categories: BTreeMap<String, usize>,
        /// `None` lets the identify orchestrator record pending phases; `Some` is verbatim.
        partial_report: Option<Value>,
    },
}

pub fn phase_entity(
    journal_root: &Path,
    plan: &EntityPhasePlan,
) -> Result<PhaseResult, ForwardPhaseError> {
    let mut entity_created = false;
    let mut current = load_entity(journal_root, &plan.target_entity_id)?;
    if plan.will_create {
        let expected = &plan.intended_identity;
        if current.is_none() {
            let name = expected
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let entity_type = expected
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let context = EntityOperationContext {
                kind: EntityOperationKind::Create,
                caller: Value::String("speaker_resolve.identify_cluster".to_owned()),
                actor: Value::Null,
                metadata: json!({"operation_kind":"speaker_identify","operation_id":plan.operation_id}),
            };
            match create_journal_entity(
                journal_root,
                &plan.target_entity_id,
                name,
                entity_type,
                None,
                None,
                &[],
                true,
                Some(&context),
            ) {
                Ok(_) => entity_created = true,
                Err(solstone_core_entity::EntityLifecycleError::EntityAlreadyExists { .. }) => {}
                Err(error) => return Err(error.into()),
            }
            current = load_entity(journal_root, &plan.target_entity_id)?;
        } else {
            entity_created = true;
        }
        let Some(entity) = current.as_ref() else {
            return Err(repair(
                ForwardPhase::Entity,
                "entity_missing",
                "entity",
                Some(Value::Null),
            ));
        };
        if meaningful_identity(&entity.value) != meaningful_identity(expected) {
            return Err(repair(
                ForwardPhase::Entity,
                "concurrent_change",
                "concurrent_change",
                Some(Value::Null),
            ));
        }
        let history_event_refs =
            history_refs(journal_root, &plan.target_entity_id, &plan.operation_id)?;
        if history_event_refs.len() != 1 {
            return Err(repair(
                ForwardPhase::Entity,
                "concurrent_change",
                "concurrent_change",
                Some(json!({"history_event_refs": history_event_refs})),
            ));
        }
        return Ok(PhaseResult {
            fields: json!({
                "entity_id": plan.target_entity_id,
                "entity_created": entity_created,
                "identity_after_hash": identity_hash(&entity.value),
                "identity_after": entity.value,
                "history_event_refs": history_event_refs,
                "counts": {"entity_created": i64::from(entity_created)},
                "skipped_reasons": {},
            }),
        });
    }
    let Some(entity) = current.take() else {
        return Err(repair(
            ForwardPhase::Entity,
            "entity_missing",
            "entity",
            Some(Value::Null),
        ));
    };
    Ok(PhaseResult {
        fields: json!({
            "entity_id": plan.target_entity_id, "entity_created": false,
            "identity_after_hash": identity_hash(&entity.value), "identity_after": entity.value,
            "history_event_refs": [], "counts": {"entity_created": 0}, "skipped_reasons": {},
        }),
    })
}

pub fn phase_keep_separate(
    journal_root: &Path,
    operation_id: &str,
    entries: &[KeepSeparatePhaseEntry],
) -> Result<PhaseResult, ForwardPhaseError> {
    let mut recorded = 0usize;
    let mut already = 0usize;
    let mut pair_keys = entries
        .iter()
        .map(|entry| entry.pair_key.clone())
        .collect::<Vec<_>>();
    pair_keys.sort();
    pair_keys.dedup();
    for entry in entries {
        let present = find_assertion(journal_root, &entry.entity_id_a, &entry.entity_id_b)?
            .is_some_and(|assertion| {
                assertion.sources.iter().any(|source| {
                    source.source_kind == entry.source_kind
                        && source.operation_id.as_deref() == Some(operation_id)
                })
            });
        if present {
            already += 1;
            continue;
        }
        record_keep_separate_assertion(
            journal_root,
            &entry.entity_id_a,
            &entry.entity_id_b,
            &entry.source_kind,
            Some(operation_id),
            entry.detection_count_used,
        )?;
        recorded += 1;
    }
    Ok(PhaseResult {
        fields: json!({"pair_keys":pair_keys,"recorded_count":recorded,"already_present_count":already,"counts":{"recorded":recorded,"already_present":already},"skipped_reasons":{}}),
    })
}

pub fn phase_corrections(
    journal_root: &Path,
    operation_id: &str,
    segments: &[SegmentCorrectionPlan],
) -> Result<PhaseResult, ForwardPhaseError> {
    let mut appended = Vec::new();
    let mut skipped = 0usize;
    let mut segment_count = 0usize;
    for segment in segments {
        let directory = segment_path(
            journal_root,
            &segment.day,
            &segment.segment_key,
            &segment.stream,
            false,
        )?;
        let mut existing = load_corrections(&directory);
        let mut changed = false;
        for row in &segment.rows_to_append {
            let sentence_id = row
                .get("sentence_id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let corrected = row.get("corrected_speaker");
            let natural = existing
                .iter()
                .filter(|existing| {
                    existing.get("sentence_id") == Some(&Value::from(sentence_id))
                        && existing.get("corrected_speaker") == corrected
                })
                .collect::<Vec<_>>();
            if !natural.is_empty() {
                if natural.iter().any(|existing| {
                    existing.get("operation_id").and_then(Value::as_str) == Some(operation_id)
                        && existing.get("correction_kind").and_then(Value::as_str)
                            == Some("identify")
                }) {
                    appended.push(sentence_key(segment, sentence_id));
                    changed = true;
                } else {
                    skipped += 1;
                }
                continue;
            }
            let object = row.as_object().cloned().ok_or_else(|| {
                repair(
                    ForwardPhase::Corrections,
                    "concurrent_change",
                    "segment_correction",
                    Some(Value::Null),
                )
            })?;
            append_correction(&directory, object)?;
            existing.push(row.clone());
            appended.push(sentence_key(segment, sentence_id));
            changed = true;
        }
        if changed {
            segment_count += 1;
        }
    }
    sort_keys(&mut appended);
    Ok(PhaseResult {
        fields: json!({"appended_keys":appended,"appended_count":appended.len(),"skipped_existing_count":skipped,"segment_count":segment_count,"counts":{"appended":appended.len()},"skipped_reasons":{"existing":skipped}}),
    })
}

pub fn phase_labels(
    journal_root: &Path,
    segments: &[SegmentLabelPlan],
) -> Result<PhaseResult, ForwardPhaseError> {
    let mut patched = Vec::new();
    let mut inserted = Vec::new();
    let mut already = 0usize;
    let mut segment_count = 0usize;
    for segment in segments {
        let directory = segment_path(
            journal_root,
            &segment.day,
            &segment.segment_key,
            &segment.stream,
            false,
        )?;
        let current = load_labels(&directory);
        let mut patches = Vec::<(i64, Map<String, Value>)>::new();
        let mut changed = false;
        for item in &segment.labels {
            let now = current.get(&item.sentence_id);
            let plan_changed = item.prior_label.as_ref() != Some(&item.intended_label);
            if now == Some(&item.intended_label) {
                if plan_changed {
                    if item.prior_state == "absent" {
                        inserted.push(sentence_key(segment, item.sentence_id));
                    } else {
                        patched.push(sentence_key(segment, item.sentence_id));
                    }
                    changed = true;
                } else {
                    already += 1;
                }
                continue;
            }
            if item.prior_state == "absent" && now.is_none() {
                patches.push((item.sentence_id, label_patch_fields(&item.intended_label)?));
                inserted.push(sentence_key(segment, item.sentence_id));
                changed = true;
            } else if item.prior_state == "present" && now == item.prior_label.as_ref() {
                patches.push((item.sentence_id, label_patch_fields(&item.intended_label)?));
                patched.push(sentence_key(segment, item.sentence_id));
                changed = true;
            } else {
                return Err(repair(
                    ForwardPhase::Labels,
                    "concurrent_change",
                    "concurrent_change",
                    Some(json!({"segment":segment.segment_key,"sentence_id":item.sentence_id})),
                ));
            }
        }
        if !patches.is_empty() {
            patch_labels(&directory, &patches, true)?;
        }
        if changed {
            segment_count += 1;
        }
    }
    sort_keys(&mut patched);
    sort_keys(&mut inserted);
    Ok(PhaseResult {
        fields: json!({"patched_sentence_keys":patched,"inserted_sentence_keys":inserted,"patched_count":patched.len(),"inserted_count":inserted.len(),"skipped_already_intended_count":already,"segment_count":segment_count,"counts":{"patched":patched.len(),"inserted":inserted.len()},"skipped_reasons":{"already_intended":already}}),
    })
}

pub fn phase_retro_tracker(
    journal_root: &Path,
    tracker: &mut CandidateTracker,
    plan: &RetroTrackerPhasePlan,
    encoder: &EncoderIdentity,
) -> Result<PhaseResult, ForwardPhaseError> {
    let owner_id = match admitted_owner_id(journal_root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => {
            return Err(repair(
                ForwardPhase::RetroTracker,
                OWNER_IDENTITY_INVALID_REASON,
                "owner_identity",
                None,
            ));
        }
    };
    let Some(planning_owner_entity_id) = plan.planning_owner_entity_id.as_deref() else {
        return Err(repair(
            ForwardPhase::RetroTracker,
            "speaker_identify_plan_owner_unbound",
            "owner_plan",
            None,
        ));
    };
    if owner_id != planning_owner_entity_id {
        return Err(repair(
            ForwardPhase::RetroTracker,
            "speaker_identify_plan_owner_changed",
            "owner_identity",
            None,
        ));
    }
    let Some(candidate_id) = plan.matched.then_some(plan.candidate_id).flatten() else {
        return Ok(PhaseResult {
            fields: json!({"matched":false,"candidate_id":null,"saved_keys":[],"voiceprints_saved_count":0,"voiceprints_skipped_existing_count":0,"tracker_updated":false,"counts":{},"skipped_reasons":{}}),
        });
    };
    let owner = match load_owner_centroid(journal_root, &owner_id) {
        Ok(Some(owner)) => owner,
        Ok(None) => {
            return Err(repair(
                ForwardPhase::RetroTracker,
                "owner_centroid_unavailable",
                "owner_centroid",
                None,
            ));
        }
        Err(OwnerCentroidError::IdentityInvalid | OwnerCentroidError::TargetMismatch { .. }) => {
            return Err(repair(
                ForwardPhase::RetroTracker,
                OWNER_IDENTITY_INVALID_REASON,
                "owner_identity",
                None,
            ));
        }
        Err(_) => {
            return Err(repair(
                ForwardPhase::RetroTracker,
                "owner_centroid_unavailable",
                "owner_centroid",
                None,
            ));
        }
    };
    for entry in &plan.voiceprints_to_add {
        let Some(embedding) = normalize_embedding(&entry.item.embedding) else {
            return Err(repair(
                ForwardPhase::RetroTracker,
                "retro_embedding_invalid",
                "retro_embedding",
                None,
            ));
        };
        if dot(&embedding, &owner.centroid) >= owner.threshold {
            return Err(repair(
                ForwardPhase::RetroTracker,
                "owner_similarity",
                "owner_centroid",
                None,
            ));
        }
    }
    let metadata = voiceprint_metadata(journal_root, &plan.target_entity_id);
    let mut saved = Vec::new();
    let mut items = Vec::new();
    for entry in &plan.voiceprints_to_add {
        match metadata.get(&entry.key) {
            None => {
                saved.push(entry.key.clone());
                items.push(entry.item.clone());
            }
            Some(rows) if rows.iter().any(|row| row == &entry.metadata) => {
                saved.push(entry.key.clone())
            }
            Some(_) => {
                return Err(repair(
                    ForwardPhase::RetroTracker,
                    "voiceprint_metadata_mismatch",
                    "voiceprint",
                    Some(Value::Null),
                ));
            }
        }
    }
    let candidate = tracker
        .snapshot_candidates_locked()?
        .into_iter()
        .find(|candidate| candidate.cand_id == candidate_id)
        .ok_or_else(|| {
            repair(
                ForwardPhase::RetroTracker,
                "candidate_missing",
                "speaker_candidate",
                Some(Value::Null),
            )
        })?;
    let candidate_json = candidate.to_json();
    if Some(&candidate_json) != plan.candidate_before.as_ref()
        && Some(&candidate_json) != plan.candidate_after.as_ref()
    {
        return Err(repair(
            ForwardPhase::RetroTracker,
            "concurrent_change",
            "concurrent_change",
            Some(Value::Null),
        ));
    }
    let tracker_updated = plan.candidate_before != plan.candidate_after;
    if tracker_updated || !items.is_empty() {
        let apply = RetroactiveConfirmPlan {
            matched: true,
            candidate_id: Some(candidate_id),
            entity_id: plan.target_entity_id.clone(),
            items,
        };
        apply_retroactive_confirm_plan(tracker, journal_root, &apply, encoder)?;
    }
    let after = voiceprint_metadata(journal_root, &plan.target_entity_id);
    saved = plan
        .voiceprints_to_add
        .iter()
        .filter(|entry| {
            after
                .get(&entry.key)
                .is_some_and(|rows| rows.iter().any(|row| row == &entry.metadata))
        })
        .map(|entry| entry.key.clone())
        .collect();
    saved.sort();
    saved.dedup();
    let skipped = plan.voiceprints_to_add.len().saturating_sub(saved.len());
    Ok(PhaseResult {
        fields: json!({"matched":true,"candidate_id":candidate_id,"saved_keys":saved.iter().map(DirectVoiceprintKey::to_json).collect::<Vec<_>>(),"voiceprints_saved_count":saved.len(),"voiceprints_skipped_existing_count":skipped,"tracker_updated":tracker_updated,"counts":{"saved":saved.len(),"tracker_updated":i64::from(tracker_updated)},"skipped_reasons":{"existing":skipped}}),
    })
}

pub fn phase_sentinel(
    journal_root: &Path,
    plan: &SentinelPhasePlan,
) -> Result<PhaseResult, ForwardPhaseError> {
    let mut values = load_resolved_clusters(journal_root);
    let current = values.get(&plan.cluster_key);
    if current != Some(&plan.intended_entry) {
        if current == plan.prior_entry.as_ref() || (current.is_none() && plan.prior_entry.is_none())
        {
            values.insert(plan.cluster_key.clone(), plan.intended_entry.clone());
            replace_resolved_clusters(journal_root, &values)?;
        } else {
            return Err(repair(
                ForwardPhase::Sentinel,
                "concurrent_change",
                "concurrent_change",
                Some(Value::Null),
            ));
        }
    }
    Ok(PhaseResult {
        fields: json!({"cluster_key":plan.cluster_key,"written":true,"counts":{"written":1},"skipped_reasons":{}}),
    })
}

fn load_entity(
    root: &Path,
    entity_id: &str,
) -> Result<Option<JournalEntity>, solstone_core_entity::EntityStoreError> {
    Ok(
        read_entity_identity(root, entity_id)?.map(|snapshot| JournalEntity {
            id: snapshot.entity_id().to_owned(),
            value: snapshot.value().clone(),
        }),
    )
}
fn history_refs(
    root: &Path,
    entity_id: &str,
    operation_id: &str,
) -> Result<Vec<Value>, solstone_core_entity::EntityStoreError> {
    let references = read_visible_history(root, entity_id)?
        .into_iter()
        .filter_map(|event| {
            let value = event.value();
            (value
                .get("operation")
                .and_then(Value::as_object)
                .and_then(|operation| operation.get("operation_id"))
                .and_then(Value::as_str)
                == Some(operation_id))
            .then(|| {
                let sequence = value.get("seq").and_then(Value::as_i64).unwrap_or_default();
                let version_id = value
                    .get("version_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                json!({
                    "version_id": value.get("version_id"),
                    "seq": value.get("seq"),
                    "path": format!("entities/{entity_id}/history/events/{sequence:020}-{version_id}.json"),
                })
            })
        })
        .collect();
    Ok(references)
}
fn meaningful_identity(value: &Value) -> Value {
    let mut object = Map::new();
    for field in [
        "id",
        "name",
        "type",
        "aka",
        "emails",
        "is_principal",
        "blocked",
    ] {
        if let Some(value) = value.get(field) {
            object.insert(field.to_owned(), value.clone());
        }
    }
    Value::Object(object)
}
fn identity_hash(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&sorted_json(&meaningful_identity(value))).expect("JSON serializes")
        )
    )
}

fn sorted_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut sorted = BTreeMap::new();
            for (key, child) in object {
                sorted.insert(key.clone(), sorted_json(child));
            }
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(values.iter().map(sorted_json).collect()),
        _ => value.clone(),
    }
}
fn sentence_key<T: SegmentIdentity>(segment: &T, sentence_id: i64) -> Value {
    json!({"day":segment.day(),"segment_key":segment.segment_key(),"stream":segment.stream(),"sentence_id":sentence_id})
}
trait SegmentIdentity {
    fn day(&self) -> &str;
    fn stream(&self) -> &str;
    fn segment_key(&self) -> &str;
}
impl SegmentIdentity for SegmentCorrectionPlan {
    fn day(&self) -> &str {
        &self.day
    }
    fn stream(&self) -> &str {
        &self.stream
    }
    fn segment_key(&self) -> &str {
        &self.segment_key
    }
}
impl SegmentIdentity for SegmentLabelPlan {
    fn day(&self) -> &str {
        &self.day
    }
    fn stream(&self) -> &str {
        &self.stream
    }
    fn segment_key(&self) -> &str {
        &self.segment_key
    }
}
fn sort_keys(values: &mut [Value]) {
    values.sort_by_key(|value| serde_json::to_string(value).expect("JSON serializes"));
}
/// Read the current per-sentence labels for prepared-plan snapshotting.
pub(crate) fn load_labels(segment: &Path) -> HashMap<i64, Value> {
    fs::read(segment.join("talents/speaker_labels.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|label| {
            label
                .get("sentence_id")
                .and_then(Value::as_i64)
                .map(|sid| (sid, label))
        })
        .collect()
}

/// Read the current correction rows for prepared-plan snapshotting.
pub(crate) fn load_corrections(segment: &Path) -> Vec<Value> {
    fs::read(segment.join("talents/speaker_corrections.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("corrections").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(Value::is_object)
        .collect()
}
fn label_patch_fields(intended: &Value) -> Result<Map<String, Value>, ForwardPhaseError> {
    let object = intended.as_object().ok_or_else(|| {
        repair(
            ForwardPhase::Labels,
            "concurrent_change",
            "segment_label",
            Some(Value::Null),
        )
    })?;
    Ok(["speaker", "confidence", "method"]
        .into_iter()
        .filter_map(|field| {
            object
                .get(field)
                .map(|value| (field.to_owned(), value.clone()))
        })
        .collect())
}
fn voiceprint_metadata(root: &Path, entity_id: &str) -> BTreeMap<DirectVoiceprintKey, Vec<Value>> {
    load_entity_voiceprints_file(root, entity_id)
        .into_iter()
        .flat_map(|archive| archive.metadata)
        .filter_map(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter_map(|value| direct_key_from_metadata(&value).map(|key| (key, value)))
        .fold(BTreeMap::new(), |mut all, (key, value)| {
            all.entry(key).or_default().push(value);
            all
        })
}
fn direct_key_from_metadata(value: &Value) -> Option<DirectVoiceprintKey> {
    Some(DirectVoiceprintKey {
        day: value.get("day")?.as_str()?.to_owned(),
        segment_key: value.get("segment_key")?.as_str()?.to_owned(),
        source: value.get("source")?.as_str()?.to_owned(),
        sentence_id: value.get("sentence_id")?.as_i64()?,
    })
}
fn resolved_path(root: &Path) -> PathBuf {
    root.join("awareness/discovery_clusters.resolved.json")
}
/// Read the Python-owned resolved-cluster sentinel cache tolerantly.
pub(crate) fn load_resolved_clusters(root: &Path) -> BTreeMap<String, Value> {
    fs::read(resolved_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .map(|object| object.into_iter().collect())
        .unwrap_or_default()
}
/// Replace the resolved-cluster sentinel cache after a compare-and-swap decision.
pub(crate) fn replace_resolved_clusters(
    root: &Path,
    values: &BTreeMap<String, Value>,
) -> Result<(), ForwardPhaseError> {
    let path = resolved_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ForwardPhaseError::SentinelIo {
            path: path.clone(),
            source,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(values).expect("JSON serializes");
    atomic_replace(&path, &bytes, AtomicWriteOptions::default()).map_err(|source| {
        ForwardPhaseError::SentinelIo {
            path,
            source: std::io::Error::other(source),
        }
    })
}
// Existing `Some(Value::Null)` reports remain intentionally non-recordable pending §8 follow-up 1.
fn repair(
    phase: ForwardPhase,
    code: &'static str,
    category: &str,
    partial_report: Option<Value>,
) -> ForwardPhaseError {
    ForwardPhaseError::RepairRequired {
        phase,
        code,
        categories: BTreeMap::from([(category.to_owned(), 1)]),
        partial_report,
    }
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
