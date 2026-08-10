// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Serialize;
use serde_json::Value;

use crate::JournalRoot;
use crate::speakers_calendar::{
    audio_embedding_sources, day_dirs, iter_segments, journal_principal_id,
};
use crate::speakers_npz::{load_voiceprints, owner_centroid_summary};

const QUALITY_WINDOW_DAYS: usize = 30;

pub async fn quality(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    Json(quality_status(&root.0)).into_response()
}

fn quality_status(root: &Path) -> QualityStatus {
    let mut window_days = day_dirs(root);
    window_days.sort_by(|left, right| right.cmp(left));
    window_days.truncate(QUALITY_WINDOW_DAYS);
    let mut counters = Counters::default();
    for day in &window_days {
        for segment in iter_segments(root, day) {
            if audio_embedding_sources(&segment.path).is_empty() {
                continue;
            }
            count_segment_quality(&segment.path, &mut counters);
        }
    }
    QualityStatus {
        quality_window_days: QUALITY_WINDOW_DAYS,
        quality_window_count: window_days.len(),
        quality_window_error_count: counters.unreadable_files.total_window_count,
        tier_histogram: counters.tier_histogram,
        demotions_by_class: counters.demotions_by_class,
        corrections_window_count: counters.corrections_window_count,
        unreadable_files: counters.unreadable_files,
        empty_labels_without_skipped_segments: counters.empty_labels_without_skipped_segments,
        owner_voice: owner_voice_state(root),
    }
}

#[derive(Default)]
struct Counters {
    tier_histogram: TierHistogram,
    demotions_by_class: DemotionsByClass,
    corrections_window_count: usize,
    unreadable_files: UnreadableFiles,
    empty_labels_without_skipped_segments: usize,
}

#[derive(Default, Serialize)]
struct TierHistogram {
    high_statements: usize,
    medium_statements: usize,
    margin_declined_statements: usize,
    unlabeled_sentence_statements: usize,
    skipped_stub_segments: usize,
    no_labels_file_segments: usize,
}

#[derive(Default, Serialize)]
struct DemotionsByClass {
    owner_margin_declined: DemotionCounts,
    acoustic_margin_declined: DemotionCounts,
}

#[derive(Default, Serialize)]
struct DemotionCounts {
    high_statements: usize,
    medium_statements: usize,
    none_statements: usize,
    total_statements: usize,
}

#[derive(Default, Serialize)]
struct UnreadableFiles {
    speaker_labels_window_count: usize,
    speaker_corrections_window_count: usize,
    total_window_count: usize,
}

#[derive(Serialize)]
struct QualityStatus {
    quality_window_days: usize,
    quality_window_count: usize,
    quality_window_error_count: usize,
    tier_histogram: TierHistogram,
    demotions_by_class: DemotionsByClass,
    corrections_window_count: usize,
    unreadable_files: UnreadableFiles,
    empty_labels_without_skipped_segments: usize,
    owner_voice: OwnerVoice,
}

#[derive(Serialize)]
struct OwnerVoice {
    bootstrap_state: &'static str,
    status: String,
    centroid_saved: bool,
    evidence_tier: Option<String>,
    evidence_count: i32,
    built_at: Option<String>,
    refreshed_at: Option<String>,
}

fn count_segment_quality(segment: &Path, counters: &mut Counters) {
    count_label_file(&segment.join("talents/speaker_labels.json"), counters);
    count_corrections_file(&segment.join("talents/speaker_corrections.json"), counters);
}

fn count_label_file(path: &Path, counters: &mut Counters) {
    if !path.exists() {
        counters.tier_histogram.no_labels_file_segments += 1;
        return;
    }
    let Some(payload) = read_json_object(path) else {
        count_unreadable(counters, UnreadableKind::Labels);
        return;
    };
    let Some(labels) = payload.get("labels").and_then(Value::as_array) else {
        count_unreadable(counters, UnreadableKind::Labels);
        return;
    };
    if !labels.iter().all(label_is_classifiable) {
        count_unreadable(counters, UnreadableKind::Labels);
        return;
    }
    if payload.get("skipped") == Some(&Value::Bool(true)) {
        counters.tier_histogram.skipped_stub_segments += 1;
    } else if labels.is_empty() {
        counters.empty_labels_without_skipped_segments += 1;
    }
    for label in labels {
        count_label(label, counters);
    }
}

