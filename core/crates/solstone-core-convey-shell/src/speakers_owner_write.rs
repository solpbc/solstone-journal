// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native mutations for the owner-voice workflow.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_speaker_resolve::owner_admission::{OwnerAdmission, admitted_owner_id};
use solstone_core_speaker_resolve::owner_candidate::{
    clear_owner_candidate_in_lock, hold_owner_candidate_lock, hydrate_owner_candidate_samples,
    load_owner_candidate,
};
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidRebuildInput, OwnerCentroidRebuildOutcome, OwnerCentroidWriteInput,
    load_owner_centroid, rebuild_owner_centroid, write_owner_centroid,
};
use solstone_core_speaker_resolve::owner_detection::{
    MIN_MEDIAN_DURATION, MIN_STATEMENTS, OWNER_IDENTITY_INVALID, Quality, detect_owner_candidate,
    evidence_quality, mean, parse_time,
};
use solstone_core_speaker_resolve::owner_provisional::collect_manual_owner_embeddings;
use solstone_core_speaker_resolve::owner_voice::{
    OwnerVoiceError, cooldown_days_remaining, record_owner_voice_saved, reject_owner_candidate,
    undo_owner_voice, update_voiceprint_state, voiceprint_state,
};

use crate::JournalRoot;
use crate::speakers_attribution::action;
use crate::speakers_source::is_safe_source_component;
use solstone_core_speaker_resolve::segment_catalog::{
    SegmentLookup, decode_stream_layout_value, lookup_segment,
};

const EXISTING_CENTROID_GUIDANCE: &str = "Owner centroid already exists. Run solstone call speakers rebuild-owner to refresh it from current manual tags.";
const OWNER_IDENTITY_INVALID_MESSAGE: &str =
    "that speaker command couldn't run because your configured owner identity needs attention.";
const OWNER_IDENTITY_INVALID_DETAIL: &str = "configured owner identity is not admitted";

pub(crate) async fn detect(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = optional_json(request).await;
    let force = body.get("force") == Some(&Value::Bool(true));
    match detect_owner_candidate(&root.0, force) {
        Ok(mut value) => {
            if matches!(value["status"].as_str(), Some("no_cluster" | "low_quality")) {
                let count = match manual_count(&root.0, admitted_principal_id(&root.0).as_deref()) {
                    Ok(count) => count,
                    Err(error) => {
                        return err(
                            "speaker_command_failed",
                            "that speaker command didn't finish.",
                            &error.to_string(),
                            StatusCode::INTERNAL_SERVER_ERROR,
                        );
                    }
                };
                value["manual_tags_count"] = json!(count);
                value["can_build_from_tags"] = json!(count >= MIN_STATEMENTS);
            }
            Json(value).into_response()
        }
        Err(error) => owner_error(error),
    }
}

