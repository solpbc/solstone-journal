// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI owner-decision and manual-tag routes.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::SegmentLayout;

use crate::JournalRoot;
use crate::speakers_attribution::entity_allowed;
use crate::speakers_segment_catalog::{
    DirectSupport, SegmentLookup, UNSUPPORTED_LAYOUT_DETAIL, UNSUPPORTED_LAYOUT_MESSAGE,
    UNSUPPORTED_LAYOUT_REASON, decode_stream_layout_value, lookup_segment,
};

pub async fn tag(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(day) = body.get("day").and_then(Value::as_str) else {
        return required("day");
    };
    let Some(stream) = body.get("stream").and_then(Value::as_str) else {
        return required("stream");
    };
    let Some(segment_key) = body.get("segment").and_then(Value::as_str) else {
        return required("segment");
    };
    let Some(sentence_id) = body.get("sentence_id").and_then(Value::as_i64) else {
        return required("sentence_id");
    };
    let Some(speaker) = body.get("speaker").and_then(Value::as_str) else {
        return required("speaker");
    };
    let Some(source) = body.get("source").and_then(Value::as_str) else {
        return required("source");
    };
    let layout = decode_stream_layout_value(body.get("stream_layout"));
    let segment = match lookup_segment(
        &root.0,
        day,
        stream,
        segment_key,
        layout,
        DirectSupport::Refuse,
    ) {
        SegmentLookup::Present(path) => path,
        SegmentLookup::UnsupportedLayout => {
            return err(
                UNSUPPORTED_LAYOUT_REASON,
                UNSUPPORTED_LAYOUT_MESSAGE,
                UNSUPPORTED_LAYOUT_DETAIL,
                StatusCode::BAD_REQUEST,
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
        SegmentLookup::Absent => {
            return err(
                "speaker_review_unavailable",
                "I couldn't load that speaker review.",
                "No speaker labels found",
                StatusCode::NOT_FOUND,
            );
        }
        SegmentLookup::Failed(error) => return speaker_write_error(error.to_string(), true),
    };
    let labels = match std::fs::read(segment.join("talents/speaker_labels.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
    {
        Some(labels) => labels,
        None => {
            return err(
                "speaker_review_unavailable",
                "I couldn't load that speaker review.",
                "No speaker labels found",
                StatusCode::NOT_FOUND,
            );
        }
    };
    let current = labels
        .iter()
        .find(|label| label.get("sentence_id").and_then(Value::as_i64) == Some(sentence_id));
    if let Err(response) = entity_allowed(&root.0, speaker) {
        return response;
    }
    if current.is_some_and(|label| {
        label.get("speaker").and_then(Value::as_str) == Some(speaker)
            && label.get("method").and_then(Value::as_str) == Some("user_assigned")
    }) {
        return Json(json!({"success":true,"status":"already_assigned","owner_bootstrap_outcome":Value::Null})).into_response();
    }
    if current
        .and_then(|label| label.get("speaker").and_then(Value::as_str))
        .is_some()
    {
        return err(
            "speaker_attribution_state_invalid",
            "I couldn't change that speaker attribution.",
            "Pick a sentence without a speaker.",
            StatusCode::CONFLICT,
        );
    }
    let embedding_path = segment.join(format!("{source}.npz"));
    let embedding =
        match solstone_core_speaker_id::embeddings::load_embeddings_file(&embedding_path) {
            Ok(Some(file)) => file
                .statements
                .into_iter()
                .find_map(|(id, values)| (id == sentence_id).then_some(values)),
            Ok(None) => None,
            Err(error) => return speaker_write_error(error.to_string(), false),
        };
    let Some(embedding) = embedding else {
        return err(
            "speaker_sentence_missing",
            "I couldn't find that sentence.",
            "Pick a different sentence with an embedding.",
            StatusCode::NOT_FOUND,
        );
    };
    let metadata = json!({"day":day,"stream_layout":layout_name(layout.expect("successful lookup decoded layout")),"segment_key":segment_key,"source":source,"sentence_id":sentence_id,"stream":stream});
    if let Err(error) = solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
        &root.0,
        speaker,
        embedding,
        metadata,
        &encoder(),
    ) {
        return speaker_write_error(error.to_string(), false);
    }
    let old_method = current.and_then(|label| {
        label
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let mut patch = Map::new();
    patch.insert("speaker".to_owned(), json!(speaker));
    patch.insert("confidence".to_owned(), json!("high"));
    patch.insert("method".to_owned(), json!("user_assigned"));
    if let Err(error) =
        solstone_core_speaker_id::labels::patch_labels(&segment, &[(sentence_id, patch)], true)
    {
        return speaker_write_error(error.to_string(), true);
    }
    let mut correction = Map::new();
    correction.insert("sentence_id".to_owned(), json!(sentence_id));
    correction.insert("original_speaker".to_owned(), Value::Null);
    correction.insert("corrected_speaker".to_owned(), json!(speaker));
    correction.insert(
        "original_method".to_owned(),
        old_method.map_or(Value::Null, Value::String),
    );
    correction.insert("timestamp".to_owned(), json!(Utc::now().timestamp_millis()));
    if let Err(error) =
        solstone_core_speaker_id::corrections::append_correction(&segment, correction)
    {
        return speaker_write_error(error.to_string(), true);
    }
    Json(json!({"success":true,"status":"assigned","speaker":speaker})).into_response()
}

fn layout_name(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}

fn encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

pub async fn confirm(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let candidate =
        match solstone_core_speaker_resolve::owner_candidate::load_owner_candidate(&root.0) {
            Ok(Some(candidate)) => candidate,
            Ok(None) => {
                return err(
                    "speaker_command_failed",
                    "I couldn't finish that speaker command.",
                    "No candidate available",
                    StatusCode::BAD_REQUEST,
                );
            }
            Err(error) => return owner_error(error.to_string()),
        };
    let principal = match solstone_core_entity::read_journal_principal(&root.0) {
        Ok(Some(principal)) => principal,
        _ => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                "No principal entity found",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let Some(principal_id) = principal.get("id").and_then(Value::as_str) else {
        return err(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            "No principal entity found",
            StatusCode::BAD_REQUEST,
        );
    };
    let input = solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
        centroid: candidate.centroid,
        cluster_size: candidate.cluster_size,
        timestamp: Utc::now().to_rfc3339(),
        evidence_tier: candidate.evidence_tier.clone(),
    };
    if let Err(error) = solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
        &root.0,
        principal_id,
        &input,
    ) {
        return owner_error(error.to_string());
    }
    if let Err(error) =
        solstone_core_speaker_resolve::owner_candidate::clear_owner_candidate(&root.0)
    {
        return owner_error(error.to_string());
    }
    Json(json!({
        "status":"confirmed", "principal_id":principal_id, "cluster_size":candidate.cluster_size,
        "evidence_tier":candidate.evidence_tier, "partial_success":true,
        "awareness_state":{"status":"skipped","reason_code":"speaker_awareness_state_not_native","detail":"Owner centroid was saved and the candidate was cleared, but awareness/current.json was not updated."}
    })).into_response()
}

pub async fn reject(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match solstone_core_speaker_resolve::owner_candidate::clear_owner_candidate(&root.0) {
        Ok(_) => Json(json!({
            "status":"rejected", "partial_success":true,
            "awareness_state":{"status":"skipped","reason_code":"speaker_awareness_state_not_native","detail":"The candidate was cleared, but the rejection/cooldown state was not recorded."}
        })).into_response(),
        Err(error) => owner_error(error.to_string()),
    }
}

async fn body(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| {
            err(
                "missing_request_body",
                "I couldn't find any data in that request.",
                "Unable to read request body",
                StatusCode::BAD_REQUEST,
            )
        })?;
    if bytes.is_empty() {
        return Err(err(
            "missing_request_body",
            "I couldn't find any data in that request.",
            "no request body",
            StatusCode::BAD_REQUEST,
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        err(
            "invalid_json_request",
            "I couldn't read that JSON request.",
            "request body must be a JSON object",
            StatusCode::BAD_REQUEST,
        )
    })
}

fn required(field: &str) -> Response {
    err(
        "missing_required_field",
        "I couldn't find a required field.",
        &format!("{field} is required"),
        StatusCode::BAD_REQUEST,
    )
}
fn err(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}
fn owner_error(detail: String) -> Response {
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
fn speaker_write_error(detail: String, labels: bool) -> Response {
    let (code, message) = if labels {
        (
            "speaker_labels_busy",
            "I couldn't update those speaker attributions right now because they were busy. Try again in a moment.",
        )
    } else {
        (
            "speaker_voiceprint_busy",
            "I couldn't update that voice right now because it was busy. Try again in a moment.",
        )
    };
    if detail.contains("busy") || detail.contains("lock") {
        err(code, message, &detail, StatusCode::SERVICE_UNAVAILABLE)
    } else {
        err(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            &detail,
            StatusCode::BAD_REQUEST,
        )
    }
}