fn label_is_classifiable(label: &Value) -> bool {
    label
        .as_object()
        .is_some_and(|label| match label.get("confidence") {
            None | Some(Value::Null) => true,
            Some(Value::String(value)) => value == "high" || value == "medium",
            _ => false,
        })
}

fn count_label(label: &Value, counters: &mut Counters) {
    let confidence = label.get("confidence").and_then(Value::as_str);
    let owner_declined = label.get("owner_margin_declined") == Some(&Value::Bool(true));
    let acoustic_declined = label.get("acoustic_margin_declined") == Some(&Value::Bool(true));
    if owner_declined {
        count_demotion(
            &mut counters.demotions_by_class.owner_margin_declined,
            confidence,
        );
    }
    if acoustic_declined {
        count_demotion(
            &mut counters.demotions_by_class.acoustic_margin_declined,
            confidence,
        );
    }
    match confidence {
        Some("high") => counters.tier_histogram.high_statements += 1,
        Some("medium") => counters.tier_histogram.medium_statements += 1,
        _ if owner_declined || acoustic_declined => {
            counters.tier_histogram.margin_declined_statements += 1;
        }
        _ => counters.tier_histogram.unlabeled_sentence_statements += 1,
    }
}

fn count_demotion(counts: &mut DemotionCounts, confidence: Option<&str>) {
    match confidence {
        Some("high") => counts.high_statements += 1,
        Some("medium") => counts.medium_statements += 1,
        _ => counts.none_statements += 1,
    }
    counts.total_statements += 1;
}

fn count_corrections_file(path: &Path, counters: &mut Counters) {
    if !path.exists() {
        return;
    }
    let Some(payload) = read_json_object(path) else {
        count_unreadable(counters, UnreadableKind::Corrections);
        return;
    };
    let corrections = payload
        .get("corrections")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let Some(corrections) = corrections.as_array() else {
        count_unreadable(counters, UnreadableKind::Corrections);
        return;
    };
    if !corrections.iter().all(Value::is_object) {
        count_unreadable(counters, UnreadableKind::Corrections);
        return;
    }
    counters.corrections_window_count += corrections.len();
}

enum UnreadableKind {
    Labels,
    Corrections,
}

fn count_unreadable(counters: &mut Counters, kind: UnreadableKind) {
    match kind {
        UnreadableKind::Labels => counters.unreadable_files.speaker_labels_window_count += 1,
        UnreadableKind::Corrections => {
            counters.unreadable_files.speaker_corrections_window_count += 1;
        }
    }
    counters.unreadable_files.total_window_count += 1;
}

fn read_json_object(path: &Path) -> Option<Value> {
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value.is_object().then_some(value)
}

fn owner_voice_state(root: &Path) -> OwnerVoice {
    let voiceprint = awareness_voiceprint(root);
    let status = voiceprint
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| "none".to_owned());
    let principal_id = journal_principal_id(root);
    if let Some(principal_id) = principal_id.as_deref()
        && let Some(centroid) = owner_centroid_summary(
            &root
                .join("entities")
                .join(principal_id)
                .join("owner_centroid.npz"),
        )
    {
        return OwnerVoice {
            bootstrap_state: "bootstrapped",
            status,
            centroid_saved: true,
            evidence_tier: Some(centroid.evidence_tier),
            evidence_count: centroid.cluster_size,
            built_at: centroid.created_at,
            refreshed_at: Some(centroid.last_refreshed_at),
        };
    }
    let manual_tags_count = principal_id
        .as_deref()
        .map(|id| count_manual_owner_tags(root, id))
        .unwrap_or_default();
    OwnerVoice {
        bootstrap_state: "pre_bootstrap",
        status,
        centroid_saved: false,
        evidence_tier: voiceprint
            .get("evidence_tier")
            .and_then(Value::as_str)
            .map(str::to_owned),
        evidence_count: i32::try_from(manual_tags_count).unwrap_or(i32::MAX),
        built_at: None,
        refreshed_at: None,
    }
}