pub(crate) async fn build_from_tags(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match bootstrap_owner_from_manual_tags(&root.0) {
        Ok(value) if value["reason_code"] == OWNER_IDENTITY_INVALID => owner_identity_invalid(),
        Ok(value) if value.get("error").is_some() => err(
            "entity_not_found",
            "that person couldn't be found.",
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

pub(crate) async fn rebuild(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
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

pub(crate) async fn confirm(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match required_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(principal_id) = admitted_principal_id(&root.0) else {
        return owner_identity_invalid();
    };

    test_hooks::wait_if_hold_confirm_or_reject();

    let _lock = match hold_owner_candidate_lock(&root.0) {
        Ok(guard) => guard,
        Err(error) => return owner_error(error.to_string()),
    };

    let candidate = match load_owner_candidate(&root.0) {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            return err(
                "speaker_review_unavailable",
                "that speaker review couldn't be loaded.",
                "No candidate available",
                StatusCode::NOT_FOUND,
            );
        }
        Err(error) => return owner_error(error.to_string()),
    };
    let awareness = voiceprint_state(&root.0);
    if awareness.get("status").and_then(Value::as_str) != Some("candidate") {
        return err(
            "speaker_review_unavailable",
            "that speaker review couldn't be loaded.",
            "Awareness state is not candidate",
            StatusCode::NOT_FOUND,
        );
    }

    let req_version = body.get("version").and_then(Value::as_str);
    if req_version != Some(&candidate.version)
        || awareness.get("detected_at").and_then(Value::as_str) != Some(&candidate.version)
    {
        return err(
            "speaker_candidate_stale_version",
            "that speaker candidate is no longer current.",
            "Candidate version mismatch or stale generation",
            StatusCode::CONFLICT,
        );
    }

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
    if !is_safe_source_component(source) {
        return err(
            "invalid_request_value",
            "source name is invalid.",
            "use one file name, without a path",
            StatusCode::BAD_REQUEST,
        );
    }
    let Some(sentence_id) = body.get("sentence_id").and_then(Value::as_i64) else {
        return missing_fields();
    };
    let Some(audio_sha256) = body.get("audio_sha256").and_then(Value::as_str) else {
        return missing_fields();
    };
    if audio_sha256.is_empty() {
        return err(
            "speaker_candidate_unverified",
            "that speaker sample has unverified audio evidence.",
            "audio_sha256 must be a non-empty hex hash",
            StatusCode::BAD_REQUEST,
        );
    }

    let awareness_samples = awareness.get("samples").and_then(Value::as_array);
    let sample_match = awareness_samples.and_then(|samples| {
        samples.iter().find(|s| {
            s.get("day").and_then(Value::as_str) == Some(day)
                && s.get("stream").and_then(Value::as_str) == Some(stream)
                && s.get("segment_key").and_then(Value::as_str) == Some(segment_key)
                && s.get("source").and_then(Value::as_str) == Some(source)
                && s.get("sentence_id").and_then(Value::as_i64) == Some(sentence_id)
        })
    });

    let Some(sample_row) = sample_match else {
        return err(
            "speaker_candidate_unverified",
            "that speaker sample is not part of the active candidate.",
            "Sample not found in active candidate samples",
            StatusCode::BAD_REQUEST,
        );
    };

    let sample_version = sample_row.get("version").and_then(Value::as_str);
    let sample_embedding_sha256 = sample_row.get("embedding_sha256").and_then(Value::as_str);
    let sample_audio_sha256 = sample_row.get("audio_sha256").and_then(Value::as_str);

    if sample_version != Some(&candidate.version)
        || sample_embedding_sha256.is_none()
        || sample_audio_sha256 != Some(audio_sha256)
    {
        return err(
            "speaker_candidate_unverified",
            "that speaker sample does not have valid evidentiary hashes.",
            "Sample hash verification failed",
            StatusCode::BAD_REQUEST,
        );
    }

    let hydrated =
        hydrate_owner_candidate_samples(&root.0, &candidate, std::slice::from_ref(sample_row));
    if hydrated.is_empty() || hydrated[0].get("artifact_eligible") != Some(&Value::Bool(true)) {
        let reason = hydrated
            .first()
            .and_then(|h| h.get("evidence_reason"))
            .and_then(Value::as_str)
            .unwrap_or("evidence_not_eligible");
        return err(
            "speaker_candidate_unverified",
            "that speaker sample could not be verified against on-disk evidence.",
            reason,
            StatusCode::BAD_REQUEST,
        );
    }

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
    if let Err(error) = clear_owner_candidate_in_lock(&root.0) {
        return owner_error(error.to_string());
    }
    let relabel = match record_owner_voice_saved(
        &root.0,
        "confirm",
        candidate.cluster_size.max(0) as usize,
        &candidate.evidence_tier,
    ) {
        Ok(relabel) => relabel,
        Err(error) => return owner_error(error),
    };
    if let Err(error) = action(
        &root.0,
        "owner_voiceprint_confirm",
        json!({"principal_id":principal_id,"cluster_size":candidate.cluster_size}),
    ) {
        return owner_error(error);
    }
    Json(json!({"status":"confirmed","principal_id":principal_id,"relabel":relabel}))
        .into_response()
}

pub(crate) async fn reject(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match required_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    test_hooks::wait_if_hold_confirm_or_reject();
    let version = body
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match reject_owner_candidate(&root.0, version) {
        Ok(value) => Json(value).into_response(),
        Err(error) => owner_voice_error(error),
    }
}

pub(crate) async fn undo(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match undo_owner_voice(&root.0) {
        Ok(value) => {
            if let Err(error) = action(&root.0, "owner_voiceprint_undo", json!({})) {
                return owner_error(error);
            }
            Json(value).into_response()
        }
        Err(error) => owner_voice_error(error),
    }
}

/// Map a shared owner-voice failure onto the owner-write error envelope.
pub(crate) fn owner_voice_error(error: OwnerVoiceError) -> Response {
    match error {
        OwnerVoiceError::IdentityInvalid => owner_identity_invalid(),
        OwnerVoiceError::NoCandidate => err(
            "speaker_review_unavailable",
            "that speaker review couldn't be loaded.",
            "No candidate available",
            StatusCode::NOT_FOUND,
        ),
        OwnerVoiceError::StaleCandidate => err(
            "speaker_candidate_stale_version",
            "that speaker candidate is no longer current.",
            "Candidate version mismatch or stale generation",
            StatusCode::CONFLICT,
        ),
        OwnerVoiceError::NoSavedVoice => err(
            "speaker_owner_voice_missing",
            "there's no saved voice to undo.",
            "No owner voice is saved",
            StatusCode::NOT_FOUND,
        ),
        OwnerVoiceError::Failed(detail) => owner_error(detail),
    }
}

pub(crate) async fn set_aside(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match required_json(request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if admitted_principal_id(&root.0).is_none() {
        return owner_identity_invalid();
    }

    test_hooks::wait_if_hold_confirm_or_reject();

    let _lock = match hold_owner_candidate_lock(&root.0) {
        Ok(guard) => guard,
        Err(error) => return owner_error(error.to_string()),
    };

    let candidate = match load_owner_candidate(&root.0) {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            return err(
                "speaker_review_unavailable",
                "that speaker review couldn't be loaded.",
                "No candidate available",
                StatusCode::NOT_FOUND,
            );
        }
        Err(error) => return owner_error(error.to_string()),
    };
    let awareness = voiceprint_state(&root.0);

    let req_version = body.get("version").and_then(Value::as_str);
    if req_version != Some(&candidate.version)
        || awareness.get("detected_at").and_then(Value::as_str) != Some(&candidate.version)
    {
        return err(
            "speaker_candidate_stale_version",
            "that speaker candidate is no longer current.",
            "Candidate version mismatch or stale generation",
            StatusCode::CONFLICT,
        );
    }

    if let Err(error) = clear_owner_candidate_in_lock(&root.0) {
        return owner_error(error.to_string());
    }
    if let Err(error) = update_voiceprint_state(
        &root.0,
        json!({"status":"no_cluster","detected_at":Value::Null,"samples":[]}),
    ) {
        return owner_error(error);
    }
    Json(json!({"status":"no_cluster"})).into_response()
}

pub(crate) async fn classify(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
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
    if !is_safe_source_component(source) {
        return err(
            "invalid_request_value",
            "source name is invalid.",
            "use one file name, without a path",
            StatusCode::BAD_REQUEST,
        );
    }
    let Some(principal) = admitted_principal_id(&root.0) else {
        return owner_identity_invalid();
    };
    if !valid_day(day) {
        return err(
            "invalid_day",
            "that day couldn't be used.",
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
    ) {
        SegmentLookup::Present(path) => path,
        SegmentLookup::Absent => return Json(json!({"sentences":[]})).into_response(),
        SegmentLookup::MalformedLayout => {
            return err(
                "invalid_segment_or_stream",
                "that segment or stream couldn't be used.",
                "Invalid segment key",
                StatusCode::BAD_REQUEST,
            );
        }
        SegmentLookup::Failed(error) => {
            return err(
                "speaker_command_failed",
                "that speaker command didn't finish.",
                &error.to_string(),
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

pub(crate) async fn ready(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
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
    let relabel = record_owner_voice_saved(root, "teach", evidence.len(), quality.tier)?;
    Ok(
        json!({"status":"confirmed","principal_id":principal_id,"cluster_size":evidence.len(),"evidence_tier":quality.tier,"relabel":relabel}),
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
            let relabel = record_owner_voice_saved(root, "redo", evidence.len(), quality.tier)?;
            object.insert("relabel".to_owned(), relabel);
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

fn owner_detection_ready(root: &Path) -> Result<Value, ()> {
    let principal = admitted_principal_id(root).ok_or(())?;
    if load_owner_centroid(root, &principal)
        .ok()
        .flatten()
        .is_some()
    {
        return Ok(json!({"ready":false,"reason":"centroid_exists"}));
    }
    let state = voiceprint_state(root);
    if let Some(days_remaining) = cooldown_days_remaining(&state) {
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

fn quality(
    evidence: &[solstone_core_speaker_resolve::owner_provisional::ManualOwnerEmbedding],
) -> Quality {
    evidence_quality(
        &evidence
            .iter()
            .map(|item| item.embedding.clone())
            .collect::<Vec<_>>(),
        evidence.iter().filter_map(duration).collect(),
    )
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
fn duration(
    item: &solstone_core_speaker_resolve::owner_provisional::ManualOwnerEmbedding,
) -> Option<f64> {
    let read = solstone_core_speaker_id::transcript::read_transcript_rows(
        &fs::read(&item.jsonl_path).ok()?,
    )
    .ok()?;
    let starts = read
        .rows
        .iter()
        .filter_map(|row| {
            row.value
                .get("start")
                .and_then(Value::as_str)
                .map(|start| (row.sentence_id, parse_time(start)))
        })
        .collect::<Vec<_>>();
    let index = starts.iter().position(|(id, _)| *id == item.sentence_id)?;
    let start = starts[index].1?;
    starts.get(index + 1)?.1.map(|next| (next - start) as f64)
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
        StatusCode::CONFLICT,
    )
}
fn manual_count(
    root: &Path,
    principal: Option<&str>,
) -> Result<usize, solstone_core_journal_io::zip2_name::Zip2NameFailure> {
    principal
        .map(|id| crate::speakers_quality::count_manual_owner_tags(root, id))
        .transpose()
        .map(|count| count.unwrap_or_default())
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
            "that JSON request couldn't be read.",
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
        "a required field is missing.",
        "Missing required fields",
        StatusCode::BAD_REQUEST,
    )
}
fn missing_body() -> Response {
    err(
        "missing_request_body",
        "that request had no data in it.",
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
            "that voice couldn't be updated right now because it was busy. try again in a moment.",
            &detail,
            StatusCode::SERVICE_UNAVAILABLE,
        )
    } else {
        err(
            "speaker_command_failed",
            "that speaker command didn't finish.",
            &detail,
            StatusCode::BAD_REQUEST,
        )
    }
}

#[cfg(feature = "test-hooks")]
pub mod test_hooks {
    use std::sync::{Condvar, Mutex, OnceLock};

    static HOOKS: OnceLock<Hooks> = OnceLock::new();

    struct Hooks {
        pause_after_snapshot: Mutex<bool>,
        pause_after_snapshot_cv: Condvar,
        pause_after_snapshot_entered: Mutex<usize>,
        pause_after_snapshot_entered_cv: Condvar,
        hold_confirm_or_reject: Mutex<bool>,
        hold_confirm_or_reject_cv: Condvar,
        hold_confirm_or_reject_entered: Mutex<usize>,
        hold_confirm_or_reject_entered_cv: Condvar,
    }

    impl Hooks {
        fn get() -> &'static Self {
            HOOKS.get_or_init(|| Hooks {
                pause_after_snapshot: Mutex::new(false),
                pause_after_snapshot_cv: Condvar::new(),
                pause_after_snapshot_entered: Mutex::new(0),
                pause_after_snapshot_entered_cv: Condvar::new(),
                hold_confirm_or_reject: Mutex::new(false),
                hold_confirm_or_reject_cv: Condvar::new(),
                hold_confirm_or_reject_entered: Mutex::new(0),
                hold_confirm_or_reject_entered_cv: Condvar::new(),
            })
        }
    }

    pub fn reset() {
        let hooks = Hooks::get();
        *hooks.pause_after_snapshot.lock().unwrap() = false;
        *hooks.pause_after_snapshot_entered.lock().unwrap() = 0;
        hooks.pause_after_snapshot_cv.notify_all();
        hooks.pause_after_snapshot_entered_cv.notify_all();
        *hooks.hold_confirm_or_reject.lock().unwrap() = false;
        *hooks.hold_confirm_or_reject_entered.lock().unwrap() = 0;
        hooks.hold_confirm_or_reject_cv.notify_all();
        hooks.hold_confirm_or_reject_entered_cv.notify_all();
    }

    pub fn set_pause_after_snapshot(enable: bool) {
        let hooks = Hooks::get();
        let mut guard = hooks.pause_after_snapshot.lock().unwrap();
        *guard = enable;
        if !enable {
            hooks.pause_after_snapshot_cv.notify_all();
        }
    }

    pub fn entered_pause_after_snapshot() -> usize {
        let hooks = Hooks::get();
        *hooks.pause_after_snapshot_entered.lock().unwrap()
    }

    pub fn wait_entered_pause_after_snapshot(expected: usize) {
        let hooks = Hooks::get();
        let mut guard = hooks.pause_after_snapshot_entered.lock().unwrap();
        while *guard < expected {
            guard = hooks.pause_after_snapshot_entered_cv.wait(guard).unwrap();
        }
    }

    pub fn wait_if_pause_after_snapshot() {
        let hooks = Hooks::get();
        {
            let mut entered = hooks.pause_after_snapshot_entered.lock().unwrap();
            *entered += 1;
            hooks.pause_after_snapshot_entered_cv.notify_all();
        }
        let mut guard = hooks.pause_after_snapshot.lock().unwrap();
        while *guard {
            guard = hooks.pause_after_snapshot_cv.wait(guard).unwrap();
        }
    }

    pub fn set_hold_confirm_or_reject(enable: bool) {
        let hooks = Hooks::get();
        let mut guard = hooks.hold_confirm_or_reject.lock().unwrap();
        *guard = enable;
        if !enable {
            hooks.hold_confirm_or_reject_cv.notify_all();
        }
    }

    pub fn entered_hold_confirm_or_reject() -> usize {
        let hooks = Hooks::get();
        *hooks.hold_confirm_or_reject_entered.lock().unwrap()
    }

    pub fn wait_entered_hold_confirm_or_reject(expected: usize) {
        let hooks = Hooks::get();
        let mut guard = hooks.hold_confirm_or_reject_entered.lock().unwrap();
        while *guard < expected {
            guard = hooks.hold_confirm_or_reject_entered_cv.wait(guard).unwrap();
        }
    }

    pub fn wait_if_hold_confirm_or_reject() {
        let hooks = Hooks::get();
        {
            let mut entered = hooks.hold_confirm_or_reject_entered.lock().unwrap();
            *entered += 1;
            hooks.hold_confirm_or_reject_entered_cv.notify_all();
        }
        let mut guard = hooks.hold_confirm_or_reject.lock().unwrap();
        while *guard {
            guard = hooks.hold_confirm_or_reject_cv.wait(guard).unwrap();
        }
    }
}

#[cfg(not(feature = "test-hooks"))]
pub mod test_hooks {
    #[inline(always)]
    pub fn wait_if_hold_confirm_or_reject() {}
}
