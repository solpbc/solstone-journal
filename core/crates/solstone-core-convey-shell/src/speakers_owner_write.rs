// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native mutations for the owner-voice workflow.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::{
    JsonWriteOptions, LockOptions, SegmentLayout, hold_lock, write_json,
};
use solstone_core_speaker_resolve::candidate_tracker::{
    CandidateProfile, CandidateTracker, trim_solo_cluster_rows,
};
use solstone_core_speaker_resolve::owner_admission::{OwnerAdmission, admitted_owner_id};
use solstone_core_speaker_resolve::owner_candidate::{clear_owner_candidate, load_owner_candidate};
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidRebuildInput, OwnerCentroidRebuildOutcome, OwnerCentroidWriteInput,
    load_owner_centroid, rebuild_owner_centroid, write_owner_centroid,
};
use solstone_core_speaker_resolve::owner_provisional::collect_manual_owner_embeddings;

use crate::JournalRoot;
use crate::speakers_attribution::action;
use crate::speakers_known::intra_cosine_p25;
use crate::speakers_quality::awareness_voiceprint;
use crate::speakers_segment_catalog::{
    DirectSupport, SegmentLookup, decode_stream_layout_value, lookup_segment,
};

const MIN_STATEMENTS: usize = 30;
const STRONG_STATEMENTS: usize = 100;
const MIN_MEDIAN_DURATION: f64 = 1.5;
const STANDARD_P25: f64 = 0.30;
const STRONG_P25: f64 = 0.15;
const COOLDOWN_DAYS: i64 = 14;
const CANDIDATE_EXPANSION_MAX_EMBEDDINGS: usize = 3000;
const OWNER_THRESHOLD: f32 = 0.43;
const EXISTING_CENTROID_GUIDANCE: &str = "Owner centroid already exists. Run solstone call speakers rebuild-owner to refresh it from current manual tags.";
const OWNER_IDENTITY_INVALID: &str = "speaker_owner_identity_invalid";
const OWNER_IDENTITY_INVALID_MESSAGE: &str =
    "I couldn't run that speaker command because your configured owner identity needs attention.";
const OWNER_IDENTITY_INVALID_DETAIL: &str = "configured owner identity is not admitted";

pub async fn detect(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = optional_json(request).await;
    let force = body.get("force") == Some(&Value::Bool(true));
    match detect_owner_candidate(&root.0, force) {
        Ok(value) => Json(value).into_response(),
        Err(error) => owner_error(error),
    }
}

pub async fn build_from_tags(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match bootstrap_owner_from_manual_tags(&root.0) {
        Ok(value) if value["reason_code"] == OWNER_IDENTITY_INVALID => owner_identity_invalid(),
        Ok(value) if value.get("error").is_some() => err(
            "entity_not_found",
            "I couldn't find that person.",
            value["error"].as_str().unwrap_or("owner build failed"),
            StatusCode::BAD_REQUEST,
        ),
        Ok(value) => {
            if value["status"] == "confirmed"
                && let Err(error) = action(
                    &root.0,
                    "owner_voiceprint_build_from_tags",
                    json!({"principal_id":value["principal_id"],"cluster_size":value["cluster_size"]}),
                )
            {
                return owner_error(error);
            }
            Json(value).into_response()
        }
        Err(error) => owner_error(error),
    }
}

pub async fn rebuild(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = optional_json(request).await;
    let override_regression = body.get("override") == Some(&Value::Bool(true));
    match rebuild_owner(&root.0, override_regression) {
        Ok(value) => {
            if value["reason_code"] == OWNER_IDENTITY_INVALID {
                return owner_identity_invalid();
            }
            if value["status"] == "rebuilt"
                && let Err(error) = action(
                    &root.0,
                    "owner_voiceprint_rebuild",
                    json!({"principal_id":value["principal_id"],"cluster_size":value["cluster_size"],"override":value["override_applied"]}),
                )
            {
                return owner_error(error);
            }
            Json(value).into_response()
        }
        Err(error) => owner_error(error),
    }
}

pub async fn confirm(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let Some(principal_id) = admitted_principal_id(&root.0) else {
        return owner_identity_invalid();
    };
    let candidate = match load_owner_candidate(&root.0) {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            return err(
                "speaker_review_unavailable",
                "I couldn't load that speaker review.",
                "No candidate available",
                StatusCode::NOT_FOUND,
            );
        }
        Err(error) => return owner_error(error.to_string()),
    };
    if let Err(error) = write_owner_centroid(
        &root.0,
        &principal_id,
        &OwnerCentroidWriteInput {
            centroid: candidate.centroid,
            cluster_size: candidate.cluster_size,
            timestamp: Utc::now().to_rfc3339(),
            evidence_tier: candidate.evidence_tier.clone(),
        },
    ) {
        return owner_error(error.to_string());
    }
    if let Err(error) = clear_owner_candidate(&root.0) {
        return owner_error(error.to_string());
    }
    if let Err(error) = update_voiceprint(
        &root.0,
        json!({"status":"confirmed","cluster_size":candidate.cluster_size,"confirmed_at":Utc::now().to_rfc3339(),"evidence_tier":candidate.evidence_tier}),
    ) {
        return owner_error(error);
    }
    if let Err(error) = action(
        &root.0,
        "owner_voiceprint_confirm",
        json!({"principal_id":principal_id,"cluster_size":candidate.cluster_size}),
    ) {
        return owner_error(error);
    }
    Json(json!({"status":"confirmed","principal_id":principal_id})).into_response()
}

