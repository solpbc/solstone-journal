// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backfill selection and speaker-label preservation primitives.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::bootstrap::scan_segments;
use crate::resolve::{resolve, ResolveError, ResolveOutcome};

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

#[derive(Debug, Error)]
pub enum BackfillError {
    #[error("backfill scan failed: {0}")]
    Scan(#[from] crate::bootstrap::BootstrapError),
    #[error("native attribution failed: {0}")]
    Resolve(#[from] ResolveError),
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
