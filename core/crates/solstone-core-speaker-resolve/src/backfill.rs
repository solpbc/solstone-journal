// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backfill selection and speaker-label preservation primitives.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_entity::hold_entity_trust_lock;
use solstone_core_speaker_id::labels::write_full_labels;
use thiserror::Error;

use crate::backfill_operations::{
    BACKFILL_OPERATION_SCHEMA_VERSION, BackfillCheckpointOutcome, BackfillOperationEvent,
    BackfillOperationPayload, BackfillOperationState, BackfillOperationTerminalStatus,
    BackfillSegmentError, BackfillSegmentKey, append_backfill_event, backfill_operations_path,
    fold_backfill_operation, load_backfill_operations,
};
use crate::bootstrap::scan_segments;
use crate::owner_admission::OWNER_IDENTITY_INVALID_REASON;
use crate::resolve::{ResolveError, ResolveOutcome, resolve};

/// Classification used by default backfill selection for an existing label payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerLabelsState {
    Stubbed,
    Labelled,
}

/// One chronologically selected segment for a later backfill execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillSegment {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub path: PathBuf,
}

/// Results of backfill's enumerate-and-filter phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillPlan {
    pub total_segments: usize,
    pub total_eligible: usize,
    pub already_labeled: usize,
    pub skipped_no_embed: usize,
    pub to_process: Vec<BackfillSegment>,
}

/// One bounded-JSON CLI request for a resumable native backfill run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillRunRequest {
    pub journal_root: PathBuf,
    pub operation_id: String,
    pub reattribute: bool,
    pub now_ms: i64,
}

/// Progress and terminal state returned after one in-process backfill invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillRunResult {
    pub operation_id: String,
    pub total_count: usize,
    pub processed_count: usize,
    pub skipped_count: usize,
    pub error_count: usize,
    pub error_segments: Vec<BackfillSegmentError>,
    pub pending_count: usize,
    pub done: bool,
}

