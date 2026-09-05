// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Serialize;
use serde_json::Value;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::{DEFAULT_STREAM, SegmentLayout};

use crate::JournalRoot;
use crate::speakers_calendar::{
    admitted_speaker_ids, audio_embedding_sources, day_dirs, iter_segments,
    label_has_admitted_speaker, load_all_journal_entities,
};
use crate::speakers_npz::{load_voiceprints, owner_centroid_summary};
use solstone_core_speaker_resolve::segment_catalog::{
    CatalogBuildError, DirectSupport, SegmentLookup, decode_stream_layout_value, lookup_segment,
};

const QUALITY_WINDOW_DAYS: usize = 30;

pub async fn quality(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match quality_status(&root.0) {
        Ok(status) => Json(status).into_response(),
        Err(QualityError::IdentityInvalid) => error_envelope(
            "speaker_owner_identity_invalid",
            "I couldn't load speaker quality because your configured owner identity needs attention.",
            "configured owner identity is not admitted",
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        Err(QualityError::Catalog(error)) => error_envelope(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

#[derive(Debug)]
enum QualityError {
    IdentityInvalid,
    Catalog(CatalogBuildError),
}

fn quality_status(root: &Path) -> Result<QualityStatus, QualityError> {
    let principal_id = match solstone_core_speaker_resolve::owner_admission::admitted_owner_id(root)
    {
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Admitted(id) => id,
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Invalid => {
            return Err(QualityError::IdentityInvalid);
        }
    };
    let admitted_speaker_ids = admitted_speaker_ids(&load_all_journal_entities(root));
    let mut window_days = day_dirs(root).map_err(QualityError::Catalog)?;
    window_days.sort_by(|left, right| right.cmp(left));
    window_days.truncate(QUALITY_WINDOW_DAYS);
    let mut counters = Counters::default();
    for day in &window_days {
        for segment in iter_segments(root, day).map_err(QualityError::Catalog)? {
            if audio_embedding_sources(&segment.path).is_empty() {
                continue;
            }
            count_segment_quality(&segment.path, &mut counters, &admitted_speaker_ids);
        }
    }
    Ok(QualityStatus {
        quality_window_days: QUALITY_WINDOW_DAYS,
        quality_window_count: window_days.len(),
        quality_window_error_count: counters.unreadable_files.total_window_count,
        tier_histogram: counters.tier_histogram,
        demotions_by_class: counters.demotions_by_class,
        corrections_window_count: counters.corrections_window_count,
        unreadable_files: counters.unreadable_files,
        empty_labels_without_skipped_segments: counters.empty_labels_without_skipped_segments,
        owner_voice: owner_voice_state(root, &principal_id),
    })
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
    manual_tag_lookup_error_count: usize,
    built_at: Option<String>,
    refreshed_at: Option<String>,
}

fn count_segment_quality(
    segment: &Path,
    counters: &mut Counters,
    admitted_speaker_ids: &BTreeSet<String>,
) {
    count_label_file(
        &segment.join("talents/speaker_labels.json"),
        counters,
        admitted_speaker_ids,
    );
    count_corrections_file(&segment.join("talents/speaker_corrections.json"), counters);
}

fn count_label_file(path: &Path, counters: &mut Counters, admitted_speaker_ids: &BTreeSet<String>) {
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
        count_label(label, counters, admitted_speaker_ids);
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

fn count_label(label: &Value, counters: &mut Counters, admitted_speaker_ids: &BTreeSet<String>) {
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
    match quality_tier_for_label(label, admitted_speaker_ids) {
        "high_statements" => counters.tier_histogram.high_statements += 1,
        "medium_statements" => counters.tier_histogram.medium_statements += 1,
        "margin_declined_statements" => {
            counters.tier_histogram.margin_declined_statements += 1;
        }
        "unlabeled_sentence_statements" => {
            counters.tier_histogram.unlabeled_sentence_statements += 1;
        }
        _ => unreachable!("quality tier is fixed"),
    }
}

pub(crate) fn label_has_ineligible_speaker(
    label: &Value,
    admitted_speaker_ids: &BTreeSet<String>,
) -> bool {
    !label_has_admitted_speaker(label, admitted_speaker_ids)
}

pub(crate) fn quality_tier_for_label(
    label: &Value,
    admitted_speaker_ids: &BTreeSet<String>,
) -> &'static str {
    if label_has_ineligible_speaker(label, admitted_speaker_ids) {
        return "unlabeled_sentence_statements";
    }
    match label.get("confidence").and_then(Value::as_str) {
        Some("high") => "high_statements",
        Some("medium") => "medium_statements",
        _ if label.get("owner_margin_declined") == Some(&Value::Bool(true))
            || label.get("acoustic_margin_declined") == Some(&Value::Bool(true)) =>
        {
            "margin_declined_statements"
        }
        _ => "unlabeled_sentence_statements",
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

fn owner_voice_state(root: &Path, principal_id: &str) -> OwnerVoice {
    let voiceprint = awareness_voiceprint(root);
    let status = voiceprint
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| "none".to_owned());
    let manual_stats = manual_owner_tag_stats(root, principal_id);
    if let Some(centroid) = owner_centroid_summary(
        &root
            .join("entities")
            .join(principal_id)
            .join("owner_centroid.npz"),
    ) {
        return OwnerVoice {
            bootstrap_state: "bootstrapped",
            status,
            centroid_saved: true,
            evidence_tier: Some(centroid.evidence_tier),
            evidence_count: centroid.cluster_size,
            manual_tag_lookup_error_count: manual_stats.lookup_error_count,
            built_at: centroid.created_at,
            refreshed_at: Some(centroid.last_refreshed_at),
        };
    }
    OwnerVoice {
        bootstrap_state: "pre_bootstrap",
        status,
        centroid_saved: false,
        evidence_tier: voiceprint
            .get("evidence_tier")
            .and_then(Value::as_str)
            .map(str::to_owned),
        evidence_count: i32::try_from(manual_stats.manual_tags_count).unwrap_or(i32::MAX),
        manual_tag_lookup_error_count: manual_stats.lookup_error_count,
        built_at: None,
        refreshed_at: None,
    }
}

pub(crate) fn awareness_voiceprint(root: &Path) -> BTreeMap<String, Value> {
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

#[derive(Default)]
pub(crate) struct ManualOwnerTagStats {
    pub(crate) manual_tags_count: usize,
    pub(crate) streams_represented: usize,
    pub(crate) lookup_error_count: usize,
}

pub(crate) fn count_manual_owner_tags(root: &Path, principal_id: &str) -> usize {
    manual_owner_tag_stats(root, principal_id).manual_tags_count
}

pub(crate) fn manual_owner_tag_stats(root: &Path, principal_id: &str) -> ManualOwnerTagStats {
    let Some(voiceprints) = load_voiceprints(
        &root
            .join("entities")
            .join(principal_id)
            .join("voiceprints.npz"),
    ) else {
        return ManualOwnerTagStats {
            manual_tags_count: 0,
            streams_represented: 0,
            lookup_error_count: 0,
        };
    };
    let mut latest =
        BTreeMap::<(String, String, String, String, String, i64), (i64, usize, Value)>::new();
    let mut invalid_layout_count = 0;
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
        let layout = match decode_stream_layout_value(row.get("stream_layout")) {
            Ok(SegmentLayout::Direct) => "direct",
            Ok(SegmentLayout::Named) => "named",
            Err(_) => {
                invalid_layout_count += 1;
                continue;
            }
        };
        let added_at = value_as_i64(row.get("added_at")).unwrap_or(-1);
        let key = (
            day.to_owned(),
            layout.to_owned(),
            row.get("stream")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
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
    let mut manual_tags_count = 0;
    let mut lookup_error_count = invalid_layout_count;
    let mut streams = std::collections::BTreeSet::new();
    for (_, _, row) in latest.into_values() {
        match manual_owner_tag_stream(root, principal_id, &row) {
            ManualOwnerTagResolution::Tagged(stream) => {
                manual_tags_count += 1;
                if !stream.is_empty() {
                    streams.insert(stream);
                }
            }
            ManualOwnerTagResolution::LookupError => lookup_error_count += 1,
            ManualOwnerTagResolution::NotTagged => {}
        }
    }
    ManualOwnerTagStats {
        manual_tags_count,
        streams_represented: streams.len(),
        lookup_error_count,
    }
}

fn value_as_i64(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_i64)
        .or_else(|| value.and_then(Value::as_str)?.parse().ok())
}

enum ManualOwnerTagResolution {
    Tagged(String),
    NotTagged,
    LookupError,
}

fn manual_owner_tag_stream(
    root: &Path,
    principal_id: &str,
    row: &Value,
) -> ManualOwnerTagResolution {
    let Some(day) = row.get("day").and_then(Value::as_str) else {
        return ManualOwnerTagResolution::NotTagged;
    };
    let Some(segment_key) = row.get("segment_key").and_then(Value::as_str) else {
        return ManualOwnerTagResolution::NotTagged;
    };
    let Some(source) = row.get("source").and_then(Value::as_str) else {
        return ManualOwnerTagResolution::NotTagged;
    };
    let Some(sentence_id) = value_as_i64(row.get("sentence_id")) else {
        return ManualOwnerTagResolution::NotTagged;
    };
    let Ok(layout) = decode_stream_layout_value(row.get("stream_layout")) else {
        return ManualOwnerTagResolution::LookupError;
    };
    let stream = match (
        layout,
        row.get("stream")
            .and_then(Value::as_str)
            .filter(|stream| !stream.is_empty()),
    ) {
        (SegmentLayout::Direct, _) => DEFAULT_STREAM,
        (SegmentLayout::Named, Some(stream)) => stream,
        (SegmentLayout::Named, None) => return ManualOwnerTagResolution::LookupError,
    };
    let segment = match lookup_segment(
        root,
        day,
        stream,
        segment_key,
        Ok(layout),
        DirectSupport::Allow,
    ) {
        SegmentLookup::Present(path) => path,
        SegmentLookup::Absent
        | SegmentLookup::UnsupportedLayout
        | SegmentLookup::MalformedLayout
        | SegmentLookup::Failed(_) => return ManualOwnerTagResolution::LookupError,
    };
    let Some(labels) = read_json_object(&segment.join("talents/speaker_labels.json"))
        .and_then(|labels| labels.get("labels").and_then(Value::as_array).cloned())
    else {
        return ManualOwnerTagResolution::NotTagged;
    };
    let label = labels
        .into_iter()
        .find(|label| value_as_i64(label.get("sentence_id")) == Some(sentence_id));
    let Some(label) = label else {
        return ManualOwnerTagResolution::NotTagged;
    };
    if label.get("speaker").and_then(Value::as_str) != Some(principal_id) {
        return ManualOwnerTagResolution::NotTagged;
    }
    if !matches!(
        label.get("method").and_then(Value::as_str),
        Some("user_assigned" | "user_corrected" | "user_confirmed")
    ) {
        return ManualOwnerTagResolution::NotTagged;
    }
    if segment_overlap_fraction(&segment.join(format!("{source}.jsonl"))) <= 0.10 {
        ManualOwnerTagResolution::Tagged(stream.to_owned())
    } else {
        ManualOwnerTagResolution::NotTagged
    }
}

pub(crate) fn segment_overlap_fraction(path: &Path) -> f64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| contents.lines().next().map(str::to_owned))
        .and_then(|line| serde_json::from_str::<Value>(&line).ok())
        .and_then(|header| header.get("overlap_fraction").and_then(Value::as_f64))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::{quality_status, quality_tier_for_label};
    use crate::speakers_npz::owner_centroid_summary;
    use std::path::Path;

    #[test]
    fn empty_journal_has_pre_bootstrap_quality() {
        let root = std::env::temp_dir().join("solstone-quality-empty-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("chronicle")).expect("chronicle creates");
        std::fs::create_dir_all(root.join("entities/owner")).expect("owner creates");
        std::fs::write(
            root.join("entities/owner/entity.json"),
            r#"{"id":"owner","type":"Person","is_principal":true}"#,
        )
        .expect("owner writes");
        let status = serde_json::to_value(quality_status(&root).expect("quality builds"))
            .expect("quality serializes");
        assert_eq!(status["quality_window_count"], 0);
        assert_eq!(status["owner_voice"]["bootstrap_state"], "pre_bootstrap");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_centroid_is_not_loaded() {
        assert!(owner_centroid_summary(Path::new("missing-owner-centroid.npz")).is_none());
    }

    #[test]
    fn manual_owner_tag_stream_reads_direct_layout() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("journal");
        let segment = root.path().join("chronicle/20260731/080000_300");
        std::fs::create_dir_all(segment.join("talents")).expect("direct");
        std::fs::write(
            segment.join("talents/speaker_labels.json"),
            r#"{"labels":[{"sentence_id":1,"speaker":"ada_lovelace","method":"user_assigned"}]}"#,
        )
        .expect("labels");
        std::fs::write(
            segment.join("mic_audio.jsonl"),
            "{\"overlap_fraction\":0}\n",
        )
        .expect("jsonl");
        let stream = super::manual_owner_tag_stream(
            root.path(),
            "ada_lovelace",
            &serde_json::json!({
                "day": "20260731",
                "segment_key": "080000_300",
                "source": "mic_audio",
                "sentence_id": 1,
                "stream_layout": "direct"
            }),
        );
        assert!(matches!(
            stream,
            super::ManualOwnerTagResolution::Tagged(ref value) if value == "_default"
        ));
    }

    #[test]
    fn invalid_speaker_is_reclassified_as_unlabeled() {
        let admitted = BTreeSet::from(["person".to_owned()]);
        assert_eq!(
            quality_tier_for_label(&json!({"speaker":"tool","confidence":"high"}), &admitted),
            "unlabeled_sentence_statements"
        );
        assert_eq!(
            quality_tier_for_label(&json!({"speaker":"person","confidence":"high"}), &admitted),
            "high_statements"
        );
        assert_eq!(
            quality_tier_for_label(&json!({"confidence":"high"}), &admitted),
            "unlabeled_sentence_statements"
        );
    }
}