fn awareness_voiceprint(root: &Path) -> BTreeMap<String, Value> {
    let path = root.join("awareness/current.json");
    if !path.exists() {
        return BTreeMap::new();
    }
    read_json_object(&path)
        .and_then(|current| {
            current
                .get("voiceprint")
                .and_then(Value::as_object)
                .cloned()
        })
        .map(|voiceprint| voiceprint.into_iter().collect())
        .unwrap_or_default()
}

fn count_manual_owner_tags(root: &Path, principal_id: &str) -> usize {
    let Some(voiceprints) = load_voiceprints(
        &root
            .join("entities")
            .join(principal_id)
            .join("voiceprints.npz"),
    ) else {
        return 0;
    };
    let mut latest = BTreeMap::<(String, String, String, i64), (i64, usize, Value)>::new();
    for (index, row) in voiceprints.metadata.into_iter().enumerate() {
        let Some(day) = row.get("day").and_then(Value::as_str) else {
            continue;
        };
        let Some(segment_key) = row.get("segment_key").and_then(Value::as_str) else {
            continue;
        };
        let Some(source) = row.get("source").and_then(Value::as_str) else {
            continue;
        };
        let Some(sentence_id) = value_as_i64(row.get("sentence_id")) else {
            continue;
        };
        let added_at = value_as_i64(row.get("added_at")).unwrap_or(-1);
        let key = (
            day.to_owned(),
            segment_key.to_owned(),
            source.to_owned(),
            sentence_id,
        );
        if latest
            .get(&key)
            .is_none_or(|current| (added_at, index) > (current.0, current.1))
        {
            latest.insert(key, (added_at, index, row));
        }
    }
    latest
        .into_values()
        .filter(|(_, _, row)| valid_manual_owner_tag(root, principal_id, row))
        .count()
}

fn value_as_i64(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_i64)
        .or_else(|| value.and_then(Value::as_str)?.parse().ok())
}

fn valid_manual_owner_tag(root: &Path, principal_id: &str, row: &Value) -> bool {
    let Some(day) = row.get("day").and_then(Value::as_str) else {
        return false;
    };
    let Some(segment_key) = row.get("segment_key").and_then(Value::as_str) else {
        return false;
    };
    let Some(source) = row.get("source").and_then(Value::as_str) else {
        return false;
    };
    let Some(sentence_id) = value_as_i64(row.get("sentence_id")) else {
        return false;
    };
    let segment = match row
        .get("stream")
        .and_then(Value::as_str)
        .filter(|stream| !stream.is_empty())
    {
        Some(stream) => root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment_key),
        None => {
            let matches = fs::read_dir(root.join("chronicle").join(day))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path().join(segment_key))
                .filter(|path| path.is_dir())
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return false;
            }
            matches.into_iter().next().unwrap_or_default()
        }
    };
    let Some(labels) = read_json_object(&segment.join("talents/speaker_labels.json"))
        .and_then(|labels| labels.get("labels").and_then(Value::as_array).cloned())
    else {
        return false;
    };
    let label = labels
        .into_iter()
        .find(|label| value_as_i64(label.get("sentence_id")) == Some(sentence_id));
    let Some(label) = label else { return false };
    if label.get("speaker").and_then(Value::as_str) != Some(principal_id) {
        return false;
    }
    if !matches!(
        label.get("method").and_then(Value::as_str),
        Some("user_assigned" | "user_corrected" | "user_confirmed")
    ) {
        return false;
    }
    segment_overlap_fraction(&segment.join(format!("{source}.jsonl"))) <= 0.10
}

fn segment_overlap_fraction(path: &Path) -> f64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| contents.lines().next().map(str::to_owned))
        .and_then(|line| serde_json::from_str::<Value>(&line).ok())
        .and_then(|header| header.get("overlap_fraction").and_then(Value::as_f64))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::quality_status;
    use crate::speakers_npz::owner_centroid_summary;
    use std::path::Path;

    #[test]
    fn empty_journal_has_pre_bootstrap_quality() {
        let root = std::env::temp_dir().join("solstone-quality-empty-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("chronicle")).expect("chronicle creates");
        let status = serde_json::to_value(quality_status(&root)).expect("quality serializes");
        assert_eq!(status["quality_window_count"], 0);
        assert_eq!(status["owner_voice"]["bootstrap_state"], "pre_bootstrap");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_centroid_is_not_loaded() {
        assert!(owner_centroid_summary(Path::new("missing-owner-centroid.npz")).is_none());
    }
}
