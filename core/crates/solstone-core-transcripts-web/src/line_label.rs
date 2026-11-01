// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Who said one transcript line, in the owner's words.
//!
//! Setting writes the owner's answer for the line to the owner line-label
//! store; removing deletes the line's answer whoever gave it, so the line
//! goes back to the journal's guess. Neither touches the segment's speaker
//! labels, corrections, voiceprints or owner-voice state, and neither writes
//! an activity edit. After a change the journal is asked to think again the
//! activities over the segment and the day's briefing.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as RoutePath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};
use solstone_core_speaker_resolve::owner_line_labels::{
    LineSpeaker, OwnerLine, OwnerLineLabelError, remove_owner_line_labels, set_owner_line_labels,
};

use crate::{AppState, legacy_error_response};

const ACTOR_HEADER: &str = "x-solstone-actor";
const ACTOR_OWNER: &str = "owner";

/// What the routes need from a request before anything is written.
struct Target {
    day: String,
    stream: String,
    key: String,
    line: OwnerLine,
    body: Map<String, Value>,
}

#[allow(clippy::result_large_err)]
fn target(
    root: &Path,
    headers: &HeaderMap,
    (day, stream, key): (String, String, String),
    body: &[u8],
) -> Result<Target, Response> {
    if headers.get(ACTOR_HEADER).and_then(|v| v.to_str().ok()) != Some(ACTOR_OWNER) {
        return Err(legacy_error_response(
            "owner_line_owner_only",
            "only you can say who said a line.",
            "saying who said a line needs the owner",
            StatusCode::FORBIDDEN,
        ));
    }
    if !crate::day::valid_day(&day)
        || !crate::segment::valid_stream(&stream)
        || matches!(stream.as_str(), "." | "..")
        || !crate::segment::valid_key(&key)
    {
        return Err(invalid("day, stream and segment name one segment"));
    }
    let body = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let (Some(source), Some(sentence_id)) = (
        body.get("source")
            .and_then(Value::as_str)
            .filter(|source| !source.is_empty()),
        body.get("sentence_id").and_then(Value::as_i64),
    ) else {
        return Err(invalid("source and sentence_id name the line"));
    };
    let line = OwnerLine {
        segment_dir: root
            .join("chronicle")
            .join(crate::segment_media::segment_rel(&day, &stream, &key)),
        source: source.to_owned(),
        sentence_id,
    };
    Ok(Target {
        day,
        stream,
        key,
        line,
        body,
    })
}

fn invalid(detail: &str) -> Response {
    legacy_error_response(
        "owner_line_invalid",
        "who said that line couldn't be saved.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}

fn store_refused(error: &OwnerLineLabelError) -> Response {
    let status = match error {
        OwnerLineLabelError::NoLines => StatusCode::BAD_REQUEST,
        OwnerLineLabelError::Lock(_) | OwnerLineLabelError::MergeRecoveryPending => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        OwnerLineLabelError::Write(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    };
    legacy_error_response(
        error.reason_code(),
        "who said that line couldn't be saved.",
        error.to_string(),
        status,
    )
}

/// Ask the journal to think again the activities over the segment, in any
/// facet, and the day's briefing. The answer stands either way.
fn think_again(root: &Path, target: &Target) -> Map<String, Value> {
    let pairs = BTreeSet::from([(target.stream.clone(), target.key.clone())]);
    let activities = solstone_core_system::owner_ask::activities_over(root, &target.day, &pairs)
        .unwrap_or_default();
    #[cfg(not(test))]
    let asked = solstone_core_system::owner_ask::think_again(root, &target.day, &activities, true);
    // Routine tests check the ask itself and never wake work discovery's socket.
    #[cfg(test)]
    let asked = solstone_core_system::owner_ask::record(
        root,
        &target.day,
        &activities,
        true,
        chrono::Utc::now().timestamp_millis(),
    );
    let mut reply = Map::from_iter([("ok".to_owned(), Value::Bool(true))]);
    match asked {
        Ok(_) => {
            reply.insert("asked".to_owned(), Value::Bool(true));
        }
        Err(error) => {
            reply.insert("asked".to_owned(), Value::Bool(false));
            reply.insert(
                "ask_reason".to_owned(),
                Value::String(error.reason_code().to_owned()),
            );
        }
    }
    reply
}

pub(crate) async fn set_line_label(
    State(state): State<Arc<AppState>>,
    RoutePath(path): RoutePath<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let root = state.journal_root.as_path();
    let target = match target(root, &headers, path, &body) {
        Ok(target) => target,
        Err(response) => return response,
    };
    let Some(speaker) = target
        .body
        .get("speaker")
        .and_then(LineSpeaker::from_tagged)
    else {
        return legacy_error_response(
            "owner_line_speaker_invalid",
            "who said that line couldn't be saved.",
            "speaker is {\"kind\":\"owner\"}, {\"kind\":\"person\",\"id\":...} or {\"kind\":\"unknown\"}",
            StatusCode::BAD_REQUEST,
        );
    };
    match set_owner_line_labels(root, std::slice::from_ref(&target.line), &speaker) {
        Ok(_) => Json(think_again(root, &target)).into_response(),
        Err(error) => store_refused(&error),
    }
}

pub(crate) async fn remove_line_label(
    State(state): State<Arc<AppState>>,
    RoutePath(path): RoutePath<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let root = state.journal_root.as_path();
    let target = match target(root, &headers, path, &body) {
        Ok(target) => target,
        Err(response) => return response,
    };
    match remove_owner_line_labels(root, std::slice::from_ref(&target.line)) {
        Ok(0) => Json(serde_json::json!({"ok": true, "asked": false})).into_response(),
        Ok(_) => Json(think_again(root, &target)).into_response(),
        Err(error) => store_refused(&error),
    }
}

#[cfg(test)]
#[path = "line_label_tests.rs"]
mod tests;
