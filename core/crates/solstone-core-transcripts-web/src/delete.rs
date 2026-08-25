// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deferred owner-directed transcript segment deletion.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_retention::door;
use solstone_core_retention::{NoIndex, Outcome, RemovalReason, Target};

use crate::{AppState, legacy_error_response};

pub(crate) async fn delete_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((day, stream, key)): RoutePath<(String, String, String)>,
) -> Response {
    if !valid_day(&day) {
        return invalid_day();
    }
    if !valid_key(&key) {
        return invalid_segment("Invalid segment key format", StatusCode::BAD_REQUEST);
    }
    if !valid_stream(&stream) {
        return invalid_segment("Invalid stream format", StatusCode::BAD_REQUEST);
    }
    let day_dir = state.journal_root.join("chronicle").join(&day);
    let segment_dir = day_dir.join(&stream).join(&key);
    if !segment_dir.is_dir() {
        return invalid_segment("Segment not found", StatusCode::NOT_FOUND);
    }
    // valid_day/valid_stream/valid_key exclude a `..` path component, so this
    // is defense in depth for a future validator regression, not reachable now;
    // retention also refuses per-entry removals outside the journal.
    if segment_dir.strip_prefix(&day_dir).is_err() {
        return invalid_segment("Invalid segment path", StatusCode::FORBIDDEN);
    }

    let pending_id = match pending_id() {
        Ok(value) => value,
        Err(error) => {
            return legacy_error_response(
                "file_read_failed",
                "I couldn't read that file.",
                format!("Failed to delete segment: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let search_index_warning =
        !solstone_core_system::lifecycle::is_supervisor_up(&*state.journal_root);
    let request = DeleteRequest {
        day,
        stream,
        key,
        pending_id: pending_id.clone(),
    };
    append_action(&state.journal_root, &request, "pending", json!({}));

    let root = (*state.journal_root).clone();
    let commit_request = request.clone();
    state
        .deferred_deletes
        .schedule(pending_id.clone(), state.delete_window, move || {
            commit_delete(root, commit_request)
        });

    let commit_at_ms = Utc::now().timestamp_millis() + state.delete_window.as_millis() as i64;
    let mut body = json!({
        "success": true,
        "deleted": request.key,
        "pending": pending_id,
        "commit_at_ms": commit_at_ms,
        "ttl_seconds": 10,
    });
    if search_index_warning {
        body["search_index_warning"] = json!(true);
    }
    Json(body).into_response()
}

pub(crate) async fn cancel_delete(
    State(state): State<Arc<AppState>>,
    RoutePath(pending_id): RoutePath<String>,
) -> Response {
    if !valid_pending_id(&pending_id) || !state.deferred_deletes.cancel(&pending_id) {
        return operation_unavailable();
    }
    // Intentional Python divergence: this writer files by Local::now(), not segment day.
    let _ = solstone_core_facets::append_action_log(
        &state.journal_root,
        None,
        "app",
        "transcripts",
        "segment_delete",
        json!({"pending_id":pending_id,"phase":"cancelled"}),
    );
    Json(json!({"cancelled":pending_id})).into_response()
}

#[derive(Clone, Debug)]
struct DeleteRequest {
    day: String,
    stream: String,
    key: String,
    pending_id: String,
}

fn commit_delete(journal_root: PathBuf, request: DeleteRequest) {
    let target = Target {
        day: request.day.clone(),
        stream: request.stream.clone(),
        dir: request.key.clone(),
    };
    let outcome = door::remove_segments(
        &journal_root,
        &[target],
        &Utc::now().to_rfc3339(),
        RemovalReason::OwnerSegmentDelete,
        "unknown",
    );
    // The chronicle is authoritative: this notification deliberately happens
    // only after the removal door has returned its proven outcome.
    let _ = door::notify_index(&NoIndex, &outcome);
    record_terminal_action(&journal_root, &request, &outcome);
}

fn record_terminal_action(journal_root: &Path, request: &DeleteRequest, outcome: &Outcome) {
    let (phase, detail) = terminal_detail(outcome);
    append_action(journal_root, request, phase, detail);
}

/// Classify every outcome shape at the one terminal action-log boundary.
///
/// `remove_segments` currently cannot set `halted`, but checking it first keeps
/// the durable record conservative if a future door implementation can.
fn terminal_detail(outcome: &Outcome) -> (&'static str, Value) {
    if let Some(halt) = &outcome.halted {
        return ("failed", json!({"reason":halt.reason}));
    }
    let post_commit_failures = outcome
        .targets
        .iter()
        .filter_map(|target| target.post_commit_failure.as_ref())
        .map(|failure| json!({"entry":failure.entry,"reason":failure.reason}))
        .collect::<Vec<_>>();
    if !post_commit_failures.is_empty() {
        let removed = outcome
            .removed_paths()
            .map(|path| path.as_str().to_owned())
            .collect::<Vec<_>>();
        return (
            "failed",
            json!({"removed":removed,"post_commit_failures":post_commit_failures}),
        );
    }
    let refused = outcome
        .targets
        .iter()
        .flat_map(|target| target.not_removed.iter())
        .map(|entry| json!({"entry":entry.entry,"reason":entry.reason,"staged":entry.staged}))
        .collect::<Vec<_>>();
    if !refused.is_empty() {
        return ("refused", json!({"refused":refused}));
    }
    let removed = outcome
        .removed_paths()
        .map(|path| path.as_str().to_owned())
        .collect::<Vec<_>>();
    ("committed", json!({"removed":removed}))
}

fn append_action(journal_root: &Path, request: &DeleteRequest, phase: &str, detail: Value) {
    let mut params = serde_json::Map::new();
    params.insert("day".into(), json!(request.day));
    params.insert("segment_key".into(), json!(request.key));
    params.insert("stream".into(), json!(request.stream));
    params.insert("pending_id".into(), json!(request.pending_id));
    params.insert("phase".into(), json!(phase));
    if let Value::Object(detail) = detail {
        params.extend(detail);
    }
    // Intentional Python divergence: this writer files by Local::now(), not segment day.
    let _ = solstone_core_facets::append_action_log(
        journal_root,
        None,
        "app",
        "transcripts",
        "segment_delete",
        Value::Object(params),
    );
}

fn pending_id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn valid_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn valid_stream(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

pub(crate) fn valid_key(value: &str) -> bool {
    let Some((time, length)) = value.split_once('_') else {
        return false;
    };
    time.len() == 6
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && !length.is_empty()
        && length.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_pending_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid_day() -> Response {
    legacy_error_response(
        "invalid_day",
        "I couldn't use that day.",
        "Invalid day format",
        StatusCode::BAD_REQUEST,
    )
}

fn invalid_segment(detail: &str, status: StatusCode) -> Response {
    legacy_error_response(
        "invalid_segment_or_stream",
        "I couldn't use that segment or stream.",
        detail,
        status,
    )
}

fn operation_unavailable() -> Response {
    legacy_error_response(
        "operation_no_longer_available",
        "I couldn't finish because that action is no longer available.",
        "already committed or unknown",
        StatusCode::GONE,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use solstone_core_retention::{NotRemoved, Outcome, RunHalt, Target, TargetOutcome};
    use tempfile::TempDir;

    use super::{DeleteRequest, record_terminal_action, terminal_detail};

    #[test]
    fn halted_outcomes_are_failed_even_though_remove_segments_cannot_produce_them() {
        let root = TempDir::new().expect("journal");
        let request = DeleteRequest {
            day: "20260731".into(),
            stream: "field".into(),
            key: "090000_300".into(),
            pending_id: "0".repeat(32),
        };
        // `door::remove_segments` initializes halted to None and never mutates
        // it. This constructed receipt is the direct unit coverage for the
        // future-proof classifier, not a claim of an end-to-end halt path.
        let outcome = Outcome {
            targets: vec![],
            halted: Some(RunHalt {
                reason: "door stopped".into(),
            }),
        };
        super::append_action(root.path(), &request, "pending", serde_json::json!({}));
        record_terminal_action(root.path(), &request, &outcome);
        let (_, detail) = terminal_detail(&outcome);
        assert_eq!(detail["reason"], "door stopped");
        let actions = fs::read_dir(root.path().join("config/actions"))
            .expect("action directory")
            .next()
            .expect("action file")
            .expect("entry")
            .path();
        let rows = fs::read_to_string(actions)
            .expect("action")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json row"))
            .collect::<Vec<_>>();
        assert_eq!(rows[0]["params"]["phase"], "pending");
        assert_eq!(rows[1]["params"]["phase"], "failed");
        assert_eq!(rows[1]["params"]["reason"], "door stopped");
    }

    #[test]
    fn refusal_outcomes_keep_every_not_removed_entry() {
        let root = TempDir::new().expect("journal");
        let request = DeleteRequest {
            day: "20260731".into(),
            stream: "field".into(),
            key: "090000_300".into(),
            pending_id: "0".repeat(32),
        };
        let outcome = Outcome {
            targets: vec![TargetOutcome {
                target: Target {
                    day: "20260731".into(),
                    stream: "field".into(),
                    dir: "090000_300".into(),
                },
                removed: vec![],
                not_removed: vec![NotRemoved {
                    entry: "mic.flac".into(),
                    reason: "busy".into(),
                    staged: Some(".staged".into()),
                }],
                post_commit_failure: None,
            }],
            halted: None,
        };
        let (phase, detail) = terminal_detail(&outcome);
        assert_eq!(phase, "refused");
        assert_eq!(detail["refused"][0]["entry"], "mic.flac");
        super::append_action(root.path(), &request, "pending", serde_json::json!({}));
        record_terminal_action(root.path(), &request, &outcome);
        let actions = fs::read_dir(root.path().join("config/actions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let rows = fs::read_to_string(actions).unwrap();
        assert!(rows.contains("\"phase\": \"pending\""));
        assert!(rows.contains("\"phase\": \"refused\""));
    }
}
