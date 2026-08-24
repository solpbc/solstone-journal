// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI-shaped, read-only speaker routes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as RoutePath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::SegmentLayout;

use crate::JournalRoot;
use crate::speakers_calendar::{
    audio_embedding_sources, is_day, label_has_admitted_speaker, scan_segment_embeddings,
};
use crate::speakers_quality::{label_has_ineligible_speaker, quality_tier_for_label};
use crate::speakers_review::is_admissible_speaker_entity;
use crate::speakers_segment_catalog::{
    CatalogedSegment, DirectSupport, SegmentLookup, catalog_journal, decode_stream_layout,
    lookup_segment,
};

#[derive(Debug, Deserialize)]
pub struct LimitQuery {
    limit: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamLayoutQuery {
    stream_layout: Option<String>,
}

pub async fn segments(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath(day): RoutePath<String>,
    Query(query): Query<LimitQuery>,
) -> Response {
    if !is_day(&day) {
        return err(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
            StatusCode::BAD_REQUEST,
        );
    }
    let limit = match query
        .limit
        .as_deref()
        .unwrap_or("20")
        .trim()
        .parse::<usize>()
    {
        Ok(value) if value > 0 => value,
        _ => {
            return err(
                "invalid_request_value",
                "I couldn't use one of those values.",
                "limit must be a positive integer",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let mut segments = match scan_segment_embeddings(&root.0, &day) {
        Ok(segments) => segments,
        Err(error) => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    segments.sort_by(|left, right| left.key.cmp(&right.key));
    let total = segments.len();
    let values = segments
        .into_iter()
        .take(limit)
        .map(|segment| segment.payload)
        .collect::<Vec<_>>();
    Json(json!({"success":true,"day":day,"segments":values,"returned":values.len(),"limit":limit,"total":total})).into_response()
}

pub async fn review(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath((day, stream, segment_key, source)): RoutePath<(String, String, String, String)>,
    Query(query): Query<StreamLayoutQuery>,
) -> Response {
    if !is_day(&day) {
        return err(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
            StatusCode::BAD_REQUEST,
        );
    }
    let layout = decode_stream_layout(query.stream_layout.as_deref());
    let segment = match lookup_segment(
        &root.0,
        &day,
        &stream,
        &segment_key,
        layout,
        DirectSupport::Allow,
    ) {
        SegmentLookup::Present(path) => path,
        SegmentLookup::Absent => {
            return err(
                "speaker_review_unavailable",
                "I couldn't load that speaker review.",
                "No transcript found",
                StatusCode::NOT_FOUND,
            );
        }
        SegmentLookup::MalformedLayout => {
            return err(
                "invalid_segment_or_stream",
                "I couldn't use that segment or stream.",
                "Invalid segment key or stream",
                StatusCode::BAD_REQUEST,
            );
        }
        SegmentLookup::Failed(error) => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
        SegmentLookup::UnsupportedLayout => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                "segment layout is not readable",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let transcript = segment.join(format!("{source}.jsonl"));
    let Ok(raw) = fs::read_to_string(transcript) else {
        return err(
            "speaker_review_unavailable",
            "I couldn't load that speaker review.",
            "No transcript found",
            StatusCode::NOT_FOUND,
        );
    };
    let embedded = embedded_sentence_ids(&segment.join(format!("{source}.npz")));
    let labels = labels_by_sentence(&segment);
    let admitted_speaker_ids = journal_entities(&root.0)
        .iter()
        .filter(|(_, entity)| is_admissible_speaker_entity(entity))
        .map(|(entity_id, _)| entity_id.clone())
        .collect::<BTreeSet<_>>();
    let sentences = raw
        .lines()
        .skip(1)
        .enumerate()
        .filter_map(|(index, line)| {
            let entry: Value = serde_json::from_str(line).ok()?;
            let id = i64::try_from(index + 1).ok()?;
            let label = labels.get(&id);
            let active_label = active_speaker_label(label, &admitted_speaker_ids);
            let speaker = active_label
                .and_then(|value| value.get("speaker"))
                .cloned()
                .unwrap_or(Value::Null);
            let confidence = active_label
                .and_then(|value| value.get("confidence"))
                .cloned()
                .unwrap_or(Value::Null);
            let method = active_label
                .and_then(|value| value.get("method"))
                .cloned()
                .unwrap_or(Value::Null);
            let needs_review = label.is_none() || speaker.is_null();
            Some(json!({
                "sentence_id": id,
                "text": entry.get("text").cloned().unwrap_or_else(|| json!("")),
                "has_embedding": embedded.contains(&id),
                "speaker": speaker,
                "confidence": confidence,
                "method": method,
                "needs_review": needs_review,
            }))
        })
        .collect::<Vec<_>>();
    Json(json!({"success":true,"day":day,"stream_layout":layout_name(layout.expect("successful lookup decoded layout")),"stream":stream,"segment_key":segment_key,"source":source,"sentences":sentences})).into_response()
}

pub async fn status(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let owner_id = match solstone_core_speaker_resolve::owner_admission::admitted_owner_id(&root.0)
    {
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Admitted(id) => id,
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Invalid => {
            return err(
                "speaker_owner_identity_invalid",
                "I couldn't load speaker status because your configured owner identity needs attention.",
                "configured owner identity is not admitted",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let entities = journal_entities(&root.0);
    let admitted_speaker_ids = entities
        .iter()
        .filter(|(_, entity)| is_admissible_speaker_entity(entity))
        .map(|(entity_id, _)| entity_id.clone())
        .collect::<BTreeSet<_>>();
    let voiceprint = awareness_voiceprint(&root.0);
    let segments = match catalog_journal(&root.0) {
        Ok(segments) => segments,
        Err(error) => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let owner = owner_section(&root.0, &voiceprint, &owner_id);
    Json(json!({
        "embeddings": embeddings_section(&segments),
        "owner": owner,
        "speakers": speakers_section(&root.0, &entities, &admitted_speaker_ids),
        "pool": pool_section(&root.0),
        "clusters": clusters_section(&root.0),
        "imports": imports_section(&segments),
        "attribution": attribution_section(&segments, &admitted_speaker_ids),
        "quality": quality_section(&root.0, &segments, &voiceprint, &admitted_speaker_ids, &owner_id),
    }))
    .into_response()
}

pub async fn suggest(
    Extension(root): Extension<Arc<JournalRoot>>,
    Query(query): Query<LimitQuery>,
) -> Response {
    let limit = match query
        .limit
        .as_deref()
        .unwrap_or("5")
        .trim()
        .parse::<usize>()
    {
        Ok(value) => value,
        _ => {
            return err(
                "invalid_request_value",
                "I couldn't use one of those values.",
                "limit must be an integer",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let segments = match catalog_journal(&root.0) {
        Ok(segments) => segments,
        Err(error) => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let mut suggestions = Vec::new();
    suggestions.extend(import_linkable(&root.0, &segments));
    suggestions.extend(candidate_pair_suggestions(&root.0));
    suggestions.extend(low_confidence_suggestions(&segments));
    suggestions.sort_by_key(suggestion_sort_key);
    let items = suggestions.into_iter().take(limit).collect::<Vec<_>>();
    Json(json!({"status":"ok","items":items,"issues":[],"markdown":format_suggestions(&items)}))
        .into_response()
}

pub async fn keep_separate(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let assertions =
        fold_keep_separate(&jsonl_values(&root.0.join("speakers/keep-separate.jsonl")));
    Json(json!({"assertions":assertions,"total":assertions.len()})).into_response()
}

pub async fn dismissals(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let dismissals = fold_dismissals(&jsonl_values(
        &root.0.join("speakers/cluster-dismissals.jsonl"),
    ));
    Json(json!({"dismissals":dismissals,"total":dismissals.len()})).into_response()
}

fn err(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}

fn labels_by_sentence(segment: &Path) -> BTreeMap<i64, Value> {
    read_json(&segment.join("talents/speaker_labels.json"))
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|label| {
            label
                .get("sentence_id")
                .and_then(Value::as_i64)
                .map(|id| (id, label))
        })
        .collect()
}

fn active_speaker_label<'a>(
    label: Option<&'a Value>,
    admitted_speaker_ids: &BTreeSet<String>,
) -> Option<&'a Value> {
    label.filter(|label| label_has_admitted_speaker(label, admitted_speaker_ids))
}

fn embedded_sentence_ids(path: &Path) -> BTreeSet<i64> {
    // A malformed or absent NPZ means no sentence is embedded; this mirrors the
    // browser reader's conservative presentation behavior.
    crate::speakers_npz::load_segment_embeddings(path)
        .map(|data| data.statement_ids.into_iter().map(i64::from).collect())
        .unwrap_or_default()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn jsonl_values(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .into_iter()
        .flat_map(|raw| {
            raw.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect::<Vec<Value>>()
        })
        .collect()
}

fn journal_entities(root: &Path) -> BTreeMap<String, Value> {
    solstone_core_entity::load_all_journal_entities(root)
        .unwrap_or_default()
        .into_iter()
        .map(|entity| (entity.id, entity.value))
        .collect()
}

fn awareness_voiceprint(root: &Path) -> BTreeMap<String, Value> {
    read_json(&root.join("awareness/current.json"))
        .and_then(|value| value.get("voiceprint").and_then(Value::as_object).cloned())
        .map(|value| value.into_iter().collect())
        .unwrap_or_default()
}

fn owner_centroid_exists(root: &Path, owner_id: &str) -> bool {
    root.join("entities")
        .join(owner_id)
        .join("owner_centroid.npz")
        .exists()
}

fn embeddings_section(segments: &[CatalogedSegment]) -> Value {
    let mut streams = BTreeMap::<String, usize>::new();
    let mut days = BTreeSet::new();
    let mut total = 0usize;
    for segment in segments {
        if audio_embedding_sources(&segment.path).is_empty() {
            continue;
        }
        total += 1;
        days.insert(segment.day.clone());
        *streams.entry(segment.stream.clone()).or_default() += 1;
    }
    let range = days
        .first()
        .zip(days.last())
        .map(|(first, last)| json!([first, last]));
    json!({"segments":total,"streams":streams,"days":days.len(),"date_range":range})
}

fn owner_section(root: &Path, voiceprint: &BTreeMap<String, Value>, owner_id: &str) -> Value {
    let status = voiceprint
        .get("status")
        .cloned()
        .unwrap_or_else(|| json!("none"));
    let mut result = Map::new();
    result.insert("status".to_owned(), status.clone());
    let status_text = status.as_str().unwrap_or("none");
    if status_text == "candidate" {
        for key in [
            "cluster_size",
            "detected_at",
            "streams_represented",
            "recommendation",
            "evidence_tier",
        ] {
            result.insert(
                key.to_owned(),
                voiceprint.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        result.insert("candidate_available".to_owned(), Value::Bool(true));
        result.insert("next_step".to_owned(), json!("confirm_candidate"));
        result.insert(
            "guidance".to_owned(),
            json!("Confirm this candidate if it is your voice."),
        );
    } else if status_text == "low_quality" {
        for (key, default) in [
            ("source", json!("candidate_pool")),
            ("low_quality_reason", json!("")),
            ("observed_value", json!(0.0)),
            ("threshold_value", json!(0.0)),
            ("evidence_tier", Value::Null),
            ("intra_cosine_p25_bound", Value::Null),
            ("segments_checked", json!(0)),
            ("attempted_at", json!("")),
        ] {
            result.insert(
                key.to_owned(),
                voiceprint.get(key).cloned().unwrap_or(default),
            );
        }
    } else if status_text == "no_cluster" {
        for key in ["segments_checked", "attempted_at"] {
            result.insert(
                key.to_owned(),
                voiceprint.get(key).cloned().unwrap_or(Value::Null),
            );
        }
    }
    let centroid_path = root
        .join("entities")
        .join(owner_id)
        .join("owner_centroid.npz");
    let centroid = crate::speakers_npz::owner_centroid_summary(&centroid_path);
    result.insert("centroid_saved".to_owned(), Value::Bool(centroid.is_some()));
    if status_text == "confirmed"
        && let Some(centroid) = centroid
    {
        result.insert("centroid_metadata".to_owned(), json!({"cluster_size":centroid.cluster_size,"streams":Value::Null,"created_at":centroid.created_at,"last_refreshed_at":centroid.last_refreshed_at,"threshold":centroid.threshold,"margin":centroid.margin,"intra_cosine_p25":Value::Null,"evidence_hash":centroid.evidence_hash,"evidence_intra_cosine_p25":centroid.evidence_intra_cosine_p25,"evidence_tier":centroid.evidence_tier}));
    }
    Value::Object(result)
}

fn speakers_section(
    root: &Path,
    entities: &BTreeMap<String, Value>,
    admitted_speaker_ids: &BTreeSet<String>,
) -> Value {
    let mut speakers = Vec::new();
    for (id, entity) in entities {
        if !admitted_speaker_ids.contains(id) {
            continue;
        }
        let Some(archive) = solstone_core_entity::load_entity_voiceprints_file(root, id) else {
            continue;
        };
        let metadata = archive
            .metadata
            .iter()
            .filter_map(|row| serde_json::from_str::<Value>(row).ok())
            .collect::<Vec<_>>();
        let streams = metadata
            .iter()
            .filter_map(|row| row.get("stream").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>();
        let segments = metadata
            .iter()
            .map(|row| {
                (
                    row.get("day").and_then(Value::as_str).unwrap_or(""),
                    row.get("segment_key").and_then(Value::as_str).unwrap_or(""),
                )
            })
            .collect::<BTreeSet<_>>();
        let last_seen = metadata
            .iter()
            .filter_map(|row| row.get("last_seen_ts").and_then(Value::as_i64))
            .max();
        speakers.push(json!({"entity_id":id,"name":entity.get("name").cloned().unwrap_or_else(|| json!(id)),"embedding_count":archive.rows,"segment_count":segments.len(),"streams":streams,"last_seen_ts":last_seen,"intra_cosine_p25":Value::Null}));
    }
    Value::Array(speakers)
}

fn pool_section(root: &Path) -> Value {
    let candidates = read_json(&root.join("awareness/speaker_candidates.json"))
        .and_then(|value| value.get("candidates").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut statuses = BTreeMap::<String, usize>::new();
    let dense = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .get("n_intervals")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                >= 3
        })
        .count();
    for candidate in &candidates {
        *statuses
            .entry(
                candidate
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )
            .or_default() += 1;
    }
    json!({"candidate_count":candidates.len(),"dense_count":dense,"status_breakdown":statuses,"consolidation_summary":read_json(&root.join("awareness/speaker_candidates.json")).and_then(|value| value.get("consolidation_summary").cloned()).unwrap_or(Value::Null)})
}

fn clusters_section(root: &Path) -> Value {
    let Some(data) = read_json(&root.join("awareness/discovery_clusters.json")) else {
        return Value::Null;
    };
    let clusters = data
        .get("clusters")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    json!({"cached_at":data.get("version").cloned().unwrap_or(Value::Null),"count":clusters.len(),"clusters":clusters})
}

fn imports_section(segments: &[CatalogedSegment]) -> Value {
    json!({"meetings_files":segments.iter().filter(|segment| segment.path.join("meetings.md").exists()).count(),"screen_files":segments.iter().filter(|segment| segment.path.join("talents/screen.json").exists()).count()})
}

fn attribution_section(
    segments: &[CatalogedSegment],
    admitted_speaker_ids: &BTreeSet<String>,
) -> Value {
    let mut files = 0usize;
    let mut labels = 0usize;
    let mut confidence = BTreeMap::<String, usize>::new();
    let mut method = BTreeMap::<String, usize>::new();
    for segment in segments {
        let Some(rows) = read_json(&segment.path.join("talents/speaker_labels.json"))
            .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
        else {
            continue;
        };
        files += 1;
        for row in rows {
            labels += 1;
            let ineligible = label_has_ineligible_speaker(&row, admitted_speaker_ids);
            *confidence
                .entry(
                    (!ineligible)
                        .then(|| row.get("confidence"))
                        .flatten()
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned(),
                )
                .or_default() += 1;
            *method
                .entry(
                    (!ineligible)
                        .then(|| row.get("method"))
                        .flatten()
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned(),
                )
                .or_default() += 1;
        }
    }
    json!({"files":files,"labels":labels,"by_confidence":confidence,"by_method":method})
}

fn quality_section(
    root: &Path,
    segments: &[CatalogedSegment],
    voiceprint: &BTreeMap<String, Value>,
    admitted_speaker_ids: &BTreeSet<String>,
    owner_id: &str,
) -> Value {
    let mut tier = json!({"high_statements":0,"medium_statements":0,"margin_declined_statements":0,"unlabeled_sentence_statements":0,"skipped_stub_segments":0,"no_labels_file_segments":0});
    let mut unreadable = json!({"speaker_labels_window_count":0,"speaker_corrections_window_count":0,"total_window_count":0});
    let mut corrections = 0usize;
    let mut empty = 0usize;
    for segment in segments.iter().rev().take(30) {
        if !fs::read_dir(&segment.path)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                entry.path().extension().and_then(|ext| ext.to_str()) == Some("npz")
                    && (entry.file_name().to_string_lossy() == "audio.npz"
                        || entry.file_name().to_string_lossy().ends_with("_audio.npz"))
            })
        {
            continue;
        }
        match read_json(&segment.path.join("talents/speaker_labels.json")) {
            None => {
                tier["no_labels_file_segments"] =
                    json!(tier["no_labels_file_segments"].as_u64().unwrap_or(0) + 1)
            }
            Some(payload) => match payload.get("labels").and_then(Value::as_array) {
                None => {
                    unreadable["speaker_labels_window_count"] = json!(
                        unreadable["speaker_labels_window_count"]
                            .as_u64()
                            .unwrap_or(0)
                            + 1
                    );
                    unreadable["total_window_count"] =
                        json!(unreadable["total_window_count"].as_u64().unwrap_or(0) + 1);
                }
                Some(rows) => {
                    if payload.get("skipped") == Some(&Value::Bool(true)) {
                        tier["skipped_stub_segments"] =
                            json!(tier["skipped_stub_segments"].as_u64().unwrap_or(0) + 1);
                    } else if rows.is_empty() {
                        empty += 1;
                    }
                    for row in rows {
                        let target = quality_tier_for_label(row, admitted_speaker_ids);
                        tier[target] = json!(tier[target].as_u64().unwrap_or(0) + 1);
                    }
                }
            },
        }
        if let Some(payload) = read_json(&segment.path.join("talents/speaker_corrections.json")) {
            if let Some(rows) = payload.get("corrections").and_then(Value::as_array) {
                corrections += rows.len();
            } else {
                unreadable["speaker_corrections_window_count"] = json!(
                    unreadable["speaker_corrections_window_count"]
                        .as_u64()
                        .unwrap_or(0)
                        + 1
                );
                unreadable["total_window_count"] =
                    json!(unreadable["total_window_count"].as_u64().unwrap_or(0) + 1);
            }
        }
    }
    let centroid = owner_centroid_exists(root, owner_id);
    json!({"quality_window_days":30,"quality_window_count":segments.iter().map(|segment| &segment.day).collect::<BTreeSet<_>>().len().min(30),"quality_window_error_count":unreadable["total_window_count"],"tier_histogram":tier,"demotions_by_class":{"owner_margin_declined":{"high_statements":0,"medium_statements":0,"none_statements":0,"total_statements":0},"acoustic_margin_declined":{"high_statements":0,"medium_statements":0,"none_statements":0,"total_statements":0}},"corrections_window_count":corrections,"unreadable_files":unreadable,"empty_labels_without_skipped_segments":empty,"owner_voice":{"bootstrap_state":if centroid { "bootstrapped" } else { "pre_bootstrap" },"status":voiceprint.get("status").cloned().unwrap_or_else(|| json!("none")),"centroid_saved":centroid,"evidence_tier":voiceprint.get("evidence_tier").cloned().unwrap_or(Value::Null),"evidence_count":0,"built_at":Value::Null,"refreshed_at":Value::Null}})
}

fn import_linkable(root: &Path, segments: &[CatalogedSegment]) -> Vec<Value> {
    let mut participants = BTreeMap::<String, (usize, BTreeSet<String>)>::new();
    for segment in segments {
        let Ok(text) = fs::read_to_string(segment.path.join("meetings.md")) else {
            continue;
        };
        for line in text.lines() {
            for name in line
                .split(',')
                .map(str::trim)
                .filter(|name| name.contains(' '))
            {
                let entry = participants.entry(name.to_ascii_lowercase()).or_default();
                entry.0 += 1;
                entry.1.insert(segment.day.clone());
            }
        }
    }
    let mut values = Vec::new();
    for (id, entity) in journal_entities(root) {
        if entity.get("is_principal").and_then(Value::as_bool) == Some(true)
            || entity.get("blocked").and_then(Value::as_bool) == Some(true)
            || root
                .join("entities")
                .join(&id)
                .join("voiceprints.npz")
                .exists()
        {
            continue;
        }
        let mut names = entity
            .get("aka")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        names.push(entity.get("name").cloned().unwrap_or_else(|| json!("")));
        let matches = participants
            .iter()
            .filter(|(participant, _)| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|name| name.trim().eq_ignore_ascii_case(participant))
            })
            .collect::<Vec<_>>();
        let count = matches.iter().map(|(_, info)| info.0).sum::<usize>();
        if count == 0 {
            continue;
        }
        let days = matches
            .into_iter()
            .flat_map(|(_, info)| info.1.iter().cloned())
            .collect::<BTreeSet<_>>();
        values.push(json!({"type":"import_linkable","entity_id":id,"name":entity.get("name").cloned().unwrap_or_else(|| json!(id)),"meetings_mentioned":count,"meeting_days":days}));
    }
    values.sort_by_key(|value| {
        std::cmp::Reverse(
            value
                .get("meetings_mentioned")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
    });
    values
}

fn candidate_pair_suggestions(root: &Path) -> Vec<Value> {
    let mut values = jsonl_values(&root.join("speakers/candidate-pair-review-candidates.jsonl")).into_iter().filter(|row| row.get("status").and_then(Value::as_str) == Some("open")).map(|row| { let evidence = row.get("evidence").cloned().unwrap_or_else(|| json!({})); json!({"type":"speaker_candidate_pair","key":row.get("key").cloned().unwrap_or(Value::Null),"anchor_a":row.get("anchor_a").cloned().unwrap_or(Value::Null),"anchor_b":row.get("anchor_b").cloned().unwrap_or(Value::Null),"similarity":row.get("similarity").or_else(|| evidence.get("similarity")).cloned().unwrap_or_else(|| json!(0.0)),"source_intervals":evidence.get("source_intervals").cloned().unwrap_or_else(|| json!(0)),"target_intervals":evidence.get("target_intervals").cloned().unwrap_or_else(|| json!(0))}) }).collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .get("similarity")
            .and_then(Value::as_f64)
            .partial_cmp(&left.get("similarity").and_then(Value::as_f64))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    values
}

fn low_confidence_suggestions(segments: &[CatalogedSegment]) -> Vec<Value> {
    let mut values = Vec::new();
    for segment in segments {
        let Some(rows) = read_json(&segment.path.join("talents/speaker_labels.json"))
            .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
        else {
            continue;
        };
        let total = rows.len();
        let medium = rows
            .iter()
            .filter(|row| row.get("confidence").and_then(Value::as_str) != Some("high"))
            .count();
        if medium <= 10 {
            continue;
        }
        let missing = rows
            .iter()
            .filter(|row| {
                row.get("speaker")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            })
            .count();
        values.push(json!({"type":"low_confidence_review","day":segment.day,"segment_key":segment.name,"stream":segment.stream,"stream_layout":layout_name(segment.layout),"medium_or_null_count":medium,"total_labels":total,"has_speakers":segment.path.join("talents/speakers.json").is_file(),"null_proportion": if total == 0 { 0.0 } else { missing as f64 / total as f64 }}));
    }
    values.sort_by(|left, right| {
        left.get("has_speakers")
            .and_then(Value::as_bool)
            .cmp(&right.get("has_speakers").and_then(Value::as_bool))
            .then_with(|| {
                right
                    .get("null_proportion")
                    .and_then(Value::as_f64)
                    .partial_cmp(&left.get("null_proportion").and_then(Value::as_f64))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    values
}

fn layout_name(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}

fn suggestion_sort_key(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn format_suggestions(items: &[Value]) -> String {
    if items.is_empty() {
        return "No speaker curation suggestions found.".to_owned();
    }
    items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("import_linkable") => Some(format!(
                "Import linkable: {} ({}) — mentioned in {} meetings\n  Days: {}",
                item.get("name").and_then(Value::as_str).unwrap_or_default(),
                item.get("entity_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                item.get("meetings_mentioned")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                item.get("meeting_days")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Some("speaker_candidate_pair") => Some(format!(
                "Speaker candidate pair: similarity {:.2} ({} vs {} intervals)",
                item.get("similarity")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                item.get("source_intervals")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                item.get("target_intervals")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            )),
            Some("low_confidence_review") => Some(format!(
                "Low confidence review: {}/{} — {} of {} labels are medium/unresolved",
                item.get("day").and_then(Value::as_str).unwrap_or_default(),
                item.get("segment_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                item.get("medium_or_null_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                item.get("total_labels")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fold_keep_separate(events: &[Value]) -> Vec<Value> {
    let mut sources = BTreeMap::<String, BTreeMap<(String, Option<String>), Value>>::new();
    for event in events {
        let Some(pair) = event.get("pair_key").and_then(Value::as_str) else {
            continue;
        };
        let Some(kind) = event.get("source_kind").and_then(Value::as_str) else {
            continue;
        };
        let operation = event
            .get("operation_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let key = (kind.to_owned(), operation.clone());
        let group = sources.entry(pair.to_owned()).or_default();
        if event.get("event_kind").and_then(Value::as_str) == Some("source_removed") {
            group.remove(&key);
            continue;
        }
        let replace = group
            .get(&key)
            .and_then(|previous| previous.get("detection_count").and_then(Value::as_i64))
            .unwrap_or(-1)
            < event
                .get("detection_count")
                .and_then(Value::as_i64)
                .unwrap_or(-1);
        if replace {
            group.insert(key, event.clone());
        }
    }
    let mut assertions = Vec::new();
    for (pair, rows) in sources {
        if rows.is_empty() {
            continue;
        }
        let values = rows.into_values().collect::<Vec<_>>();
        let ids = pair.split('|').collect::<Vec<_>>();
        if ids.len() != 2 {
            continue;
        }
        let timestamps = values
            .iter()
            .filter_map(|row| row.get("ts").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assertions.push(json!({"assertion_id":format!("ks_{}", stable_id(&pair)),"pair_key":pair,"entity_id_a":ids[0],"entity_id_b":ids[1],"dismissed_detection_count":values.iter().filter_map(|row| row.get("detection_count").and_then(Value::as_i64)).max().unwrap_or(0),"source_count":values.len(),"created_at":timestamps.iter().min(),"updated_at":timestamps.iter().max(),"last_recorded_at":timestamps.iter().max()}));
    }
    assertions
}

fn fold_dismissals(events: &[Value]) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut values = Vec::new();
    for event in events {
        let Some(id) = event.get("dismiss_event_id").and_then(Value::as_str) else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let members = event
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        values.push(json!({"dismissal_id":format!("cdsm_{}",stable_id(id)),"disposition":event.get("disposition").cloned().unwrap_or_else(|| json!("quiet")),"member_count":members.len(),"event_count":1,"created_at":event.get("ts").cloned().unwrap_or(Value::Null),"updated_at":event.get("ts").cloned().unwrap_or(Value::Null)}));
    }
    values.sort_by(|left, right| {
        left.get("dismissal_id")
            .and_then(Value::as_str)
            .cmp(&right.get("dismissal_id").and_then(Value::as_str))
    });
    values
}

fn stable_id(input: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:024x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use solstone_core_journal_io::SegmentLayout;

    use super::{
        CatalogedSegment, active_speaker_label, attribution_section, quality_section,
        speakers_section,
    };

    #[test]
    fn invalid_speakers_are_unassigned_in_cli_read_projections() {
        let root = std::env::temp_dir().join(format!(
            "solstone-cli-read-admission-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let segment = root.join("segment");
        fs::create_dir_all(segment.join("talents")).expect("talents directory");
        fs::write(segment.join("audio.npz"), []).expect("audio marker");
        fs::write(
            segment.join("talents/speaker_labels.json"),
            json!({"labels":[
                {"speaker":"person","confidence":"high","method":"automatic"},
                {"speaker":"tool","confidence":"high","method":"automatic"}
            ]})
            .to_string(),
        )
        .expect("labels");
        let entities = BTreeMap::from([
            (
                "person".to_owned(),
                json!({"type":"Person","name":"Person"}),
            ),
            ("tool".to_owned(), json!({"type":"Tool","name":"Tool"})),
        ]);
        for entity_id in ["person", "tool"] {
            let entity_dir = root.join("entities").join(entity_id);
            fs::create_dir_all(&entity_dir).expect("entity directory");
            fs::write(
                entity_dir.join("entity.json"),
                serde_json::to_vec(&entities[entity_id]).expect("entity json"),
            )
            .expect("entity writes");
            solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
                &root,
                entity_id,
                vec![1.0; 256],
                json!({"day":"20260808","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
                &solstone_core_entity::EncoderIdentity {
                    id: "test".to_owned(),
                    sha256: "0".repeat(64),
                    width: 256,
                },
            )
            .expect("voiceprint writes");
        }
        let segments = vec![CatalogedSegment {
            day: "20260808".to_owned(),
            layout: SegmentLayout::Named,
            stream: "main".to_owned(),
            name: "120000_1".to_owned(),
            key: "120000_1".to_owned(),
            path: segment,
        }];
        let admitted = BTreeSet::from(["person".to_owned()]);

        let speakers = speakers_section(&root, &entities, &admitted);
        assert_eq!(speakers.as_array().map(Vec::len), Some(1));
        assert_eq!(speakers[0]["entity_id"], "person");

        let attribution = attribution_section(&segments, &admitted);
        assert_eq!(attribution["by_confidence"]["high"], 1);
        assert_eq!(attribution["by_confidence"]["unknown"], 1);
        assert_eq!(attribution["by_method"]["automatic"], 1);
        assert_eq!(attribution["by_method"]["unknown"], 1);

        let quality = quality_section(&root, &segments, &BTreeMap::new(), &admitted, "person");
        assert_eq!(quality["tier_histogram"]["high_statements"], 1);
        assert_eq!(
            quality["tier_histogram"]["unlabeled_sentence_statements"],
            1
        );

        assert!(active_speaker_label(Some(&json!({"speaker":"tool"})), &admitted,).is_none());
        assert!(active_speaker_label(Some(&json!({"speaker":"person"})), &admitted,).is_some());
        let _ = fs::remove_dir_all(root);
    }
}
