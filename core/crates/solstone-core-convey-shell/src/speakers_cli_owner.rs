// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI owner-decision and manual-tag routes.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use jiff::Timestamp;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::coalesce::ResponseLocale;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::SegmentLayout;

use crate::JournalRoot;
use crate::speakers_source::is_safe_source_component;
use crate::window_catalog;
use solstone_core_speaker_resolve::segment_catalog::{
    SegmentLookup, decode_required_stream_layout_value, lookup_segment,
};

pub async fn tag(
    Extension(root): Extension<Arc<JournalRoot>>,
    locale: Option<Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    let body = match body(locale, request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(day) = body.get("day").and_then(Value::as_str) else {
        return required(locale, "day");
    };
    let Some(stream) = body.get("stream").and_then(Value::as_str) else {
        return required(locale, "stream");
    };
    let Some(segment_key) = body.get("segment_key").and_then(Value::as_str) else {
        return required(locale, "segment_key");
    };
    let Some(sentence_id) = body.get("sentence_id").and_then(Value::as_i64) else {
        return required(locale, "sentence_id");
    };
    let Some(source) = body.get("source").and_then(Value::as_str) else {
        return required(locale, "source");
    };
    if !is_safe_source_component(source) {
        return err(
            locale,
            "invalid_request_value",
            "journal.window.speakers.attribution.attribution.response.source.name.is.invalid",
            "use one file name, without a path",
            StatusCode::BAD_REQUEST,
        );
    }
    let speaker = match solstone_core_speaker_resolve::owner_admission::admitted_owner_id(&root.0) {
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Admitted(id) => id,
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Invalid => {
            return err(
                locale,
                "speaker_owner_identity_invalid",
                "journal.window.speakers.error.attribution.owner.identity.invalid",
                "configured owner identity is not admitted",
                StatusCode::CONFLICT,
            );
        }
    };
    let layout = match decode_required_stream_layout_value(body.get("stream_layout")) {
        Ok(layout) => layout,
        Err(_) => {
            return err(
                locale,
                "invalid_segment_or_stream",
                "journal.window.speakers.attribution.attribution.response.that.segment.or.stream.couldn.t",
                "Invalid segment key or stream",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let segment = match lookup_segment(&root.0, day, stream, segment_key, Ok(layout)) {
        SegmentLookup::Present(path) => path,
        SegmentLookup::MalformedLayout => {
            return err(
                locale,
                "invalid_segment_or_stream",
                "journal.window.speakers.attribution.attribution.response.that.segment.or.stream.couldn.t",
                "Invalid segment key or stream",
                StatusCode::BAD_REQUEST,
            );
        }
        SegmentLookup::Absent => {
            return err(
                locale,
                "speaker_review_unavailable",
                "journal.window.speakers.error.that.speaker.review.couldn.t.be.loaded",
                "No speaker labels found",
                StatusCode::NOT_FOUND,
            );
        }
        SegmentLookup::Failed(error) => {
            return speaker_write_error(locale, error.to_string(), true);
        }
    };
    let labels = match std::fs::read(segment.join("talents/speaker_labels.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
    {
        Some(labels) => labels,
        None => {
            return err(
                locale,
                "speaker_review_unavailable",
                "journal.window.speakers.error.that.speaker.review.couldn.t.be.loaded",
                "No speaker labels found",
                StatusCode::NOT_FOUND,
            );
        }
    };
    let current = labels
        .iter()
        .find(|label| label.get("sentence_id").and_then(Value::as_i64) == Some(sentence_id));
    if current.is_some_and(|label| {
        label.get("speaker").and_then(Value::as_str) == Some(speaker.as_str())
            && label.get("method").and_then(Value::as_str) == Some("user_assigned")
    }) {
        return Json(json!({"success":true,"status":"already_assigned","stream_layout":layout_name(layout),"owner_bootstrap_outcome":Value::Null})).into_response();
    }
    if current
        .and_then(|label| label.get("speaker").and_then(Value::as_str))
        .is_some()
    {
        return err(
            locale,
            "speaker_attribution_state_invalid",
            "journal.window.speakers.error.attribution.state.invalid",
            "Pick a sentence without a speaker.",
            StatusCode::BAD_REQUEST,
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
            Err(error) => return speaker_write_error(locale, error.to_string(), false),
        };
    let Some(embedding) = embedding else {
        return err(
            locale,
            "speaker_sentence_missing",
            "journal.window.speakers.error.sentence.not.found",
            "Pick a different sentence with an embedding.",
            StatusCode::NOT_FOUND,
        );
    };
    let metadata = json!({"day":day,"stream_layout":layout_name(layout),"segment_key":segment_key,"source":source,"sentence_id":sentence_id,"stream":stream});
    if let Err(error) = solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
        &root.0,
        &speaker,
        embedding,
        metadata,
        &encoder(),
    ) {
        return speaker_write_error(locale, error.to_string(), false);
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
        return speaker_write_error(locale, error.to_string(), true);
    }
    let mut correction = Map::new();
    correction.insert("sentence_id".to_owned(), json!(sentence_id));
    correction.insert("original_speaker".to_owned(), Value::Null);
    correction.insert("corrected_speaker".to_owned(), json!(speaker));
    correction.insert(
        "original_method".to_owned(),
        old_method.map_or(Value::Null, Value::String),
    );
    correction.insert(
        "timestamp".to_owned(),
        json!(Timestamp::now().as_millisecond()),
    );
    if let Err(error) =
        solstone_core_speaker_id::corrections::append_correction(&segment, correction)
    {
        return speaker_write_error(locale, error.to_string(), true);
    }
    Json(json!({"success":true,"status":"assigned","speaker":speaker,"stream_layout":layout_name(layout)})).into_response()
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

pub async fn confirm(
    Extension(root): Extension<Arc<JournalRoot>>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    if matches!(
        solstone_core_speaker_resolve::owner_admission::admitted_owner_id(&root.0),
        solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Invalid
    ) {
        return err(
            locale,
            "speaker_owner_identity_invalid",
            "journal.window.speakers.error.owner.confirm.identity.invalid",
            "configured owner identity is not admitted",
            StatusCode::CONFLICT,
        );
    }
    err(
        locale,
        "review_required",
        "journal.window.speakers.error.owner.confirm.review.required",
        "Native confirm-owner does not save a centroid. Open the browser to review the candidate.",
        StatusCode::BAD_REQUEST,
    )
}

/// The CLI reject records the same rejection, cooldown and remembered voice
/// as the browser's "no, try again".
pub async fn reject(
    Extension(root): Extension<Arc<JournalRoot>>,
    locale: Option<Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    let body = match body(locale, request).await {
        Ok(val) => val,
        Err(resp) => return resp,
    };
    let Some(version) = body.get("version").and_then(Value::as_str) else {
        if matches!(
            solstone_core_speaker_resolve::owner_admission::admitted_owner_id(&root.0),
            solstone_core_speaker_resolve::owner_admission::OwnerAdmission::Invalid
        ) {
            return crate::speakers_owner_write::owner_voice_error(
                locale,
                solstone_core_speaker_resolve::owner_voice::OwnerVoiceError::IdentityInvalid,
            );
        }
        return err(
            locale,
            "missing_required_field",
            "journal.window.speakers.attribution.attribution.response.a.required.field.is.missing",
            "version is required",
            StatusCode::BAD_REQUEST,
        );
    };
    match solstone_core_speaker_resolve::owner_voice::reject_owner_candidate(&root.0, version) {
        Ok(value) => Json(value).into_response(),
        Err(error) => crate::speakers_owner_write::owner_voice_error(locale, error),
    }
}

async fn body(locale: &str, request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| {
            err(
                locale,
                "missing_request_body",
                "journal.window.speakers.attribution.attribution.response.that.request.had.no.data.in",
                "Unable to read request body",
                StatusCode::BAD_REQUEST,
            )
        })?;
    if bytes.is_empty() {
        return Err(err(
            locale,
            "missing_request_body",
            "journal.window.speakers.attribution.attribution.response.that.request.had.no.data.in",
            "no request body",
            StatusCode::BAD_REQUEST,
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        err(
            locale,
            "invalid_json_request",
            "journal.window.speakers.attribution.attribution.response.that.json.request.couldn.t.be",
            "request body must be a JSON object",
            StatusCode::BAD_REQUEST,
        )
    })
}

fn required(locale: &str, field: &str) -> Response {
    err(
        locale,
        "missing_required_field",
        "journal.window.speakers.attribution.attribution.response.a.required.field.is.missing",
        &format!("{field} is required"),
        StatusCode::BAD_REQUEST,
    )
}
fn err(locale: &str, code: &str, message_id: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(
        code,
        window_catalog::text(locale, message_id),
        detail,
        status,
    )
    .into_response()
}
fn speaker_write_error(locale: &str, detail: String, labels: bool) -> Response {
    let (code, message_id) = if labels {
        (
            "speaker_labels_busy",
            "journal.window.speakers.attribution.attribution.response.those.speaker.attributions.couldn.t.be",
        )
    } else {
        (
            "speaker_voiceprint_busy",
            "journal.window.speakers.attribution.attribution.response.that.voice.couldn.t.be.updated",
        )
    };
    if detail.contains("busy") || detail.contains("lock") {
        err(
            locale,
            code,
            message_id,
            &detail,
            StatusCode::SERVICE_UNAVAILABLE,
        )
    } else {
        err(
            locale,
            "speaker_command_failed",
            "journal.window.speakers.error.that.speaker.command.didn.t.finish",
            &detail,
            StatusCode::BAD_REQUEST,
        )
    }
}
