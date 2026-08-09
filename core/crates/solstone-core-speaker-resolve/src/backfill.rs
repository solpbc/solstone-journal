// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backfill selection and speaker-label preservation primitives.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_speaker_id::labels::write_full_labels;
use thiserror::Error;

use crate::backfill_operations::{
    BACKFILL_OPERATION_SCHEMA_VERSION, BackfillCheckpointOutcome, BackfillOperationEvent,
    BackfillOperationPayload, BackfillOperationState, BackfillOperationTerminalStatus,
    BackfillSegmentKey, append_backfill_event, backfill_operations_path, fold_backfill_operation,
    load_backfill_operations,
};
use crate::bootstrap::scan_segments;
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

/// Execute or resume one durable backfill operation without rewriting its snapshot.
pub fn run_backfill(request: &BackfillRunRequest) -> Result<BackfillRunResult, BackfillError> {
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
            path: solstone_core_journal_io::segment_path(
                &request.journal_root,
                &key.day,
                &key.segment_key,
                &key.stream,
                false,
            )?,
        };
        let outcome =
            match resolve_backfill_segment(&request.journal_root, &segment, request.now_ms) {
                Ok(ResolveOutcome::Resolved(output)) => {
                    let labels = output.labels.iter().map(label_json).collect::<Vec<_>>();
                    write_full_labels(&segment.path, labels, &Map::new())?;
                    BackfillCheckpointOutcome::Processed
                }
                Ok(_) => BackfillCheckpointOutcome::Skipped,
                Err(_) => BackfillCheckpointOutcome::Error,
            };
        append_backfill_event(
            &ledger_path,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: format!(
                    "{}:checkpoint:{}:{}:{}",
                    request.operation_id, key.day, key.stream, key.segment_key
                ),
                operation_id: request.operation_id.clone(),
                ts: Utc::now().to_rfc3339(),
                payload: BackfillOperationPayload::Checkpoint {
                    segment: key.clone(),
                    outcome,
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
    value
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
        pending_count: state.pending_segments.len(),
        done: state.terminal_status == BackfillOperationTerminalStatus::Done,
    }
}

/// Preserve every `user_`-method label at its sentence ID while replacing other rows.
#[must_use]
pub fn merge_user_labels(current: Option<&Value>, fresh_labels: &[Value]) -> Vec<Value> {
    let mut user_by_sentence = BTreeMap::<i64, Value>::new();
    if let Some(labels) = current
        .and_then(|payload| payload.get("labels"))
        .and_then(Value::as_array)
    {
        for label in labels {
            if is_user_label(label) {
                if let Some(sentence_id) = label_sentence_id(label) {
                    user_by_sentence.insert(sentence_id, label.clone());
                }
            }
        }
    }
    let mut fresh_sentence_ids = HashSet::<i64>::new();
    let mut merged = Vec::with_capacity(fresh_labels.len() + user_by_sentence.len());
    for label in fresh_labels {
        let Some(sentence_id) = label_sentence_id(label) else {
            merged.push(label.clone());
            continue;
        };
        fresh_sentence_ids.insert(sentence_id);
        merged.push(
            user_by_sentence
                .get(&sentence_id)
                .cloned()
                .unwrap_or_else(|| label.clone()),
        );
    }
    merged.extend(
        user_by_sentence
            .into_iter()
            .filter(|(sentence_id, _)| !fresh_sentence_ids.contains(sentence_id))
            .map(|(_, label)| label),
    );
    merged
}

fn has_audio_embeddings(sources: &[String]) -> bool {
    sources
        .iter()
        .any(|source| source == "audio" || source.ends_with("_audio"))
}

fn is_user_label(label: &Value) -> bool {
    label
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| method.starts_with("user_"))
}

fn label_sentence_id(label: &Value) -> Option<i64> {
    match label.get("sentence_id")? {
        Value::Number(number) => number.as_i64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}