pub async fn reject(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    if admitted_principal_id(&root.0).is_none() {
        return owner_identity_invalid();
    }
    if let Err(error) = clear_owner_candidate(&root.0) {
        return owner_error(error.to_string());
    }
    if let Err(error) = update_voiceprint(
        &root.0,
        json!({"status":"rejected","rejected_at":Utc::now().to_rfc3339()}),
    ) {
        return owner_error(error);
    }
    Json(json!({"status":"needs_detection"})).into_response()
}

pub async fn classify(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match required_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(day) = body.get("day").and_then(Value::as_str) else {
        return missing_fields();
    };
    let Some(stream) = body.get("stream").and_then(Value::as_str) else {
        return missing_fields();
    };
    let Some(segment_key) = body.get("segment_key").and_then(Value::as_str) else {
        return missing_fields();
    };
    let Some(source) = body.get("source").and_then(Value::as_str) else {
        return missing_fields();
    };
    let Some(principal) = admitted_principal_id(&root.0) else {
        return owner_identity_invalid();
    };
    if !valid_day(day) {
        return err(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
            StatusCode::BAD_REQUEST,
        );
    }
    let segment = match lookup_segment(
        &root.0,
        day,
        stream,
        segment_key,
        decode_stream_layout_value(body.get("stream_layout")),
        DirectSupport::Allow,
    ) {
        SegmentLookup::Present(path) => path,
        SegmentLookup::Absent => return Json(json!({"sentences":[]})).into_response(),
        SegmentLookup::MalformedLayout => {
            return err(
                "invalid_segment_or_stream",
                "I couldn't use that segment or stream.",
                "Invalid segment key",
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
    let Some(centroid) = load_owner_centroid(&root.0, &principal).ok().flatten() else {
        return Json(json!({"sentences":[]})).into_response();
    };
    let path = segment.join(format!("{source}.npz"));
    let sentences = solstone_core_speaker_id::embeddings::load_embeddings_file(&path).ok().flatten().map(|file| file.statements.into_iter().filter_map(|(id, row)| {
        let norm = solstone_core_entity::normalize_embedding(&row)?;
        let score: f32 = norm.iter().zip(&centroid.centroid).map(|(a,b)| a*b).sum();
        Some(json!({"sentence_id":id,"is_owner":score >= centroid.threshold,"score":(score * 10000.0).round() / 10000.0}))
    }).collect::<Vec<_>>()).unwrap_or_default();
    Json(json!({"sentences":sentences})).into_response()
}

pub async fn ready(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match owner_detection_ready(&root.0) {
        Ok(value) => Json(value).into_response(),
        Err(()) => owner_identity_invalid(),
    }
}

/// Shared in-process owner bootstrap used by attribution's owner-teach path.
pub(crate) fn bootstrap_owner_from_manual_tags(root: &Path) -> Result<Value, String> {
    let Some(principal_id) = admitted_principal_id(root) else {
        return Ok(identity_invalid_value());
    };
    if let Ok(Some(centroid)) = load_owner_centroid(root, &principal_id) {
        return Ok(
            json!({"status":"confirmed","principal_id":principal_id,"cluster_size":centroid.cluster_size,"evidence_tier":centroid.evidence_tier.unwrap_or_else(|| "standard".to_owned()),"next_step":"rebuild_owner","guidance":EXISTING_CENTROID_GUIDANCE}),
        );
    }
    let evidence =
        collect_manual_owner_embeddings(root, &principal_id).map_err(|error| error.to_string())?;
    let quality = quality(&evidence);
    if let Some(reason) = quality.reason {
        return Ok(low_quality(reason, &quality, &evidence, "manual_tags"));
    }
    let centroid = mean(
        &evidence
            .iter()
            .map(|item| item.embedding.clone())
            .collect::<Vec<_>>(),
    )
    .and_then(|row| solstone_core_entity::normalize_embedding(&row));
    let Some(centroid) = centroid else {
        return Ok(low_quality(
            "cluster_too_diffuse",
            &quality,
            &evidence,
            "manual_tags",
        ));
    };
    write_owner_centroid(
        root,
        &principal_id,
        &OwnerCentroidWriteInput {
            centroid,
            cluster_size: evidence.len() as i32,
            timestamp: Utc::now().to_rfc3339(),
            evidence_tier: quality.tier.to_owned(),
        },
    )
    .map_err(|error| error.to_string())?;
    update_voiceprint(
        root,
        json!({"status":"confirmed","cluster_size":evidence.len(),"confirmed_at":Utc::now().to_rfc3339(),"evidence_tier":quality.tier} ),
    )?;
    Ok(
        json!({"status":"confirmed","principal_id":principal_id,"cluster_size":evidence.len(),"evidence_tier":quality.tier}),
    )
}

fn rebuild_owner(root: &Path, override_regression: bool) -> Result<Value, String> {
    let Some(principal_id) = admitted_principal_id(root) else {
        return Ok(identity_invalid_value());
    };
    if load_owner_centroid(root, &principal_id)
        .ok()
        .flatten()
        .is_none()
    {
        return Ok(rebuild_refusal("no_owner_centroid"));
    }
    let evidence =
        collect_manual_owner_embeddings(root, &principal_id).map_err(|error| error.to_string())?;
    let quality = quality(&evidence);
    if let Some(reason) = quality.reason {
        return Ok(low_quality(reason, &quality, &evidence, "manual_tags"));
    }
    let Some(centroid) = mean(
        &evidence
            .iter()
            .map(|item| item.embedding.clone())
            .collect::<Vec<_>>(),
    )
    .and_then(|row| solstone_core_entity::normalize_embedding(&row)) else {
        return Ok(low_quality(
            "cluster_too_diffuse",
            &quality,
            &evidence,
            "manual_tags",
        ));
    };
    let hash = evidence_hash(&evidence);
    let outcome = rebuild_owner_centroid(
        root,
        &principal_id,
        &OwnerCentroidRebuildInput {
            centroid,
            embeddings_count: evidence.len() as i32,
            timestamp: Utc::now().to_rfc3339(),
            evidence_hash: hash,
            evidence_intra_cosine_p25: quality.p25 as f32,
            evidence_tier: quality.tier.to_owned(),
            override_regression,
        },
    )
    .map_err(|error| error.to_string())?;
    let mut value = json!({"principal_id":principal_id,"cluster_size":evidence.len(),"streams_represented":evidence.iter().map(|item| item.stream.clone()).collect::<BTreeSet<_>>().len(),"evidence_tier":quality.tier,"evidence_quality":{"median_duration_s":quality.median,"intra_cosine_p25":quality.p25}});
    let object = value.as_object_mut().expect("object");
    match outcome {
        OwnerCentroidRebuildOutcome::Rebuilt { override_applied } => {
            object.insert("status".to_owned(), json!("rebuilt"));
            object.insert("override_applied".to_owned(), json!(override_applied));
            object.insert("next_step".to_owned(), json!("none"));
            object.insert("guidance".to_owned(), json!(""));
            update_voiceprint(
                root,
                json!({"status":"confirmed","cluster_size":evidence.len(),"evidence_tier":quality.tier}),
            )?;
        }
        OwnerCentroidRebuildOutcome::Unchanged => {
            object.insert("status".to_owned(), json!("unchanged"));
            object.insert("reason".to_owned(), json!("evidence_hash_match"));
            object.insert("override_applied".to_owned(), Value::Bool(false));
            object.insert("next_step".to_owned(), json!("none"));
            object.insert("guidance".to_owned(), json!(""));
        }
        OwnerCentroidRebuildOutcome::Refused { reason } => {
            object.insert("status".to_owned(), json!("rejected_regression"));
            object.insert("reason".to_owned(), json!(reason));
            object.insert("override_applied".to_owned(), Value::Bool(false));
        }
    }
    Ok(value)
}

fn detect_owner_candidate(root: &Path, force: bool) -> Result<Value, String> {
    let principal = admitted_principal_id(root).ok_or_else(|| OWNER_IDENTITY_INVALID.to_owned())?;
    if let Ok(Some(centroid)) = load_owner_centroid(root, &principal) {
        update_voiceprint(
            root,
            json!({"status":"confirmed","cluster_size":centroid.cluster_size,"confirmed_at":Utc::now().to_rfc3339(),"evidence_tier":centroid.evidence_tier.clone().unwrap_or_else(|| "standard".to_owned())}),
        )?;
        return Ok(
            json!({"status":"confirmed","recommendation":"confirmed","cluster_size":centroid.cluster_size,"streams_represented":0,"samples":[],"evidence_tier":centroid.evidence_tier.unwrap_or_else(|| "standard".to_owned())}),
        );
    }
    if force {
        clear_owner_candidate(root).map_err(|error| error.to_string())?;
        update_voiceprint(root, json!({"rejected_at":Value::Null}))?;
    }
    let state = awareness_voiceprint(root);
    if !force && cooldown(&state).is_some() {
        return Ok(
            json!({"status":"no_cluster","reason":"cooldown","segments_checked":0,"segments_available":0,"embeddings_available":0,"recommendation":"no_cluster","manual_tags_count":manual_count(root, Some(&principal)),"can_build_from_tags":manual_count(root, Some(&principal)) >= MIN_STATEMENTS,"days_remaining":cooldown(&state),"next_step":"wait_for_cooldown","guidance":"Wait for the owner voice rejection cooldown before running detection again, or run solstone call speakers detect --force to look now."}),
        );
    }
    if let Ok(Some(candidate)) = load_owner_candidate(root)
        && state.get("status").and_then(Value::as_str) == Some("candidate")
    {
        return Ok(
            json!({"status":"candidate","cluster_size":candidate.cluster_size,"streams_represented":state.get("streams_represented").cloned().unwrap_or_else(|| json!(0)),"recommendation":state.get("recommendation").cloned().unwrap_or_else(|| json!("single_stream")),"samples":state.get("samples").cloned().unwrap_or_else(|| json!([])),"evidence_tier":candidate.evidence_tier}),
        );
    }
    let pool_exists = root.join("awareness/speaker_candidates.json").exists();
    let candidates = CandidateTracker::new(root).candidates();
    let candidate = match select_candidate(pool_exists, candidates, Some(&principal)) {
        Ok(candidate) => candidate,
        Err(reason) => return no_cluster(root, Some(&principal), reason, 0, 0, 0),
    };
    if candidate.n_intervals < MIN_STATEMENTS {
        return candidate_low_quality(
            root,
            Some(&principal),
            CandidateLowQuality {
                reason: "too_few_stmts",
                observed: candidate.n_intervals as f64,
                threshold: MIN_STATEMENTS as f64,
                segments: candidate.n_segments,
                embeddings: candidate.n_intervals,
                tier: "standard",
                bound: STANDARD_P25,
            },
        );
    }
    let expansion = expand_candidate(root, &candidate)?;
    if expansion.rows.is_empty() {
        return no_cluster(
            root,
            Some(&principal),
            "candidate_no_usable_embeddings",
            expansion.checked,
            expansion.available,
            0,
        );
    }
    let quality = candidate_quality(&expansion.rows);
    if let Some(reason) = quality.reason {
        return candidate_low_quality(
            root,
            Some(&principal),
            CandidateLowQuality {
                reason,
                observed: if reason == "median_duration_too_short" {
                    quality.median
                } else {
                    quality.p25
                },
                threshold: if reason == "median_duration_too_short" {
                    MIN_MEDIAN_DURATION
                } else {
                    quality.bound
                },
                segments: expansion.checked,
                embeddings: expansion.rows.len(),
                tier: quality.tier,
                bound: quality.bound,
            },
        );
    }
    let Some(centroid) = mean(
        &expansion
            .rows
            .iter()
            .map(|row| row.embedding.clone())
            .collect::<Vec<_>>(),
    )
    .and_then(|row| solstone_core_entity::normalize_embedding(&row)) else {
        return no_cluster(
            root,
            Some(&principal),
            "candidate_centroid_unusable",
            expansion.checked,
            expansion.available,
            expansion.rows.len(),
        );
    };
    let streams = expansion
        .rows
        .iter()
        .map(|row| row.stream.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let recommendation = if streams > 1 {
        "ready"
    } else {
        "single_stream"
    };
    let samples = candidate_samples(root, &expansion.rows, &centroid);
    let version = Utc::now().to_rfc3339();
    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate(
        root,
        &solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
            centroid,
            cluster_size: expansion.rows.len() as i32,
            threshold: OWNER_THRESHOLD,
            version: version.clone(),
            evidence_tier: quality.tier.to_owned(),
        },
    )
    .map_err(|error| error.to_string())?;
    update_voiceprint(
        root,
        json!({"status":"candidate","cluster_size":expansion.rows.len(),"streams_represented":streams,"recommendation":recommendation,"samples":samples,"detected_at":version,"evidence_tier":quality.tier}),
    )?;
    Ok(
        json!({"status":"candidate","cluster_size":expansion.rows.len(),"streams_represented":streams,"recommendation":recommendation,"samples":samples,"evidence_tier":quality.tier}),
    )
}

fn select_candidate(
    pool_exists: bool,
    candidates: Vec<CandidateProfile>,
    principal: Option<&str>,
) -> Result<CandidateProfile, &'static str> {
    if !pool_exists {
        return Err("pool_missing");
    }
    if candidates.is_empty() {
        return Err("pool_empty");
    }
    candidates
        .into_iter()
        .filter(|candidate| {
            candidate.status != "rejected"
                && (candidate.confirmed_entity.is_none()
                    || candidate.confirmed_entity.as_deref() == principal)
        })
        .max_by(|left, right| {
            left.n_intervals
                .cmp(&right.n_intervals)
                .then_with(|| left.total_duration_s.total_cmp(&right.total_duration_s))
                .then_with(|| right.cand_id.cmp(&left.cand_id))
        })
        .ok_or("no_eligible_candidate")
}

struct CandidateRow {
    embedding: Vec<f32>,
    day: String,
    stream: String,
    segment_key: String,
    source: String,
    sentence_id: i64,
    jsonl_path: std::path::PathBuf,
    layout: SegmentLayout,
}
struct CandidateExpansion {
    rows: Vec<CandidateRow>,
    checked: usize,
    available: usize,
}
fn expand_candidate(
    root: &Path,
    candidate: &CandidateProfile,
) -> Result<CandidateExpansion, String> {
    let mut groups = BTreeMap::<(String, String), Vec<&Value>>::new();
    for source in &candidate.source_segments {
        groups
            .entry((
                source
                    .get("stream_layout")
                    .and_then(Value::as_str)
                    .unwrap_or("named")
                    .to_owned(),
                source
                    .get("stream")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ))
            .or_default()
            .push(source);
    }
    let max = groups.values().map(Vec::len).max().unwrap_or(0);
    let mut rows = Vec::new();
    let mut checked = BTreeSet::new();
    'rounds: for index in 0..max {
        for group in groups.values() {
            let Some(value) = group.get(index) else {
                continue;
            };
            let (Some(day), Some(stream), Some(segment_key), Some(source), Some(label)) = (
                value.get("day").and_then(Value::as_str),
                value.get("stream").and_then(Value::as_str),
                value.get("segment_key").and_then(Value::as_str),
                value.get("source").and_then(Value::as_str),
                value.get("cluster_label").and_then(Value::as_i64),
            ) else {
                return Err("invalid candidate source segment".to_owned());
            };
            let layout = decode_stream_layout_value(value.get("stream_layout"));
            let layout = match layout {
                Ok(layout) => layout,
                Err(_) => {
                    return Err("invalid stream_layout on candidate source segment".to_owned());
                }
            };
            checked.insert((
                day.to_owned(),
                layout_flag(layout),
                stream.to_owned(),
                segment_key.to_owned(),
            ));
            let segment = match lookup_segment(
                root,
                day,
                stream,
                segment_key,
                Ok(layout),
                DirectSupport::Allow,
            ) {
                SegmentLookup::Present(path) => path,
                SegmentLookup::Absent => continue,
                SegmentLookup::MalformedLayout => {
                    return Err("invalid stream_layout on candidate source segment".to_owned());
                }
                SegmentLookup::Failed(error) => return Err(error.to_string()),
                SegmentLookup::UnsupportedLayout => {
                    return Err("segment layout is not readable".to_owned());
                }
            };
            let jsonl = segment.join(format!("{source}.jsonl"));
            if !segment.is_dir() || crate::speakers_quality::segment_overlap_fraction(&jsonl) > 0.10
            {
                continue;
            }
            let embeddings_path = segment.join(format!("{source}.npz"));
            let file = match solstone_core_speaker_id::embeddings::load_embeddings_file(
                &embeddings_path,
            ) {
                Ok(Some(file)) => file,
                Ok(None) => continue,
                Err(error) => {
                    return Err(format!(
                        "failed to load {}: {error}",
                        embeddings_path.display()
                    ));
                }
            };
            let ids = source_sentence_ids(value)
                .ok_or_else(|| "invalid candidate sentence ids".to_owned())?;
            let mut selected = ids
                .into_iter()
                .filter_map(|id| {
                    file.statements
                        .iter()
                        .find(|(current, _)| *current == id)
                        .and_then(|(_, embedding)| {
                            solstone_core_entity::normalize_embedding(embedding).map(|embedding| {
                                CandidateRow {
                                    embedding,
                                    day: day.to_owned(),
                                    stream: stream.to_owned(),
                                    segment_key: segment_key.to_owned(),
                                    source: source.to_owned(),
                                    sentence_id: id,
                                    jsonl_path: jsonl.clone(),
                                    layout,
                                }
                            })
                        })
                })
                .collect::<Vec<_>>();
            if label == -1 {
                let before = selected
                    .iter()
                    .map(|row| row.embedding.clone())
                    .collect::<Vec<_>>();
                let (kept, _, _) = trim_solo_cluster_rows(&before);
                selected.retain(|row| kept.contains(&row.embedding));
            }
            for row in selected {
                if rows.len() >= CANDIDATE_EXPANSION_MAX_EMBEDDINGS {
                    break 'rounds;
                }
                rows.push(row);
            }
        }
    }
    Ok(CandidateExpansion {
        rows,
        checked: checked.len(),
        available: candidate.n_segments,
    })
}
fn source_sentence_ids(value: &Value) -> Option<Vec<i64>> {
    let mut ids = value
        .get("sentence_ids")?
        .as_array()?
        .iter()
        .map(|value| match value {
            Value::Bool(_) => None,
            Value::Number(value) => value
                .as_i64()
                .or_else(|| value.as_f64().map(|number| number as i64)),
            Value::String(value) => value.parse().ok(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    ids.sort_unstable();
    ids.dedup();
    Some(ids)
}
fn candidate_quality(rows: &[CandidateRow]) -> Quality {
    let tier = if rows.len() >= STRONG_STATEMENTS {
        "strong"
    } else {
        "standard"
    };
    let bound = if tier == "strong" {
        STRONG_P25
    } else {
        STANDARD_P25
    };
    let mut durations = rows
        .iter()
        .filter_map(|row| fallback_duration(&row.jsonl_path, row.sentence_id))
        .collect::<Vec<_>>();
    durations.sort_by(f64::total_cmp);
    let median = durations
        .get((durations.len().saturating_sub(1)) / 2)
        .copied()
        .unwrap_or(0.0);
    let p25 = intra_cosine_p25(
        &rows
            .iter()
            .map(|row| row.embedding.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap_or(0.0);
    let reason = if rows.len() < MIN_STATEMENTS {
        Some("too_few_stmts")
    } else if median < MIN_MEDIAN_DURATION {
        Some("median_duration_too_short")
    } else if p25 < bound {
        Some("cluster_too_diffuse")
    } else {
        None
    };
    Quality {
        reason,
        median,
        p25,
        tier,
        bound,
    }
}
fn candidate_samples(_root: &Path, rows: &[CandidateRow], centroid: &[f32]) -> Vec<Value> {
    let mut ordered = rows.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        dot(&right.embedding, centroid).total_cmp(&dot(&left.embedding, centroid))
    });
    let mut seen = BTreeSet::new();
    ordered
        .into_iter()
        .filter(|row| {
            seen.insert((
                row.day.as_str(),
                layout_flag(row.layout),
                row.stream.as_str(),
                row.segment_key.as_str(),
            ))
        })
        .take(3)
        .map(|row| {
            let segment = row
                .jsonl_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_path_buf();
            let url = ["flac", "wav", "m4a", "mp3", "ogg"]
                .iter()
                .map(|ext| segment.join(format!("{}.{}", row.source, ext)))
                .find(|path| path.is_file())
                .map(|path| {
                    serve_audio_url(
                        &row.day,
                        &row.stream,
                        &row.segment_key,
                        &path.file_name().expect("name").to_string_lossy(),
                        row.layout,
                    )
                });
            json!({
                "day": row.day,
                "stream_layout": layout_flag(row.layout),
                "stream": row.stream,
                "segment_key": row.segment_key,
                "source": row.source,
                "sentence_id": row.sentence_id,
                "duration_s": fallback_duration(&row.jsonl_path, row.sentence_id),
                "audio_url": url
            })
        })
        .collect()
}

fn layout_flag(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}

fn encode_path_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn serve_audio_url(
    day: &str,
    stream: &str,
    segment_key: &str,
    filename: &str,
    layout: SegmentLayout,
) -> String {
    let day = encode_path_component(day);
    let stream = encode_path_component(stream);
    let segment_key = encode_path_component(segment_key);
    let filename = encode_path_component(filename);
    match layout {
        SegmentLayout::Direct => {
            format!("/app/speakers/api/serve_audio/{day}/{segment_key}/{filename}")
        }
        SegmentLayout::Named => {
            format!("/app/speakers/api/serve_audio/{day}/{stream}/{segment_key}/{filename}")
        }
    }
}
fn fallback_duration(path: &Path, sentence_id: i64) -> Option<f64> {
    let starts = fs::read_to_string(path)
        .ok()?
        .lines()
        .skip(1)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            value
                .get("start")
                .and_then(Value::as_str)
                .and_then(parse_time)
        })
        .collect::<Vec<_>>();
    let index = usize::try_from(sentence_id.checked_sub(1)?).ok()?;
    Some((starts.get(index + 1)? - starts.get(index)?) as f64)
}
fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
struct CandidateLowQuality<'a> {
    reason: &'a str,
    observed: f64,
    threshold: f64,
    segments: usize,
    embeddings: usize,
    tier: &'a str,
    bound: f64,
}
fn candidate_low_quality(
    root: &Path,
    principal: Option<&str>,
    value: CandidateLowQuality<'_>,
) -> Result<Value, String> {
    update_voiceprint(
        root,
        json!({"status":"low_quality","source":"candidate_pool","low_quality_reason":value.reason,"observed_value":value.observed,"threshold_value":value.threshold,"evidence_tier":value.tier,"intra_cosine_p25_bound":value.bound,"segments_checked":value.segments,"attempted_at":Utc::now().to_rfc3339()}),
    )?;
    let count = manual_count(root, principal);
    Ok(
        json!({"status":"low_quality","source":"candidate_pool","recommendation":"low_quality","segments_available":value.segments,"embeddings_available":value.embeddings,"low_quality_reason":value.reason,"observed_value":value.observed,"threshold_value":value.threshold,"evidence_tier":value.tier,"intra_cosine_p25_bound":value.bound,"manual_tags_count":count,"can_build_from_tags":count>=MIN_STATEMENTS,"next_step":"seed_manual_tags","guidance":"Use solstone call speakers tag-owner to add validated owner tags."}),
    )
}
fn no_cluster(
    root: &Path,
    principal: Option<&str>,
    reason: &str,
    checked: usize,
    segments: usize,
    embeddings: usize,
) -> Result<Value, String> {
    update_voiceprint(root, json!({"status":"no_cluster","reason":reason}))?;
    let count = manual_count(root, principal);
    Ok(
        json!({"status":"no_cluster","reason":reason,"segments_checked":checked,"segments_available":segments,"embeddings_available":embeddings,"recommendation":"no_cluster","manual_tags_count":count,"can_build_from_tags":count>=MIN_STATEMENTS,"next_step":"seed_manual_tags","guidance":"Use solstone call speakers tag-owner to add validated owner tags."}),
    )
}

fn owner_detection_ready(root: &Path) -> Result<Value, ()> {
    let principal = admitted_principal_id(root).ok_or(())?;
    if load_owner_centroid(root, &principal)
        .ok()
        .flatten()
        .is_some()
    {
        return Ok(json!({"ready":false,"reason":"centroid_exists"}));
    }
    let state = awareness_voiceprint(root);
    if let Some(days_remaining) = cooldown(&state) {
        return Ok(json!({"ready":false,"reason":"cooldown","days_remaining":days_remaining}));
    }
    if root.join("awareness/owner_candidate.npz").exists()
        && state.get("status").and_then(Value::as_str) == Some("candidate")
    {
        let recommendation = state
            .get("recommendation")
            .and_then(Value::as_str)
            .unwrap_or("single_stream");
        return Ok(
            json!({"ready":recommendation == "ready","reason":if recommendation == "ready" { "candidate_found" } else { recommendation },"candidate_available":true,"recommendation":recommendation,"cluster_size":state.get("cluster_size").cloned().unwrap_or_else(|| json!(0)),"streams_represented":state.get("streams_represented").cloned().unwrap_or_else(|| json!(0)),"evidence_tier":state.get("evidence_tier").cloned().unwrap_or_else(|| json!("standard"))}),
        );
    }
    Ok(json!({"ready":false,"reason":"no_candidate"}))
}

struct Quality {
    reason: Option<&'static str>,
    median: f64,
    p25: f64,
    tier: &'static str,
    bound: f64,
}
fn quality(
    evidence: &[solstone_core_speaker_resolve::owner_provisional::ManualOwnerEmbedding],
) -> Quality {
    let tier = if evidence.len() >= STRONG_STATEMENTS {
        "strong"
    } else {
        "standard"
    };
    let bound = if tier == "strong" {
        STRONG_P25
    } else {
        STANDARD_P25
    };
    let mut durations = evidence.iter().filter_map(duration).collect::<Vec<_>>();
    durations.sort_by(f64::total_cmp);
    let median = if durations.is_empty() {
        0.0
    } else {
        durations[(durations.len() - 1) / 2]
    };
    let rows = evidence
        .iter()
        .map(|item| item.embedding.clone())
        .collect::<Vec<_>>();
    let p25 = intra_cosine_p25(&rows).unwrap_or(0.0);
    let reason = if evidence.len() < MIN_STATEMENTS {
        Some("too_few_stmts")
    } else if median < MIN_MEDIAN_DURATION {
        Some("median_duration_too_short")
    } else if p25 < bound {
        Some("cluster_too_diffuse")
    } else {
        None
    };
    Quality {
        reason,
        median,
        p25,
        tier,
        bound,
    }
}
fn low_quality(
    reason: &str,
    quality: &Quality,
    evidence: &[solstone_core_speaker_resolve::owner_provisional::ManualOwnerEmbedding],
    source: &str,
) -> Value {
    let observed = match reason {
        "too_few_stmts" => evidence.len() as f64,
        "median_duration_too_short" => quality.median,
        _ => quality.p25,
    };
    let threshold = match reason {
        "too_few_stmts" => MIN_STATEMENTS as f64,
        "median_duration_too_short" => MIN_MEDIAN_DURATION,
        _ => quality.bound,
    };
    json!({"status":"low_quality","reason":reason,"observed_value":observed,"threshold_value":threshold,"segment_count":evidence.iter().map(|item| (&item.day,&item.stream,&item.segment_key)).collect::<BTreeSet<_>>().len(),"embeddings_count":evidence.len(),"source":source,"evidence_tier":quality.tier,"intra_cosine_p25_bound":quality.bound,"next_step":"seed_manual_tags","guidance":"Add more validated owner tags and try again."})
}
fn rebuild_refusal(reason: &str) -> Value {
    json!({"status":"refused","reason":reason,"next_step":"build_from_tags","guidance":"Build or confirm an owner voice before running rebuild-owner."})
}
fn mean(rows: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = rows.first()?;
    let mut mean = vec![0.0; first.len()];
    for row in rows {
        for (out, value) in mean.iter_mut().zip(row) {
            *out += *value;
        }
    }
    for value in &mut mean {
        *value /= rows.len() as f32;
    }
    Some(mean)
}
fn duration(
    item: &solstone_core_speaker_resolve::owner_provisional::ManualOwnerEmbedding,
) -> Option<f64> {
    let rows = fs::read_to_string(&item.jsonl_path).ok()?;
    let starts = rows
        .lines()
        .skip(1)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|row| {
            row.get("start")
                .and_then(Value::as_str)
                .map(|start| (row.get("id").and_then(Value::as_i64), parse_time(start)))
        })
        .collect::<Vec<_>>();
    let index = starts
        .iter()
        .position(|(id, _)| *id == Some(item.sentence_id))?;
    let start = starts[index].1?;
    starts.get(index + 1)?.1.map(|next| (next - start) as f64)
}
fn parse_time(value: &str) -> Option<i64> {
    let mut parts = value.split(':').map(|part| part.parse::<i64>().ok());
    Some(parts.next()?? * 3600 + parts.next()?? * 60 + parts.next()??)
}
fn evidence_hash(
    evidence: &[solstone_core_speaker_resolve::owner_provisional::ManualOwnerEmbedding],
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"owner-rebuild-evidence-v1\n");
    for item in evidence {
        hasher.update(format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\n",
            item.day, item.stream, item.segment_key, item.source, item.sentence_id
        ));
    }
    format!("{:x}", hasher.finalize())
}
fn admitted_principal_id(root: &Path) -> Option<String> {
    match admitted_owner_id(root) {
        OwnerAdmission::Admitted(id) => Some(id),
        OwnerAdmission::Invalid => None,
    }
}
fn identity_invalid_value() -> Value {
    json!({"reason_code":OWNER_IDENTITY_INVALID})
}
fn owner_identity_invalid() -> Response {
    err(
        OWNER_IDENTITY_INVALID,
        OWNER_IDENTITY_INVALID_MESSAGE,
        OWNER_IDENTITY_INVALID_DETAIL,
        StatusCode::BAD_REQUEST,
    )
}
fn manual_count(root: &Path, principal: Option<&str>) -> usize {
    principal
        .map(|id| crate::speakers_quality::count_manual_owner_tags(root, id))
        .unwrap_or_default()
}
fn cooldown(state: &BTreeMap<String, Value>) -> Option<i64> {
    let time = state.get("rejected_at")?.as_str()?;
    let date = DateTime::parse_from_rfc3339(time).ok()?;
    let elapsed = Utc::now()
        .signed_duration_since(date.with_timezone(&Utc))
        .num_days();
    (elapsed < COOLDOWN_DAYS).then_some(COOLDOWN_DAYS - elapsed)
}
fn update_voiceprint(root: &Path, update: Value) -> Result<(), String> {
    let path = root.join("awareness/current.json");
    let _lock = hold_lock(&path, LockOptions::default()).map_err(|error| error.to_string())?;
    let mut state = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let voiceprint = state
        .entry("voiceprint".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let object = voiceprint
        .as_object_mut()
        .ok_or_else(|| "awareness voiceprint state is not an object".to_owned())?;
    object.extend(update.as_object().cloned().unwrap_or_default());
    write_json(path, &Value::Object(state), JsonWriteOptions::default())
        .map_err(|error| error.to_string())
}
async fn optional_json(request: Request) -> Value {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
}
async fn required_json(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| missing_body())?;
    if bytes.is_empty() {
        return Err(missing_body());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        err(
            "invalid_json_request",
            "I couldn't read that JSON request.",
            "request body must be a JSON object",
            StatusCode::BAD_REQUEST,
        )
    })?;
    (!value.is_null()).then_some(value).ok_or_else(missing_body)
}
fn valid_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn missing_fields() -> Response {
    err(
        "missing_required_field",
        "I couldn't find a required field.",
        "Missing required fields",
        StatusCode::BAD_REQUEST,
    )
}
fn missing_body() -> Response {
    err(
        "missing_request_body",
        "I couldn't find any data in that request.",
        "No data provided",
        StatusCode::BAD_REQUEST,
    )
}
fn err(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}
fn owner_error(detail: String) -> Response {
    if detail == OWNER_IDENTITY_INVALID {
        return owner_identity_invalid();
    }
    if detail.contains("busy") || detail.contains("lock") {
        err(
            "speaker_voiceprint_busy",
            "I couldn't update that voice right now because it was busy. Try again in a moment.",
            &detail,
            StatusCode::SERVICE_UNAVAILABLE,
        )
    } else {
        err(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            &detail,
            StatusCode::BAD_REQUEST,
        )
    }
}