#[derive(Debug, Error)]
pub enum BackfillError {
    #[error("backfill scan failed: {0}")]
    Scan(#[from] crate::bootstrap::BootstrapError),
    #[error("native attribution failed: {0}")]
    Resolve(#[from] ResolveError),
    #[error("backfill operation ledger failed: {0}")]
    Ledger(#[from] crate::backfill_operations::BackfillOperationError),
    #[error("speaker label write failed: {0}")]
    Labels(#[from] solstone_core_speaker_id::labels::LabelsError),
    #[error("segment path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    #[error("backfill operation lock failed: {0}")]
    Trust(#[from] solstone_core_entity::EntityTrustLockError),
}

/// Distinguish an explicit locked stub from every other label payload.
#[must_use]
pub fn classify_speaker_labels_payload(payload: &Value) -> SpeakerLabelsState {
    let stubbed = payload.as_object().is_some_and(|object| {
        object
            .get("labels")
            .is_some_and(|labels| labels.as_array().is_some_and(|labels| labels.is_empty()))
            && object.get("skipped") == Some(&Value::Bool(true))
    });
    if stubbed {
        SpeakerLabelsState::Stubbed
    } else {
        SpeakerLabelsState::Labelled
    }
}

/// Classify a durable label file; malformed content remains conservatively labelled.
#[must_use]
pub fn classify_speaker_labels_text(payload: &str) -> SpeakerLabelsState {
    serde_json::from_str(payload)
        .map(|value| classify_speaker_labels_payload(&value))
        .unwrap_or(SpeakerLabelsState::Labelled)
}

/// Enumerate audio-bearing segments and apply the corrected default skip policy.
pub fn plan_backfill_segments(
    journal_root: &Path,
    reattribute: bool,
) -> Result<BackfillPlan, BackfillError> {
    let mut plan = BackfillPlan {
        total_segments: 0,
        total_eligible: 0,
        already_labeled: 0,
        skipped_no_embed: 0,
        to_process: Vec::new(),
    };
    for segment in scan_segments(journal_root)? {
        plan.total_segments += 1;
        if !has_audio_embeddings(&segment.sources) {
            plan.skipped_no_embed += 1;
            continue;
        }
        plan.total_eligible += 1;
        let labels_path = segment.path.join("talents/speaker_labels.json");
        let existing_state = labels_path.exists().then(|| {
            std::fs::read_to_string(&labels_path)
                .map(|payload| classify_speaker_labels_text(&payload))
                .unwrap_or(SpeakerLabelsState::Labelled)
        });
        if !reattribute && existing_state == Some(SpeakerLabelsState::Labelled) {
            plan.already_labeled += 1;
            continue;
        }
        plan.to_process.push(BackfillSegment {
            day: segment.day,
            stream: segment.stream,
            segment_key: segment.key,
            path: segment.path,
        });
    }
    Ok(plan)
}

/// Forward one selected segment to the existing native Layers 1–3 resolver.
pub fn resolve_backfill_segment(
    journal_root: &Path,
    segment: &BackfillSegment,
    now_ms: i64,
) -> Result<ResolveOutcome, BackfillError> {
    Ok(resolve(
        journal_root,
        &segment.day,
        &segment.stream,
        &segment.segment_key,
        false,
        now_ms,
    )?)
}

/// Classify a native resolution outcome for backfill checkpointing.
#[must_use]
pub fn classify_backfill_outcome(
    outcome: &ResolveOutcome,
) -> (BackfillCheckpointOutcome, Option<String>) {
    match outcome {
        ResolveOutcome::Resolved(_) => (BackfillCheckpointOutcome::Processed, None),
        ResolveOutcome::IdentityInvalid => (
            BackfillCheckpointOutcome::Error,
            Some(OWNER_IDENTITY_INVALID_REASON.to_owned()),
        ),
        ResolveOutcome::NoOwnerCentroid
        | ResolveOutcome::SegmentMissing
        | ResolveOutcome::Empty { .. } => (BackfillCheckpointOutcome::Skipped, None),
    }
}

/// Execute or resume one durable backfill operation without rewriting its snapshot.
pub fn run_backfill(request: &BackfillRunRequest) -> Result<BackfillRunResult, BackfillError> {
    let _trust = hold_entity_trust_lock(&request.journal_root)?;
    let ledger_path = backfill_operations_path(&request.journal_root);
    let mut state = fold_backfill_operation(
        &load_backfill_operations(&ledger_path)?,
        &request.operation_id,
    )?;
    if state.is_none() {
        let plan = plan_backfill_segments(&request.journal_root, request.reattribute)?;
        let segments = plan
            .to_process
            .iter()
            .map(|segment| BackfillSegmentKey {
                day: segment.day.clone(),
                stream: segment.stream.clone(),
                segment_key: segment.segment_key.clone(),
            })
            .collect::<Vec<_>>();
        let now = Utc::now().to_rfc3339();
        append_backfill_event(
            &ledger_path,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{}:prepared", request.operation_id),
                operation_id: request.operation_id.clone(),
                ts: now.clone(),
                payload: BackfillOperationPayload::Prepared {
                    started_at: now,
                    reattribute: request.reattribute,
                    total_count: segments.len(),
                    segments,
                },
            },
        )?;
        state = fold_backfill_operation(
            &load_backfill_operations(&ledger_path)?,
            &request.operation_id,
        )?;
    }
    let state = state.expect("prepared backfill operation folds");
    if state.terminal_status == BackfillOperationTerminalStatus::Done {
        return Ok(backfill_result(&state));
    }
    for key in &state.pending_segments {
        let segment = BackfillSegment {
            day: key.day.clone(),
            stream: key.stream.clone(),
            segment_key: key.segment_key.clone(),
            path: crate::segment_path(
                &request.journal_root,
                &key.day,
                &key.segment_key,
                &key.stream,
                false,
            )?,
        };
        let (outcome, error_detail) =
            match resolve_backfill_segment(&request.journal_root, &segment, request.now_ms) {
                Ok(resolved) => {
                    let classification = classify_backfill_outcome(&resolved);
                    if let ResolveOutcome::Resolved(output) = resolved {
                        write_resolved_backfill_labels(&segment.path, &output)?;
                    }
                    classification
                }
                Err(error) => (BackfillCheckpointOutcome::Error, Some(error.to_string())),
            };
        append_backfill_event(
            &ledger_path,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: next_checkpoint_event_id(&ledger_path, &request.operation_id, key)?,
                operation_id: request.operation_id.clone(),
                ts: Utc::now().to_rfc3339(),
                payload: BackfillOperationPayload::Checkpoint {
                    segment: key.clone(),
                    outcome,
                    error_detail,
                },
            },
        )?;
    }
    let state = fold_backfill_operation(
        &load_backfill_operations(&ledger_path)?,
        &request.operation_id,
    )?
    .expect("prepared backfill operation remains present");
    if state.pending_segments.is_empty() {
        append_backfill_event(
            &ledger_path,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!("{}:completed", request.operation_id),
                operation_id: request.operation_id.clone(),
                ts: Utc::now().to_rfc3339(),
                payload: BackfillOperationPayload::Completed {
                    completed_at: Utc::now().to_rfc3339(),
                },
            },
        )?;
    }
    let state = fold_backfill_operation(
        &load_backfill_operations(&ledger_path)?,
        &request.operation_id,
    )?
    .expect("prepared backfill operation remains present");
    Ok(backfill_result(&state))
}

fn write_resolved_backfill_labels(
    segment: &Path,
    output: &crate::resolve::ResolveOutput,
) -> Result<(), solstone_core_speaker_id::labels::LabelsError> {
    let labels = output.labels.iter().map(label_json).collect::<Vec<_>>();
    write_full_labels(segment, labels, &metadata_json(&output.metadata))
}

fn label_json(label: &crate::layer1::Label) -> Value {
    let mut value = serde_json::json!({"sentence_id":label.sentence_id});
    let object = value.as_object_mut().expect("label is an object");
    if let Some(speaker) = &label.speaker {
        object.insert("speaker".to_owned(), Value::String(speaker.clone()));
    }
    if let Some(confidence) = &label.confidence {
        object.insert("confidence".to_owned(), Value::String(confidence.clone()));
    }
    if let Some(method) = &label.method {
        object.insert("method".to_owned(), Value::String(method.clone()));
    }
    if let Some(owner_margin_declined) = label.owner_margin_declined {
        object.insert(
            "owner_margin_declined".to_owned(),
            Value::Bool(owner_margin_declined),
        );
    }
    if let Some(acoustic_margin_declined) = label.acoustic_margin_declined {
        object.insert(
            "acoustic_margin_declined".to_owned(),
            Value::Bool(acoustic_margin_declined),
        );
    }
    value
}

fn metadata_json(metadata: &crate::resolve::ResolveMetadata) -> Map<String, Value> {
    let mut value = Map::new();
    value.insert(
        "owner_centroid_last_refreshed_at".to_owned(),
        metadata
            .owner_centroid_last_refreshed_at
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    value.insert(
        "voiceprint_versions".to_owned(),
        Value::Object(
            metadata
                .voiceprint_versions
                .iter()
                .map(|(entity_id, version)| (entity_id.clone(), Value::from(*version)))
                .collect(),
        ),
    );
    value.insert(
        "candidate_evidence".to_owned(),
        Value::Array(
            metadata
                .candidate_evidence
                .iter()
                .map(|evidence| {
                    serde_json::json!({"entity_id":evidence.entity_id,"sources":evidence.sources})
                })
                .collect(),
        ),
    );
    if let Some(gaps) = &metadata.candidate_evidence_gaps {
        value.insert(
            "candidate_evidence_gaps".to_owned(),
            Value::Array(
                gaps.iter()
                    .map(|gap| serde_json::json!({"source":gap.source,"reason":gap.reason}))
                    .collect(),
            ),
        );
    }
    value
}

fn next_checkpoint_event_id(
    ledger_path: &Path,
    operation_id: &str,
    key: &BackfillSegmentKey,
) -> Result<String, BackfillError> {
    let attempts = load_backfill_operations(ledger_path)?
        .iter()
        .filter(|row| {
            row.event.operation_id == operation_id
                && matches!(
                    &row.event.payload,
                    BackfillOperationPayload::Checkpoint { segment, .. } if segment == key
                )
        })
        .count();
    let base = format!(
        "{operation_id}:checkpoint:{}:{}:{}",
        key.day, key.stream, key.segment_key
    );
    Ok(if attempts == 0 {
        base
    } else {
        format!("{base}:retry:{attempts}")
    })
}

fn backfill_result(state: &BackfillOperationState) -> BackfillRunResult {
    let mut processed_count = 0;
    let mut skipped_count = 0;
    let mut error_count = 0;
    for outcome in state.checkpointed_segments.values() {
        match outcome {
            BackfillCheckpointOutcome::Processed => processed_count += 1,
            BackfillCheckpointOutcome::Skipped => skipped_count += 1,
            BackfillCheckpointOutcome::Error => error_count += 1,
        }
    }
    BackfillRunResult {
        operation_id: state.operation_id.clone(),
        total_count: state.total_segments,
        processed_count,
        skipped_count,
        error_count,
        error_segments: state
            .error_details
            .iter()
            .map(|(segment, detail)| BackfillSegmentError {
                segment: segment.clone(),
                detail: detail.clone(),
            })
            .collect(),
        pending_count: state.pending_segments.len(),
        done: state.terminal_status == BackfillOperationTerminalStatus::Done,
    }
}

fn has_audio_embeddings(sources: &[String]) -> bool {
    sources
        .iter()
        .any(|source| source == "audio" || source.ends_with("_audio"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::evidence::{CandidateEvidence, EvidenceGap};
    use crate::layer1::Label;
    use crate::resolve::{ResolveMetadata, ResolveOutput};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn ac2_backfill_write_preserves_user_prefix_and_persists_full_resolve_output() {
        let root = std::env::temp_dir().join(format!(
            "solstone-backfill-label-write-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let segment = root.join("segment");
        fs::create_dir_all(segment.join("talents")).unwrap();
        let preserved = serde_json::json!({
            "sentence_id": 1,
            "speaker": "person-user",
            "method": "user_zzz_test",
            "opaque": {"preserve": [1, 2, 3]},
        });
        fs::write(
            segment.join("talents/speaker_labels.json"),
            serde_json::json!({"labels":[
                preserved.clone(),
                {"sentence_id":2,"speaker":"stale","method":"cluster"}
            ]})
            .to_string(),
        )
        .unwrap();
        let output = ResolveOutput {
            labels: vec![
                Label {
                    sentence_id: 1,
                    speaker: Some("replacement".to_owned()),
                    confidence: Some("high".to_owned()),
                    method: Some("acoustic".to_owned()),
                    owner_margin_declined: None,
                    acoustic_margin_declined: None,
                },
                Label {
                    sentence_id: 2,
                    speaker: Some("fresh".to_owned()),
                    confidence: Some("low".to_owned()),
                    method: Some("cluster".to_owned()),
                    owner_margin_declined: Some(true),
                    acoustic_margin_declined: Some(true),
                },
            ],
            unmatched: vec![],
            unmatched_texts: HashMap::new(),
            source: Some("audio".to_owned()),
            candidates: vec![],
            metadata: ResolveMetadata {
                owner_centroid_last_refreshed_at: Some("2026-08-08T00:00:00Z".to_owned()),
                voiceprint_versions: HashMap::from([("fresh".to_owned(), 3)]),
                candidate_evidence: vec![CandidateEvidence {
                    entity_id: "fresh".to_owned(),
                    sources: vec!["screen".to_owned()],
                }],
                candidate_evidence_gaps: Some(vec![EvidenceGap {
                    source: "meeting".to_owned(),
                    reason: "missing".to_owned(),
                }]),
                voiceprint_gaps: None,
            },
        };

        write_resolved_backfill_labels(&segment, &output).unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(segment.join("talents/speaker_labels.json")).unwrap())
                .unwrap();
        assert_eq!(saved["labels"][0], preserved);
        assert_eq!(saved["labels"][1]["speaker"], "fresh");
        assert_eq!(saved["labels"][1]["owner_margin_declined"], true);
        assert_eq!(saved["labels"][1]["acoustic_margin_declined"], true);
        assert_eq!(
            saved["owner_centroid_last_refreshed_at"],
            "2026-08-08T00:00:00Z"
        );
        assert_eq!(saved["voiceprint_versions"], serde_json::json!({"fresh":3}));
        assert_eq!(
            saved["candidate_evidence"],
            serde_json::json!([{"entity_id":"fresh","sources":["screen"]}])
        );
        assert_eq!(
            saved["candidate_evidence_gaps"],
            serde_json::json!([{"source":"meeting","reason":"missing"}])
        );
        fs::remove_dir_all(root).unwrap();
    }
}
