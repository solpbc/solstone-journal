// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Replay-safe undo phases for a committed identify operation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::segment_path;
use serde_json::{Map, Value, json};
use solstone_core_entity::{EncoderIdentity, VoiceprintRemoval, remove_voiceprints_by_key};
use solstone_core_facets::{EntityHistoryReference, delete_created_entity_if_unreferenced};
use solstone_core_speaker_id::corrections::{append_correction, read_corrections};
use solstone_core_speaker_id::labels::{LabelRestoration, restore_label_rows};
use thiserror::Error;

use crate::candidate_tracker::{CandidateTracker, CandidateTrackerError};
use crate::identify_forward_phases::{
    ForwardPhaseError, load_resolved_clusters, replace_resolved_clusters,
};
use crate::identify_operations::{ForwardPhase, OperationState};
use crate::keep_separate::{KeepSeparateError, remove_operation_sources};

type PlanKey = (String, String, String, i64);

/// Failures from an undo phase. The undo executor is responsible for mapping these.
#[derive(Debug, Error)]
pub enum UndoPhaseError {
    #[error("speaker label restore failed: {0}")]
    Labels(#[from] solstone_core_speaker_id::labels::LabelsError),
    #[error("speaker correction restore failed: {0}")]
    Corrections(#[from] solstone_core_speaker_id::corrections::CorrectionsError),
    #[error("voiceprint removal failed: {0}")]
    Voiceprints(#[from] solstone_core_entity::VoiceprintOperationError),
    #[error("candidate restore failed: {0}")]
    Tracker(#[from] CandidateTrackerError),
    #[error("keep-separate restore failed: {0}")]
    KeepSeparate(#[from] KeepSeparateError),
    #[error("resolved-cluster restore failed: {0}")]
    Sentinel(#[from] ForwardPhaseError),
    #[error("created entity restore failed: {0}")]
    Entity(#[from] solstone_core_facets::FacetEntityLifecycleError),
    #[error("segment path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
}

/// The durable zero-value undo report shape shared by every undo phase.
#[must_use]
pub fn empty_undo_report(operation_id: &str, status: &str) -> Value {
    json!({
        "status":status,
        "operation_id":operation_id,
        "undo_report":{
            "labels":category(json!({"removed_inserted_count":0,"patched_existing_count":0})),
            "corrections":category(json!({"appended_count":0,"already_present_count":0})),
            "voiceprints":category(json!({"removed_count":0,"missing_count":0,"metadata_mismatch_count":0})),
            "tracker":category(json!({"restored_candidate_count":0})),
            "sentinel":category(json!({"removed_count":0,"restored_prior_count":0})),
            "entity":category(json!({"deleted":false,"blocked_categories":[],"keep_separate_sources_removed_count":0})),
        }
    })
}

/// Undo the label rows that identify inserted or patched.
pub fn undo_labels(journal_root: &Path, state: &OperationState) -> Result<Value, UndoPhaseError> {
    let mut report = category(json!({"removed_inserted_count":0,"patched_existing_count":0}));
    let Some(checkpoint) = state.phase_checkpoints.get(&ForwardPhase::Labels) else {
        return Ok(json!({"labels":report}));
    };
    let map = label_plan_map(&state.prepared_plan);
    let mut grouped = BTreeMap::<(String, String, String), Vec<LabelRestoration>>::new();
    for key in checkpoint_keys(
        checkpoint,
        &["patched_sentence_keys", "inserted_sentence_keys"],
    ) {
        let Some((segment, label)) = map.get(&key) else {
            skip(&mut report, "missing_plan", 1);
            continue;
        };
        grouped
            .entry((key.0.clone(), key.1.clone(), key.2.clone()))
            .or_default()
            .push(LabelRestoration {
                sentence_id: key.3,
                expected_current_label: label["intended_label"].clone(),
                prior_state: label["prior_state"].as_str().unwrap_or_default().to_owned(),
                prior_label: (!label["prior_label"].is_null())
                    .then(|| label["prior_label"].clone()),
            });
        let _ = segment;
    }
    for ((day, stream, segment_key), restorations) in grouped {
        let directory = segment_path(journal_root, &day, &segment_key, &stream, false)?;
        if !directory.is_dir() {
            skip(&mut report, "missing", restorations.len());
            continue;
        }
        let delta = restore_label_rows(&directory, &restorations)?;
        increment(&mut report, "restored_count", delta.restored_count);
        increment(
            &mut report,
            "removed_inserted_count",
            delta.removed_inserted_count,
        );
        increment(
            &mut report,
            "patched_existing_count",
            delta.patched_existing_count,
        );
        skip(&mut report, "missing", delta.missing_count);
        skip(&mut report, "changed", delta.changed_count);
    }
    Ok(json!({"labels":report}))
}

/// Append identify-undo correction artifacts for corrections identify added.
pub fn undo_corrections(
    journal_root: &Path,
    state: &OperationState,
    undo_started_at: &str,
) -> Result<Value, UndoPhaseError> {
    let mut report = category(json!({"appended_count":0,"already_present_count":0}));
    let Some(checkpoint) = state.phase_checkpoints.get(&ForwardPhase::Corrections) else {
        return Ok(json!({"corrections":report}));
    };
    let corrections = planned_correction_rows(&state.prepared_plan);
    let labels = label_plan_map(&state.prepared_plan);
    for key in checkpoint_keys(checkpoint, &["appended_keys"]) {
        let (Some((segment, _)), Some((_, label))) = (corrections.get(&key), labels.get(&key))
        else {
            skip(&mut report, "missing_plan", 1);
            continue;
        };
        let directory = segment_path(journal_root, &key.0, &key.2, &key.1, false)?;
        let existing = read_corrections(&directory)?;
        if existing.iter().any(|row| {
            row["operation_id"].as_str() == Some(&state.operation_id)
                && row["correction_kind"].as_str() == Some("identify_undo")
                && row["sentence_id"].as_i64() == Some(key.3)
        }) {
            increment(&mut report, "already_present_count", 1);
            skip(&mut report, "already_present", 1);
            continue;
        }
        let prior_speaker = label["prior_label"]["speaker"].clone();
        let mut row = Map::new();
        row.insert("sentence_id".to_owned(), json!(key.3));
        row.insert("original_speaker".to_owned(), json!(state.target_entity_id));
        row.insert("corrected_speaker".to_owned(), prior_speaker);
        row.insert("original_method".to_owned(), json!("user_identified"));
        row.insert("timestamp".to_owned(), json!(undo_started_at));
        row.insert("operation_id".to_owned(), json!(state.operation_id));
        row.insert("undo_of_operation_id".to_owned(), json!(state.operation_id));
        row.insert("correction_kind".to_owned(), json!("identify_undo"));
        append_correction(&directory, row)?;
        increment(&mut report, "appended_count", 1);
        increment(&mut report, "restored_count", 1);
        let _ = segment;
    }
    Ok(json!({"corrections":report}))
}

/// Remove exactly the voiceprint rows this operation checkpointed as saved.
pub fn undo_voiceprints(
    journal_root: &Path,
    state: &OperationState,
    encoder: &EncoderIdentity,
) -> Result<Value, UndoPhaseError> {
    let mut report =
        category(json!({"removed_count":0,"missing_count":0,"metadata_mismatch_count":0}));
    let removals = voiceprint_removals_for_checkpoint(state);
    if removals.is_empty() {
        return Ok(json!({"voiceprints":report}));
    }
    let delta = remove_voiceprints_by_key(
        journal_root,
        state.target_entity_id.as_deref().unwrap_or_default(),
        &removals,
        encoder,
    )?;
    increment(&mut report, "removed_count", delta.removed_count);
    increment(&mut report, "restored_count", delta.removed_count);
    increment(&mut report, "missing_count", delta.skipped_reasons.missing);
    increment(
        &mut report,
        "metadata_mismatch_count",
        delta.skipped_reasons.metadata_mismatch,
    );
    skip(&mut report, "missing", delta.skipped_reasons.missing);
    skip(
        &mut report,
        "metadata_mismatch",
        delta.skipped_reasons.metadata_mismatch,
    );
    Ok(json!({"voiceprints":report}))
}

/// Compare-restore the candidate tracker state when retroactive confirmation matched.
pub fn undo_tracker(journal_root: &Path, state: &OperationState) -> Result<Value, UndoPhaseError> {
    let mut report = category(json!({"restored_candidate_count":0}));
    let Some(checkpoint) = state.phase_checkpoints.get(&ForwardPhase::RetroTracker) else {
        return Ok(json!({"tracker":report}));
    };
    let Some(candidate_id) = checkpoint["matched"]
        .as_bool()
        .filter(|matched| *matched)
        .and_then(|_| checkpoint["candidate_id"].as_i64())
    else {
        return Ok(json!({"tracker":report}));
    };
    let retro = &state.prepared_plan["retro_confirm"];
    let mut tracker = CandidateTracker::new(journal_root);
    let delta = tracker.restore_confirmed_candidate(
        candidate_id,
        &retro["candidate_after"],
        &retro["candidate_before"],
    )?;
    increment(
        &mut report,
        "restored_candidate_count",
        delta.restored_count,
    );
    increment(&mut report, "restored_count", delta.restored_count);
    skip(&mut report, "missing", delta.missing_count);
    skip(
        &mut report,
        "already_restored",
        delta.already_restored_count,
    );
    skip(
        &mut report,
        "concurrent_change",
        delta.concurrent_change_count,
    );
    Ok(json!({"tracker":report}))
}

/// Restore or remove the resolved-cluster sentinel only when it still equals this operation's value.
pub fn undo_sentinel(journal_root: &Path, state: &OperationState) -> Result<Value, UndoPhaseError> {
    let mut report = category(json!({"removed_count":0,"restored_prior_count":0}));
    let Some(checkpoint) = state.phase_checkpoints.get(&ForwardPhase::Sentinel) else {
        return Ok(json!({"sentinel":report}));
    };
    if checkpoint["written"].as_bool() != Some(true) {
        return Ok(json!({"sentinel":report}));
    }
    let sentinel = &state.prepared_plan["sentinel"];
    let Some(cluster_key) = sentinel["cluster_key"].as_str() else {
        skip(&mut report, "missing_plan", 1);
        return Ok(json!({"sentinel":report}));
    };
    let intended = &sentinel["intended_entry"];
    let prior = (!sentinel["prior_entry"].is_null()).then(|| sentinel["prior_entry"].clone());
    let mut values = load_resolved_clusters(journal_root);
    let current = values.get(cluster_key);
    if current == Some(intended) {
        if let Some(prior) = prior {
            values.insert(cluster_key.to_owned(), prior);
            increment(&mut report, "restored_prior_count", 1);
        } else {
            values.remove(cluster_key);
            increment(&mut report, "removed_count", 1);
        }
        increment(&mut report, "restored_count", 1);
        replace_resolved_clusters(journal_root, &values)?;
    } else if current == prior.as_ref() || (current.is_none() && prior.is_none()) {
        skip(&mut report, "already_restored", 1);
    } else {
        skip(&mut report, "concurrent_change", 1);
    }
    Ok(json!({"sentinel":report}))
}

/// Remove operation-owned keep-separate sources and a created entity when still unreferenced.
pub fn undo_entity(journal_root: &Path, state: &OperationState) -> Result<Value, UndoPhaseError> {
    let mut report = category(
        json!({"deleted":false,"blocked_categories":[],"keep_separate_sources_removed_count":0}),
    );
    if !state.will_create {
        return Ok(json!({"entity":report}));
    }
    let pair_keys = state
        .phase_checkpoints
        .get(&ForwardPhase::KeepSeparate)
        .and_then(|checkpoint| checkpoint["pair_keys"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let removed = remove_operation_sources(journal_root, &state.operation_id, &pair_keys)?;
    increment(&mut report, "keep_separate_sources_removed_count", removed);
    let Some(entity_checkpoint) = state.phase_checkpoints.get(&ForwardPhase::Entity) else {
        return Ok(json!({"entity":report}));
    };
    if entity_checkpoint["entity_created"].as_bool() != Some(true) {
        return Ok(json!({"entity":report}));
    }
    let references = entity_checkpoint["history_event_refs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some(EntityHistoryReference {
                version_id: value["version_id"].as_str()?.to_owned(),
                sequence: value["seq"].as_i64()? as i128,
            })
        })
        .collect::<Vec<_>>();
    let expected_identity = if entity_checkpoint["identity_after"].is_object() {
        &entity_checkpoint["identity_after"]
    } else {
        &state.prepared_plan["entity_identity"]["intended_identity"]
    };
    let result = delete_created_entity_if_unreferenced(
        journal_root,
        state.target_entity_id.as_deref().unwrap_or_default(),
        &state.operation_id,
        expected_identity,
        &references,
    )?;
    if result.deleted {
        report["deleted"] = Value::Bool(true);
        increment(&mut report, "restored_count", 1);
    }
    let blocked = blocked_categories(&result);
    if !result.deleted && !blocked.is_empty() {
        report["blocked_categories"] =
            Value::Array(blocked.iter().cloned().map(Value::String).collect());
        skip(&mut report, "blocked", 1);
    }
    Ok(json!({"entity":report}))
}

/// Build exact expected-metadata removals from direct and retro checkpoints.
#[must_use]
pub fn voiceprint_removals_for_checkpoint(state: &OperationState) -> Vec<VoiceprintRemoval> {
    let mut metadata = BTreeMap::<ValueKey, Value>::new();
    for entry in state.prepared_plan["direct_voiceprints"]["entries_to_add"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(key) = value_key(&entry["key"]) {
            metadata.insert(key, entry["metadata"].clone());
        }
    }
    for entry in state.prepared_plan["retro_confirm"]["voiceprints_to_add"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let key = if entry["key"].is_object() {
            &entry["key"]
        } else {
            &entry["metadata"]
        };
        if let Some(key) = value_key(key) {
            metadata.insert(key, entry["metadata"].clone());
        }
    }
    let mut seen = BTreeSet::new();
    let mut removals = Vec::new();
    for phase in [ForwardPhase::DirectVoiceprints, ForwardPhase::RetroTracker] {
        let Some(checkpoint) = state.phase_checkpoints.get(&phase) else {
            continue;
        };
        for key in checkpoint["saved_keys"].as_array().into_iter().flatten() {
            let Some(tuple) = value_key(key) else {
                continue;
            };
            if !seen.insert(tuple.clone()) {
                continue;
            }
            let Some(expected_metadata) = metadata.get(&tuple) else {
                continue;
            };
            removals.push(VoiceprintRemoval {
                key: key.clone(),
                expected_metadata: Some(expected_metadata.clone()),
            });
        }
    }
    removals
}

fn label_plan_map(plan: &Value) -> BTreeMap<PlanKey, (Value, Value)> {
    plan["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|segment| {
            segment["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |label| {
                    plan_key(segment, label["sentence_id"].as_i64()?)
                        .map(|key| (key, (segment.clone(), label.clone())))
                })
        })
        .collect()
}
fn planned_correction_rows(plan: &Value) -> BTreeMap<PlanKey, (Value, Value)> {
    plan["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|segment| {
            segment["corrections"]["rows_to_append"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |row| {
                    plan_key(segment, row["sentence_id"].as_i64()?)
                        .map(|key| (key, (segment.clone(), row.clone())))
                })
        })
        .collect()
}
fn plan_key(segment: &Value, sentence_id: i64) -> Option<PlanKey> {
    Some((
        segment["day"].as_str()?.into(),
        segment["stream"].as_str()?.into(),
        segment["segment_key"].as_str()?.into(),
        sentence_id,
    ))
}
fn checkpoint_keys(checkpoint: &Value, fields: &[&str]) -> Vec<PlanKey> {
    fields
        .iter()
        .flat_map(|field| checkpoint[*field].as_array().into_iter().flatten())
        .filter_map(|key| {
            Some((
                key["day"].as_str()?.into(),
                key["stream"].as_str()?.into(),
                key["segment_key"].as_str()?.into(),
                key["sentence_id"].as_i64()?,
            ))
        })
        .collect()
}
fn category(extra: Value) -> Value {
    let mut value = json!({"restored_count":0,"skipped_count":0,"skipped_reasons":{}});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    value
}
fn increment(report: &mut Value, field: &str, count: usize) {
    let current = report[field].as_u64().unwrap_or(0);
    report[field] = json!(current + count as u64);
}
fn skip(report: &mut Value, reason: &str, count: usize) {
    if count == 0 {
        return;
    }
    increment(report, "skipped_count", count);
    let reasons = report["skipped_reasons"]
        .as_object_mut()
        .expect("undo category has reasons");
    let current = reasons.get(reason).and_then(Value::as_u64).unwrap_or(0);
    reasons.insert(reason.to_owned(), json!(current + count as u64));
}
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ValueKey(String, String, String, i64);
fn value_key(value: &Value) -> Option<ValueKey> {
    Some(ValueKey(
        value["day"].as_str()?.into(),
        value["segment_key"].as_str()?.into(),
        value["source"].as_str()?.into(),
        value["sentence_id"].as_i64()?,
    ))
}
fn blocked_categories(outcome: &solstone_core_facets::EntityDeleteGuardOutcome) -> Vec<String> {
    let refs = &outcome.references;
    let mut values = Vec::new();
    if outcome.identity_changed {
        values.push("identity_changed".into())
    }
    if outcome.history_changed {
        values.push("history_changed".into())
    }
    for (name, count) in [
        ("unrecognized_file", refs.unrecognized_file),
        ("facet_relationship", refs.facet_relationship),
        ("observation", refs.observation),
        ("activity", refs.activity),
        ("segment_label", refs.segment_label),
        ("segment_correction", refs.segment_correction),
        ("aka_crossref", refs.aka_crossref),
        ("speaker_candidate", refs.speaker_candidate),
        ("keep_separate", refs.keep_separate),
        ("identify_operation", refs.identify_operation),
        ("ambiguity", refs.ambiguity),
        ("entity_review_candidate", refs.entity_review_candidate),
        ("speaker_review_candidate", refs.speaker_review_candidate),
        ("candidate_pair", refs.candidate_pair),
        ("dismissal", refs.dismissal),
        ("unreadable", refs.unreadable),
    ] {
        if count > 0 {
            values.push(name.into())
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::segment_path;
    use solstone_core_entity::{
        VoiceprintItem, load_entity_voiceprints_file, save_voiceprints_batch,
    };

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const DAY: &str = "20260808";
    const STREAM: &str = "mic";
    const SEGMENT: &str = "120000_300";

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-identify-undo-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn encoder() -> EncoderIdentity {
        EncoderIdentity {
            id: "test".into(),
            sha256: "a".repeat(64),
            width: 256,
        }
    }
    fn embedding() -> Vec<f32> {
        let mut row = vec![0.0; 256];
        row[0] = 1.0;
        row
    }
    fn metadata(added_at: i64) -> Value {
        json!({"day":DAY,"segment_key":SEGMENT,"source":"audio","stream":STREAM,"sentence_id":7,"added_at":added_at,"last_seen_ts":added_at})
    }
    fn key() -> Value {
        json!({"day":DAY,"segment_key":SEGMENT,"source":"audio","sentence_id":7})
    }
    fn segment(root: &Path) -> PathBuf {
        let path = segment_path(root, DAY, SEGMENT, STREAM, true).unwrap();
        fs::create_dir_all(path.join("talents")).unwrap();
        path
    }
    fn entity(root: &Path, id: &str) {
        let path = root.join("entities").join(id);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("entity.json"),
            json!({"id":id,"name":id,"type":"Person"}).to_string(),
        )
        .unwrap();
    }
    fn state(root: &Path) -> OperationState {
        let _ = root;
        OperationState {
            operation_id: "idop_test".into(),
            request_id: "request".into(),
            request_fingerprint: "a".repeat(64),
            cluster_member_set: BTreeSet::new(),
            target_entity_id: Some("target".into()),
            target_entity_name: Some("Target".into()),
            will_create: false,
            entity_type: Some("Person".into()),
            reviewed_near_match_entity_ids: vec![],
            completed_phases: vec![],
            pending_phases: vec![],
            terminal_status: crate::identify_operations::TerminalStatus::InProgress,
            result: None,
            undo_report: None,
            undo_started_at: None,
            undo_committed_count: 0,
            phase_checkpoints: BTreeMap::new(),
            prepared_plan: json!({"segments":[{"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"labels":[{"sentence_id":7,"prior_state":"absent","prior_label":null,"intended_label":{"sentence_id":7,"speaker":"target","confidence":"high","method":"user_identified"}}],"corrections":{"rows_to_append":[{"sentence_id":7}]}}],"direct_voiceprints":{"entries_to_add":[{"key":key(),"metadata":metadata(1)}]},"retro_confirm":{"voiceprints_to_add":[]},"sentinel":{"cluster_key":"1","prior_entry":null,"intended_entry":{"entity_id":"target"}},"entity_identity":{"intended_identity":{"id":"target","name":"Target","type":"Person"}}}),
            repair_required: None,
            undo_repair_required: None,
            undo_phase_checkpoints: BTreeMap::new(),
        }
    }
    fn sentence_key() -> Value {
        json!({"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"sentence_id":7})
    }

    #[test]
    fn undo_labels_restores_insert_and_skips_already_removed_row() {
        let temporary = Temp::new();
        let directory = segment(temporary.path());
        fs::write(directory.join("talents/speaker_labels.json"),json!({"labels":[{"sentence_id":7,"speaker":"target","confidence":"high","method":"user_identified"}]}).to_string()).unwrap();
        let mut state = state(temporary.path());
        state.phase_checkpoints.insert(
            ForwardPhase::Labels,
            json!({"inserted_sentence_keys":[sentence_key()],"patched_sentence_keys":[]}),
        );
        assert_eq!(
            undo_labels(temporary.path(), &state).unwrap()["labels"]["removed_inserted_count"],
            1
        );
        assert_eq!(
            undo_labels(temporary.path(), &state).unwrap()["labels"]["skipped_reasons"]["missing"],
            1
        );
    }

    #[test]
    fn undo_corrections_appends_once_then_detects_existing_undo_artifact() {
        let temporary = Temp::new();
        segment(temporary.path());
        let mut state = state(temporary.path());
        state.phase_checkpoints.insert(
            ForwardPhase::Corrections,
            json!({"appended_keys":[sentence_key()]}),
        );
        assert_eq!(
            undo_corrections(temporary.path(), &state, "now").unwrap()["corrections"]["appended_count"],
            1
        );
        assert_eq!(
            undo_corrections(temporary.path(), &state, "now").unwrap()["corrections"]["already_present_count"],
            1
        );
    }

    #[test]
    fn ac8_undo_voiceprints_removes_only_checkpointed_exact_metadata() {
        let temporary = Temp::new();
        entity(temporary.path(), "target");
        let mut state = state(temporary.path());
        save_voiceprints_batch(
            temporary.path(),
            "target",
            &[
                VoiceprintItem {
                    embedding: embedding(),
                    metadata: metadata(1),
                },
                VoiceprintItem {
                    embedding: embedding(),
                    metadata: metadata(2),
                },
            ],
            &encoder(),
        )
        .unwrap();
        state.phase_checkpoints.insert(
            ForwardPhase::DirectVoiceprints,
            json!({"saved_keys":[key()]}),
        );
        let report = undo_voiceprints(temporary.path(), &state, &encoder()).unwrap();
        assert_eq!(report["voiceprints"]["removed_count"], 1);
        let archive = load_entity_voiceprints_file(temporary.path(), "target").unwrap();
        assert_eq!(archive.rows, 1);
        assert_eq!(archive.metadata, vec![metadata(2).to_string()]);
        assert_eq!(
            undo_voiceprints(temporary.path(), &state, &encoder()).unwrap()["voiceprints"]["metadata_mismatch_count"],
            1
        );
    }

    #[test]
    fn undo_tracker_restores_confirmed_snapshot_then_skips_already_restored() {
        let temporary = Temp::new();
        let mut tracker = CandidateTracker::new(temporary.path());
        tracker.process_segment(&[crate::candidate_tracker::ClusterInput{source_segment:json!({"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"source":"audio","cluster_label":1}),embeddings:vec![embedding()],durations_s:vec![1.0]}]).unwrap();
        let before = tracker.snapshot_candidates_locked().unwrap().remove(0);
        tracker.mark_confirmed(before.cand_id, "target").unwrap();
        let after = tracker.snapshot_candidates_locked().unwrap().remove(0);
        let mut state = state(temporary.path());
        state.phase_checkpoints.insert(
            ForwardPhase::RetroTracker,
            json!({"matched":true,"candidate_id":before.cand_id}),
        );
        state.prepared_plan["retro_confirm"] =
            json!({"candidate_before":before.to_json(),"candidate_after":after.to_json()});
        assert_eq!(
            undo_tracker(temporary.path(), &state).unwrap()["tracker"]["restored_candidate_count"],
            1
        );
        assert_eq!(
            undo_tracker(temporary.path(), &state).unwrap()["tracker"]["skipped_reasons"]["already_restored"],
            1
        );
    }

    #[test]
    fn undo_sentinel_restores_prior_or_skips_concurrent_drift() {
        let temporary = Temp::new();
        let mut state = state(temporary.path());
        state
            .phase_checkpoints
            .insert(ForwardPhase::Sentinel, json!({"written":true}));
        let mut values = BTreeMap::new();
        values.insert("1".into(), json!({"entity_id":"target"}));
        replace_resolved_clusters(temporary.path(), &values).unwrap();
        assert_eq!(
            undo_sentinel(temporary.path(), &state).unwrap()["sentinel"]["removed_count"],
            1
        );
        values.insert("1".into(), json!({"entity_id":"other"}));
        replace_resolved_clusters(temporary.path(), &values).unwrap();
        assert_eq!(
            undo_sentinel(temporary.path(), &state).unwrap()["sentinel"]["skipped_reasons"]["concurrent_change"],
            1
        );
    }

    #[test]
    fn undo_entity_is_a_noop_for_non_created_targets() {
        let temporary = Temp::new();
        let state = state(temporary.path());
        assert_eq!(
            undo_entity(temporary.path(), &state).unwrap()["entity"]["deleted"],
            false
        );
    }

    #[test]
    fn undo_entity_tombstones_only_its_keep_separate_sources() {
        let temporary = Temp::new();
        let mut state = state(temporary.path());
        state.will_create = true;
        crate::keep_separate::record_keep_separate_assertion(
            temporary.path(),
            "target",
            "near",
            "explicit_create_near_match",
            Some(&state.operation_id),
            1,
        )
        .unwrap();
        state.phase_checkpoints.insert(
            ForwardPhase::KeepSeparate,
            json!({"pair_keys":["near|target"]}),
        );
        let report = undo_entity(temporary.path(), &state).unwrap();
        assert_eq!(report["entity"]["keep_separate_sources_removed_count"], 1);
        assert!(
            crate::keep_separate::find_assertion(temporary.path(), "target", "near")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn undo_entity_reports_unreadable_reference_safety_block() {
        let temporary = Temp::new();
        let mut state = state(temporary.path());
        state.will_create = true;
        let entity_plan = crate::identify_forward_phases::EntityPhasePlan {
            target_entity_id: "target".into(),
            will_create: true,
            intended_identity: json!({"id":"target","name":"Target","type":"Person"}),
            operation_id: state.operation_id.clone(),
        };
        let checkpoint =
            crate::identify_forward_phases::phase_entity(temporary.path(), &entity_plan)
                .unwrap()
                .fields;
        state
            .phase_checkpoints
            .insert(ForwardPhase::Entity, checkpoint);
        let report = undo_entity(temporary.path(), &state).unwrap();
        assert_eq!(report["entity"]["deleted"], false);
        assert_eq!(
            report["entity"]["blocked_categories"],
            json!(["unreadable"])
        );
    }
}
