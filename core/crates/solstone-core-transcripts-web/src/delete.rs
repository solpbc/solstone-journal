// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deferred owner-directed transcript segment deletion.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_retention::door;
use solstone_core_retention::tombstone::TOMBSTONE_NAME;
use solstone_core_retention::{Outcome, RemovalReason, Target};

use solstone_core_serving::held_delete::{Record, Registry, Settled, valid_pending_id};

use crate::pending::{DeleteRecord, DeleteState, STORE, SegmentManifest, SegmentTarget};
use crate::{AppState, legacy_error_response};

/// Why a resumed delete keeps a segment whose files are not the confirmed ones.
const CHANGED_REASON: &str =
    "this segment's files changed after you chose to delete it, so it was kept";
/// Why a resumed delete keeps a segment it has nothing to compare against.
const UNCONFIRMED_REASON: &str =
    "your journal couldn't confirm this was the segment you chose, so it was kept";
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
    let segment_dir =
        crate::segment_media::physical_segment_dir(&state.journal_root, &day, &stream, &key);
    if !segment_dir.is_dir() {
        return invalid_segment("Segment not found", StatusCode::NOT_FOUND);
    }
    // valid_day/valid_stream/valid_key exclude a `..` path component, so this
    // is defense in depth for a future validator regression, not reachable now;
    // retention also refuses per-entry removals outside the journal.
    if segment_dir.strip_prefix(&day_dir).is_err() {
        return invalid_segment("Invalid segment path", StatusCode::FORBIDDEN);
    }

    // One request at a time decides whether a segment already has a delete
    // waiting, so two tabs or a double submit cannot both create one.
    let Ok(_create) = STORE.create_lock(&state.journal_root) else {
        return legacy_error_response(
            "segment_delete_not_saved",
            "your journal couldn't start that delete, so nothing was deleted.",
            "Failed to take the delete lock",
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    };
    // A second request for a segment already waiting joins that delete. One
    // whose timer this process no longer holds (a commit that gave up on its
    // lock) is armed again rather than swallowed.
    let waiting = STORE.waiting_for(&state.journal_root, |target: &SegmentTarget| {
        target.day == day && target.stream == stream && target.key == key
    });
    if let Some(existing) = waiting {
        if !state.deferred_deletes.contains(&existing.pending_id) {
            let remaining = existing.commit_at_ms - Utc::now().timestamp_millis();
            let delay = Duration::from_millis(u64::try_from(remaining).unwrap_or(0).max(1));
            schedule_commit(
                &state.deferred_deletes,
                &state.journal_root,
                existing.pending_id.clone(),
                delay,
                true,
            );
        }
        return accepted(&existing, true);
    }
    let prepared = pending_id().and_then(|id| {
        SegmentManifest::of(&segment_dir)
            .map(|manifest| (id, manifest))
            .map_err(|error| error.to_string())
    });
    let (pending_id, manifest) = match prepared {
        Ok(value) => value,
        Err(error) => {
            return legacy_error_response(
                "file_read_failed",
                "that file couldn't be read.",
                format!("Failed to delete segment: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let record = Record::pending(
        pending_id,
        SegmentTarget {
            day,
            stream,
            key,
            manifest,
        },
        state.delete_window,
    );
    // ⛔ Durable before the answer: a delete the page reports as under way must
    // survive the journal stopping inside its window.
    if let Err(error) = STORE.write(&state.journal_root, &record) {
        return legacy_error_response(
            "segment_delete_not_saved",
            "your journal couldn't start that delete, so nothing was deleted.",
            format!("Failed to save the pending delete: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    append_action(
        &state.journal_root,
        &DeleteRequest::from(&record),
        "pending",
        json!({}),
    );

    schedule_commit(
        &state.deferred_deletes,
        &state.journal_root,
        record.pending_id.clone(),
        state.delete_window,
        false,
    );

    accepted(&record, false)
}

fn accepted(record: &DeleteRecord, joined: bool) -> Response {
    let mut body = json!({
        "success": true,
        "deleted": record.target.key,
        "pending": record.pending_id,
        "commit_at_ms": record.commit_at_ms,
        "ttl_seconds": 10,
    });
    if joined {
        // What is left of the first request's window, not a fresh one.
        body["ttl_seconds"] = json!(record.remaining_seconds());
    }
    Json(body).into_response()
}

fn schedule_commit(
    registry: &Registry,
    journal_root: &Path,
    pending_id: String,
    delay: Duration,
    resumed: bool,
) {
    let root = journal_root.to_path_buf();
    let id = pending_id.clone();
    registry.schedule(pending_id, delay, move || {
        commit_delete(&root, &id, resumed)
    });
}

/// Resume every delete a previous run confirmed and never finished; see
/// [`solstone_core_serving::held_delete::Store::resume`].
pub(crate) fn resume_pending(journal_root: &Path, registry: &Registry) {
    STORE.resume(
        journal_root,
        registry,
        "segment-delete-resume",
        resumable,
        |root, pending_id| commit_delete(root, pending_id, true),
    );
}

/// Whether a record names a segment the delete route itself would accept.
fn resumable(record: &DeleteRecord) -> bool {
    valid_pending_id(&record.pending_id)
        && valid_day(&record.target.day)
        && valid_stream(&record.target.stream)
        && valid_key(&record.target.key)
}

pub(crate) async fn cancel_delete(
    State(state): State<Arc<AppState>>,
    RoutePath(pending_id): RoutePath<String>,
) -> Response {
    let root = state.journal_root.as_path().to_path_buf();
    // An id this journal never issued changes nothing, not even a lock file.
    if !valid_pending_id(&pending_id) || STORE.read::<SegmentTarget>(&root, &pending_id).is_none() {
        return operation_unavailable();
    }
    let id = pending_id.clone();
    let settled = tokio::task::spawn_blocking(move || settle_cancel(&root, &id)).await;
    let Ok(settled) = settled else {
        return cancel_failed("the cancellation did not finish");
    };
    let record = match settled {
        Settled::Busy => return in_progress(),
        Settled::Failed(error) => return cancel_failed(&error),
        Settled::Record(record) => record,
    };
    if record.state == DeleteState::Cancelled {
        // Only now drop this process's timer; the record already says cancelled,
        // so a timer that fires first finds it and stops.
        state.deferred_deletes.cancel(&pending_id);
        return Json(json!({"cancelled":pending_id})).into_response();
    }
    // Too late to cancel: say what actually happened, never a guess.
    match record.state {
        DeleteState::Deleted => legacy_error_response(
            "segment_already_deleted",
            "that segment was already deleted.",
            "already committed",
            StatusCode::GONE,
        ),
        DeleteState::NotDeleted => legacy_error_response(
            "segment_not_deleted",
            "that segment wasn't deleted.",
            record.reason.unwrap_or_default(),
            StatusCode::CONFLICT,
        ),
        DeleteState::Incomplete => legacy_error_response(
            "segment_delete_incomplete",
            "that segment was only partly deleted.",
            record.reason.unwrap_or_default(),
            StatusCode::CONFLICT,
        ),
        DeleteState::Pending | DeleteState::Cancelled => in_progress(),
    }
}

fn settle_cancel(journal_root: &Path, pending_id: &str) -> Settled<SegmentTarget> {
    STORE.settle_cancel(journal_root, pending_id, || {
        // Intentional Python divergence: this writer files by Local::now(), not segment day.
        let _ = solstone_core_facets::append_action_log(
            journal_root,
            None,
            "app",
            "transcripts",
            "segment_delete",
            json!({"pending_id":pending_id,"phase":"cancelled"}),
        );
    })
}

fn in_progress() -> Response {
    legacy_error_response(
        "segment_delete_in_progress",
        "that delete is already under way.",
        "past the cancel window",
        StatusCode::CONFLICT,
    )
}

/// Nothing was settled, so the record is still pending and its timer or the
/// next start still runs it.
fn cancel_failed(detail: &str) -> Response {
    legacy_error_response(
        "segment_delete_not_cancelled",
        "that delete couldn't be cancelled, so it will still happen.",
        format!("Failed to save the cancellation: {detail}"),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

/// What became of one delete, for the page to report once the window passes.
pub(crate) async fn delete_status(
    State(state): State<Arc<AppState>>,
    RoutePath(pending_id): RoutePath<String>,
) -> Response {
    let record = valid_pending_id(&pending_id)
        .then(|| STORE.read::<SegmentTarget>(&state.journal_root, &pending_id))
        .flatten();
    let Some(record) = record else {
        return legacy_error_response(
            "segment_delete_unknown",
            "your journal has no record of that delete.",
            "unknown pending id",
            StatusCode::NOT_FOUND,
        );
    };
    Json(status_body(&record)).into_response()
}

/// Deletes from the last few days that did not remove their segment, so a page
/// opened after a restart can still tell the owner.
pub(crate) async fn delete_outcomes(State(state): State<Arc<AppState>>) -> Response {
    let outcomes = STORE
        .unfinished_outcomes::<SegmentTarget>(&state.journal_root, Utc::now())
        .iter()
        .filter(|record| record.state != DeleteState::Pending || resumable(record))
        .map(status_body)
        .collect::<Vec<_>>();
    Json(json!({ "outcomes": outcomes })).into_response()
}

fn status_body(record: &DeleteRecord) -> Value {
    let mut body = json!({
        "pending": record.pending_id,
        "state": record.state.as_str(),
        "day": record.target.day,
        "stream": record.target.stream,
        "segment_key": record.target.key,
        "commit_at_ms": record.commit_at_ms,
    });
    if let Some(reason) = &record.reason {
        body["reason"] = json!(reason);
    }
    body
}

#[derive(Clone, Debug)]
struct DeleteRequest {
    day: String,
    stream: String,
    key: String,
    pending_id: String,
}

impl From<&DeleteRecord> for DeleteRequest {
    fn from(record: &DeleteRecord) -> Self {
        Self {
            day: record.target.day.clone(),
            stream: record.target.stream.clone(),
            key: record.target.key.clone(),
            pending_id: record.pending_id.clone(),
        }
    }
}

/// Run one delete to its outcome under the record's lock.
fn commit_delete(journal_root: &Path, pending_id: &str, resumed: bool) {
    STORE.commit(
        journal_root,
        pending_id,
        resumed,
        |record: &DeleteRecord, claim| {
            let resumed = claim.resumed;
            // `None`: another finisher holds the segment. The record stays pending,
            // with its claim, so the next run settles it from what is then on disk.
            let (phase, detail, state, reason) = run_delete(journal_root, record, resumed)?;
            let marker = if resumed {
                json!({"resumed":true})
            } else {
                json!({})
            };
            append_action(
                journal_root,
                &DeleteRequest::from(record),
                phase,
                merge(marker, detail),
            );
            Some((state, reason.map(str::to_owned)))
        },
    );
}

fn is_tombstone_only(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        if entry.file_name() == TOMBSTONE_NAME {
            count += 1;
        } else {
            return false;
        }
    }
    count == 1
}

fn run_delete(
    journal_root: &Path,
    record: &DeleteRecord,
    resumed: bool,
) -> Option<(&'static str, Value, DeleteState, Option<&'static str>)> {
    let segment = &record.target;
    let target = Target {
        day: segment.day.clone(),
        stream: segment.stream.clone(),
        dir: segment.key.clone(),
    };
    let segment_rel = format!(
        "chronicle/{}",
        crate::segment_media::segment_rel(&segment.day, &segment.stream, &segment.key)
    );
    let segment_dir = crate::segment_media::physical_segment_dir(
        journal_root,
        &segment.day,
        &segment.stream,
        &segment.key,
    );
    let deleted_at = Utc::now().to_rfc3339();

    // 1. If the live physical directory's only entry is tombstone.json:
    if is_tombstone_only(&segment_dir) {
        let outcome = door::confirmed_absent(journal_root, &target);
        let has_removed = outcome.removed_paths().next().is_some();
        let no_not_removed = outcome.targets.iter().all(|t| t.not_removed.is_empty());
        if has_removed && no_not_removed {
            let notify_result = door::notify_index(&RetentionIndex::new(journal_root), &outcome);
            let (phase, detail) = terminal_detail(&outcome);
            let detail = fold_notify_detail(detail, notify_result);
            return Some((phase, detail, DeleteState::Deleted, None));
        } else {
            return Some((
                "committed",
                json!({"already_removed": true}),
                DeleteState::Deleted,
                None,
            ));
        }
    }

    // 2. Holding directory: live when it holds bytes, otherwise staged sibling
    let staged_sibling = staged_dir(journal_root, record);
    let live_holds_bytes = segment_dir.is_dir() && !is_tombstone_only(&segment_dir);
    let staged_holds_bytes = !live_holds_bytes && staged_sibling.is_dir();
    let holding_dir = if live_holds_bytes {
        Some(&segment_dir)
    } else if staged_holds_bytes {
        Some(&staged_sibling)
    } else {
        None
    };

    // 3. Pre-removal owner-file verification
    if let Some(holding) = holding_dir {
        if record.target.manifest.files.is_empty() {
            if resumed {
                return Some((
                    "refused",
                    json!({"refused":[{"entry":segment_rel,"reason":UNCONFIRMED_REASON,"staged":null}]}),
                    DeleteState::NotDeleted,
                    Some(UNCONFIRMED_REASON),
                ));
            }
        } else {
            let hold_verdict = record.target.manifest.hold(holding);
            let has_added = record.target.manifest.has_added_owner_class_file(holding);
            match (hold_verdict, has_added) {
                (crate::pending::HoldVerdict::Unreadable, _) | (_, Err(_)) => {
                    return Some((
                        "refused",
                        json!({"refused":[{"entry":segment_rel,"reason":UNCONFIRMED_REASON,"staged":null}]}),
                        DeleteState::NotDeleted,
                        Some(UNCONFIRMED_REASON),
                    ));
                }
                (crate::pending::HoldVerdict::Changed, _) | (_, Ok(true)) => {
                    return Some((
                        "refused",
                        json!({"refused":[{"entry":segment_rel,"reason":CHANGED_REASON,"staged":null}]}),
                        DeleteState::NotDeleted,
                        Some(CHANGED_REASON),
                    ));
                }
                (crate::pending::HoldVerdict::Missing, Ok(false)) => {
                    if door::receipt_manifest(journal_root, &target, holding).is_none() {
                        return Some((
                            "refused",
                            json!({"refused":[{"entry":segment_rel,"reason":CHANGED_REASON,"staged":null}]}),
                            DeleteState::NotDeleted,
                            Some(CHANGED_REASON),
                        ));
                    }
                }
                (crate::pending::HoldVerdict::Matches, Ok(false)) => {}
            }
        }
    }

    // 4. Staged recovery mid-removal
    if !present(&segment_dir)
        && let Some(row) = door::recover_segment(
            journal_root,
            &target,
            &deleted_at,
            RemovalReason::OwnerSegmentDelete,
            "unknown",
        )
    {
        let outcome = Outcome {
            targets: vec![row],
            halted: None,
        };
        if outcome.removed_paths().next().is_none() && present(&staged_dir(journal_root, record)) {
            return None;
        }
        let notify_result = door::notify_index(&RetentionIndex::new(journal_root), &outcome);
        let (phase, detail) = terminal_detail(&outcome);
        let detail = merge(json!({"recovered":true}), detail);
        let detail = fold_notify_detail(detail, notify_result);
        return Some((phase, detail, owner_outcome(&outcome, &segment_dir), None));
    }

    // 5 & 6. Removal through door (live + staged coexistence falls through here and is refused)
    let outcome = door::remove_segments(
        journal_root,
        &[target],
        &deleted_at,
        RemovalReason::OwnerSegmentDelete,
        "unknown",
    );
    // The chronicle is authoritative: this notification deliberately happens
    // only after the removal door has returned its proven outcome.
    let notify_result = door::notify_index(&RetentionIndex::new(journal_root), &outcome);
    let (phase, detail) = terminal_detail(&outcome);
    let detail = fold_notify_detail(detail, notify_result);
    Some((phase, detail, owner_outcome(&outcome, &segment_dir), None))
}

fn fold_notify_detail(
    mut detail: Value,
    notify_result: Result<
        solstone_core_retention::PruneCounts,
        solstone_core_retention::NotifyError,
    >,
) -> Value {
    match notify_result {
        Ok(counts) => {
            if (counts.chunks > 0 || counts.files > 0)
                && let Value::Object(ref mut map) = detail
            {
                map.insert("index_chunks".into(), json!(counts.chunks));
                map.insert("index_files".into(), json!(counts.files));
            }
        }
        Err(_) => {
            if let Value::Object(ref mut map) = detail {
                map.insert("search_not_updated".into(), json!(true));
            }
        }
    }
    detail
}

fn staged_dir(journal_root: &Path, record: &DeleteRecord) -> std::path::PathBuf {
    crate::segment_media::physical_staged_dir(
        journal_root,
        &record.target.day,
        &record.target.stream,
        &record.target.key,
    )
}

/// Whether anything is at `path`. An error reading it counts as present, so an
/// unreadable segment is never taken for a removed one.
fn present(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// What the owner is told, on positive evidence only. Deleted needs the door's
/// own removal rows or a tombstone; a segment that is merely absent (moved,
/// unreadable) is never reported as deleted. A removal whose follow-up could
/// not be queued still removed the segment, so it reads as deleted. A refusal
/// is incomplete once any of the segment is gone or set aside.
fn owner_outcome(outcome: &Outcome, segment_dir: &Path) -> DeleteState {
    if outcome.halted.is_some() {
        return DeleteState::NotDeleted;
    }
    let refused = outcome
        .targets
        .iter()
        .any(|target| !target.not_removed.is_empty());
    let removed = outcome.removed_paths().next().is_some();
    let tombstoned = segment_dir.join(TOMBSTONE_NAME).is_file();
    if !refused && (removed || tombstoned) {
        return DeleteState::Deleted;
    }
    if refused && !removed && tombstoned {
        // Refused only because another removal got there first.
        return DeleteState::Deleted;
    }
    let staged = outcome
        .targets
        .iter()
        .flat_map(|target| target.not_removed.iter())
        .any(|entry| entry.staged.is_some());
    if removed || (staged && !present(segment_dir)) {
        return DeleteState::Incomplete;
    }
    DeleteState::NotDeleted
}

fn merge(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            base.extend(extra);
            Value::Object(base)
        }
        (_, extra) => extra,
    }
}

#[cfg(test)]
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
    if value == solstone_core_journal_io::DEFAULT_STREAM {
        return true;
    }
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

fn invalid_day() -> Response {
    legacy_error_response(
        "invalid_day",
        "that day couldn't be used.",
        "Invalid day format",
        StatusCode::BAD_REQUEST,
    )
}

fn invalid_segment(detail: &str, status: StatusCode) -> Response {
    legacy_error_response(
        "invalid_segment_or_stream",
        "that segment or stream couldn't be used.",
        detail,
        status,
    )
}

fn operation_unavailable() -> Response {
    legacy_error_response(
        "operation_no_longer_available",
        "that removal didn't finish because the action is no longer available.",
        "already committed or unknown",
        StatusCode::GONE,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use chrono::{NaiveDate, TimeZone, Utc};
    use serde_json::Value;
    use solstone_core_retention::{
        NotRemoved, Outcome, RemovalReason, RunHalt, Target, TargetOutcome,
    };
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::{
        CHANGED_REASON, DeleteRequest, DeleteState, Record, STORE, SegmentManifest, SegmentTarget,
        TOMBSTONE_NAME, UNCONFIRMED_REASON, commit_delete, door, record_terminal_action, resumable,
        terminal_detail,
    };
    use crate::{Clock, router_with_delete_window};
    use solstone_core_indexer_store::scan::scan_journal;

    fn shell() -> axum::response::Response {
        axum::response::Response::new(Body::from("shell"))
    }

    fn write(root: &Path, relative: &str, contents: impl AsRef<[u8]>) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        fs::write(path, contents).expect("file");
    }

    fn setup_acceptance_journal(root: &Path) {
        write(
            root,
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
        for (name, contents) in [
            ("audio.flac", b"raw".as_slice()),
            ("audio.jsonl", b"{}\n".as_slice()),
            ("stream.json", br#"{"stream":"field.audio"}"#.as_slice()),
            (
                "talents/summary.md",
                b"# Target\nneedle in target segment\n".as_slice(),
            ),
        ] {
            write(
                root,
                &format!("chronicle/20260805/field.audio/070000_17/{name}"),
                contents,
            );
        }
        for (name, contents) in [
            ("audio.flac", b"raw".as_slice()),
            ("audio.jsonl", b"{}\n".as_slice()),
            ("stream.json", br#"{"stream":"field.audio"}"#.as_slice()),
            (
                "talents/summary.md",
                b"# Sibling\nneedle in sibling segment\n".as_slice(),
            ),
        ] {
            write(
                root,
                &format!("chronicle/20260805/field.audio/071000_17/{name}"),
                contents,
            );
        }
        write(
            root,
            "chronicle/20260805/talents/flow.md",
            b"# Day Talent\nneedle in day talent\n",
        );
    }

    fn read_all_actions(journal_root: &Path) -> Vec<Value> {
        let actions_dir = journal_root.join("config/actions");
        if !actions_dir.is_dir() {
            return Vec::new();
        }
        let mut rows = Vec::new();
        for entry in fs::read_dir(actions_dir).expect("read actions dir") {
            let entry = entry.expect("entry");
            if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                let content = fs::read_to_string(entry.path()).expect("read action log");
                for line in content.lines() {
                    if !line.trim().is_empty() {
                        rows.push(serde_json::from_str::<Value>(line).expect("parse action json"));
                    }
                }
            }
        }
        rows
    }

    use std::collections::BTreeSet;

    #[tokio::test]
    async fn zero_window_delete_drops_the_target_from_search_and_keeps_the_sibling() {
        let root = TempDir::new().expect("journal");
        setup_acceptance_journal(root.path());

        scan_journal(root.path(), true).expect("scan journal");

        let ref_date = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let before = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search before");
        assert_eq!(before.results.len(), 3);

        let app = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()),
            shell,
            Duration::ZERO,
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/app/transcripts/api/segment/20260805/field.audio/070000_17")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("delete response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let json: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(
            json.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "commit_at_ms".to_string(),
                "deleted".to_string(),
                "pending".to_string(),
                "success".to_string(),
                "ttl_seconds".to_string(),
            ])
        );
        assert_eq!(json["deleted"], "070000_17");

        let after = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search after");
        assert_eq!(after.results.len(), 2);
        let paths: Vec<String> = after
            .results
            .iter()
            .map(|r| r.metadata.path.clone())
            .collect();
        assert!(paths.iter().any(|p| p.contains("071000_17")));
        assert!(paths.iter().any(|p| p.contains("talents/flow.md")));
        assert!(!paths.iter().any(|p| p.contains("070000_17")));

        let action_rows = read_all_actions(root.path());
        let committed = action_rows
            .iter()
            .find(|r| r["params"]["phase"] == "committed")
            .expect("committed action row");
        assert!(committed["params"]["index_chunks"].as_u64().unwrap() > 0);
        assert!(committed["params"]["index_files"].as_u64().unwrap() > 0);
        assert!(committed["params"].get("search_not_updated").is_none());
    }

    #[tokio::test]
    async fn cancelled_delete_leaves_the_index_and_stays_cancelled() {
        let root = TempDir::new().expect("journal");
        setup_acceptance_journal(root.path());

        scan_journal(root.path(), true).expect("scan journal");

        let app = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()),
            shell,
            Duration::from_secs(60),
        );

        let delete_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/app/transcripts/api/segment/20260805/field.audio/070000_17")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("delete response");
        assert_eq!(delete_res.status(), StatusCode::OK);
        let delete_bytes = to_bytes(delete_res.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let delete_json: Value = serde_json::from_slice(&delete_bytes).expect("json");
        assert_eq!(delete_json["deleted"], "070000_17");
        let pending_id = delete_json["pending"].as_str().expect("pending id string");

        let cancel_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/app/transcripts/api/cancel-delete/{pending_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("cancel response");
        assert_eq!(cancel_res.status(), StatusCode::OK);

        let snapshot_before = {
            let conn = solstone_core_indexer_store::db::open_index(root.path())
                .expect("open index before");
            let mut stmt = conn
                .prepare("SELECT path FROM files ORDER BY path")
                .expect("prepare");
            let paths: Vec<String> = stmt
                .query_map([], |row| row.get(0))
                .expect("query")
                .map(|r| r.expect("path"))
                .collect();
            paths
        };

        super::commit_delete(root.path(), pending_id, false);

        let snapshot_after = {
            let conn =
                solstone_core_indexer_store::db::open_index(root.path()).expect("open index after");
            let mut stmt = conn
                .prepare("SELECT path FROM files ORDER BY path")
                .expect("prepare");
            let paths: Vec<String> = stmt
                .query_map([], |row| row.get(0))
                .expect("query")
                .map(|r| r.expect("path"))
                .collect();
            paths
        };
        assert_eq!(snapshot_before, snapshot_after);

        let ref_date = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let search_res = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search after cancel");
        assert_eq!(search_res.results.len(), 3);
        let paths: Vec<String> = search_res
            .results
            .iter()
            .map(|r| r.metadata.path.clone())
            .collect();
        assert!(paths.iter().any(|p| p.contains("070000_17")));

        let status_res = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/app/transcripts/api/delete-status/{pending_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("status response");
        assert_eq!(status_res.status(), StatusCode::OK);
        let status_bytes = to_bytes(status_res.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let status_json: Value = serde_json::from_slice(&status_bytes).expect("json");
        assert_eq!(status_json["state"], "cancelled");

        let action_rows = read_all_actions(root.path());
        assert!(
            action_rows
                .iter()
                .any(|r| r["params"]["phase"] == "cancelled")
        );
        assert!(
            !action_rows
                .iter()
                .any(|r| r["params"]["phase"] == "committed")
        );
    }

    #[tokio::test]
    async fn delete_without_an_index_settles_deleted_and_creates_none() {
        let root = TempDir::new().expect("journal");
        setup_acceptance_journal(root.path());

        let app = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()),
            shell,
            Duration::ZERO,
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/app/transcripts/api/segment/20260805/field.audio/070000_17")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("delete response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let json: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["deleted"], "070000_17");

        let segment_dir = root.path().join("chronicle/20260805/field.audio/070000_17");
        assert!(segment_dir.join(super::TOMBSTONE_NAME).is_file());

        assert!(!root.path().join("indexer").exists());

        let action_rows = read_all_actions(root.path());
        let committed = action_rows
            .iter()
            .find(|r| r["params"]["phase"] == "committed")
            .expect("committed action row");
        assert!(committed["params"].get("search_not_updated").is_none());
    }

    #[tokio::test]
    async fn corrupt_index_records_search_not_updated_and_still_deletes() {
        let root = TempDir::new().expect("journal");
        setup_acceptance_journal(root.path());

        scan_journal(root.path(), true).expect("scan journal");

        let index_file = root.path().join("indexer/journal.sqlite");
        let _ = fs::remove_file(root.path().join("indexer/journal.sqlite-wal"));
        let _ = fs::remove_file(root.path().join("indexer/journal.sqlite-shm"));
        fs::write(&index_file, b"not a valid sqlite database").expect("corrupt db write");

        let app = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()),
            shell,
            Duration::ZERO,
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/app/transcripts/api/segment/20260805/field.audio/070000_17")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("delete response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let json: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["deleted"], "070000_17");
        assert!(json.get("search_not_updated").is_none());
        let pending_id = json["pending"].as_str().expect("pending id");

        let segment_dir = root.path().join("chronicle/20260805/field.audio/070000_17");
        assert!(segment_dir.join(super::TOMBSTONE_NAME).is_file());

        let status_res = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/app/transcripts/api/delete-status/{pending_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("status response");
        assert_eq!(status_res.status(), StatusCode::OK);
        let status_bytes = to_bytes(status_res.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let status_json: Value = serde_json::from_slice(&status_bytes).expect("json");
        assert_eq!(status_json["state"], "deleted");
        assert!(status_json.get("search_not_updated").is_none());

        let record_path = root
            .path()
            .join("config/segment-deletes")
            .join(format!("{pending_id}.json"));
        let record_text = fs::read_to_string(&record_path).expect("read held delete record");
        assert!(!record_text.contains("search_not_updated"));
        let record_json: Value =
            serde_json::from_str(&record_text).expect("parse held delete json");
        assert_eq!(record_json["state"], "deleted");

        let action_rows = read_all_actions(root.path());
        let committed = action_rows
            .iter()
            .find(|r| r["params"]["phase"] == "committed")
            .expect("committed action row");
        assert_eq!(committed["params"]["search_not_updated"], true);
        assert!(committed["params"].get("reason").is_none());
    }

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

    fn setup_direct_and_named(root: &Path, day: &str, key: &str, named_stream: &str) {
        write(
            root,
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
        write(
            root,
            &format!("chronicle/{day}/{key}/audio.flac"),
            b"direct flac",
        );
        write(root, &format!("chronicle/{day}/{key}/audio.jsonl"), b"{}\n");
        write(
            root,
            &format!("chronicle/{day}/{key}/talents/summary.md"),
            b"# Direct Target\nneedle in direct target\n",
        );
        write(
            root,
            &format!("chronicle/{day}/{named_stream}/{key}/audio.flac"),
            b"named flac",
        );
        write(
            root,
            &format!("chronicle/{day}/{named_stream}/{key}/audio.jsonl"),
            b"{}\n",
        );
        write(
            root,
            &format!("chronicle/{day}/{named_stream}/{key}/stream.json"),
            format!(r#"{{"stream":"{named_stream}"}}"#).as_bytes(),
        );
        write(
            root,
            &format!("chronicle/{day}/{named_stream}/{key}/talents/summary.md"),
            b"# Named Sibling\nneedle in named sibling\n",
        );
    }

    #[test]
    fn tombstone_only_settlement_direct_segment() {
        let root = TempDir::new().expect("journal");
        let day = "20260805";
        let key = "070000_17";
        let named_stream = "field.audio";
        write(
            root.path(),
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
        for (name, contents) in [
            ("audio.flac", b"raw direct audio".as_slice()),
            ("audio.jsonl", b"{}\n".as_slice()),
        ] {
            write(
                root.path(),
                &format!("chronicle/{day}/{key}/{name}"),
                contents,
            );
        }
        for (name, contents) in [
            ("audio.flac", b"raw named audio".as_slice()),
            ("audio.jsonl", b"{}\n".as_slice()),
            ("stream.json", br#"{"stream":"field.audio"}"#.as_slice()),
            (
                "talents/summary.md",
                b"# Named Sibling\nneedle in named sibling\n".as_slice(),
            ),
        ] {
            write(
                root.path(),
                &format!("chronicle/{day}/{named_stream}/{key}/{name}"),
                contents,
            );
        }

        scan_journal(root.path(), true).expect("scan journal");

        let ref_date = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let before_named = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in named sibling",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search before named");
        assert_eq!(before_named.results.len(), 1);

        let direct_manifest =
            SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
        let target = Target {
            day: day.to_string(),
            stream: solstone_core_journal_io::DEFAULT_STREAM.to_string(),
            dir: key.to_string(),
        };

        let _outcome = door::remove_segments(
            root.path(),
            &[target],
            "2026-08-05T12:00:00Z",
            solstone_core_retention::RemovalReason::OwnerSegmentDelete,
            "cid-test",
        );

        let physical_dir = root.path().join(format!("chronicle/{day}/{key}"));
        assert!(physical_dir.join(TOMBSTONE_NAME).is_file());
        let entries: BTreeSet<String> = fs::read_dir(&physical_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(entries, BTreeSet::from([TOMBSTONE_NAME.to_string()]));

        let tombstone_text = fs::read_to_string(physical_dir.join(TOMBSTONE_NAME)).unwrap();
        let tombstone_json: Value = serde_json::from_str(&tombstone_text).unwrap();
        let manifest_array = tombstone_json["manifest"]
            .as_array()
            .expect("manifest array");
        assert!(manifest_array.iter().any(|p| {
            let s = p.as_str().unwrap();
            s.contains("070000_17/audio.flac") && !s.contains(named_stream)
        }));
        assert!(
            !manifest_array
                .iter()
                .any(|p| p.as_str().unwrap().contains(named_stream))
        );

        let direct_flac_entry = manifest_array
            .iter()
            .find(|p| p.as_str().unwrap().ends_with("audio.flac"))
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let conn = solstone_core_indexer_store::db::open_index(root.path()).unwrap();
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?1, ?2, ?3, '', 'segment', '', 0, '')",
            rusqlite::params!["needle in direct target", direct_flac_entry, day],
        )
        .unwrap();

        let mid_direct = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in direct target",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search mid direct");
        assert_eq!(mid_direct.results.len(), 1);

        let pending_id = "1".repeat(32);
        let record = Record::pending(
            pending_id.clone(),
            SegmentTarget {
                day: day.to_string(),
                stream: solstone_core_journal_io::DEFAULT_STREAM.to_string(),
                key: key.to_string(),
                manifest: direct_manifest,
            },
            Duration::from_secs(10),
        );
        STORE.write(root.path(), &record).expect("write record");

        assert!(resumable(&record));

        commit_delete(root.path(), &pending_id, true);

        let settled = STORE
            .read::<SegmentTarget>(root.path(), &pending_id)
            .unwrap();
        assert_eq!(settled.state, DeleteState::Deleted);

        let after_direct = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in direct target",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search after direct");
        assert_eq!(after_direct.results.len(), 0);

        let after_named = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in named sibling",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search after named");
        assert_eq!(after_named.results.len(), 1);

        commit_delete(root.path(), &pending_id, true);
        let after2_named = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in named sibling",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search after 2 named");
        assert_eq!(after2_named.results.len(), 1);
    }

    #[tokio::test]
    async fn direct_layout_http_accept_and_zero_window_delete() {
        let root = TempDir::new().expect("journal");
        let day = "20260805";
        let key = "070000_17";
        let named_stream = "field.audio";
        write(
            root.path(),
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
        for (name, contents) in [
            ("audio.flac", b"raw direct audio".as_slice()),
            ("audio.jsonl", b"{}\n".as_slice()),
        ] {
            write(
                root.path(),
                &format!("chronicle/{day}/{key}/{name}"),
                contents,
            );
        }
        for (name, contents) in [
            ("audio.flac", b"raw named audio".as_slice()),
            ("audio.jsonl", b"{}\n".as_slice()),
            ("stream.json", br#"{"stream":"field.audio"}"#.as_slice()),
            (
                "talents/summary.md",
                b"# Named Sibling\nneedle in named sibling\n".as_slice(),
            ),
        ] {
            write(
                root.path(),
                &format!("chronicle/{day}/{named_stream}/{key}/{name}"),
                contents,
            );
        }

        scan_journal(root.path(), true).expect("scan journal");

        let direct_flac_entry = format!("chronicle/{day}/{key}/audio.flac");
        let conn = solstone_core_indexer_store::db::open_index(root.path()).unwrap();
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?1, ?2, ?3, '', 'segment', '', 0, '')",
            rusqlite::params!["needle in direct target", direct_flac_entry, day],
        )
        .unwrap();

        let ref_date = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let before_direct = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in direct target",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search before direct");
        assert_eq!(before_direct.results.len(), 1);

        let direct_before_flac = fs::read(
            root.path()
                .join(format!("chronicle/{day}/{key}/audio.flac")),
        )
        .unwrap();
        let named_before_flac = fs::read(
            root.path()
                .join(format!("chronicle/{day}/{named_stream}/{key}/audio.flac")),
        )
        .unwrap();

        // 1. Non-zero window
        let app = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()),
            shell,
            Duration::from_secs(60),
        );

        let delete_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/app/transcripts/api/segment/{day}/_default/{key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("delete response");
        assert_eq!(delete_res.status(), StatusCode::OK);
        let delete_bytes = to_bytes(delete_res.into_body(), usize::MAX)
            .await
            .expect("bytes");
        let delete_json: Value = serde_json::from_slice(&delete_bytes).expect("json");
        assert_eq!(
            delete_json
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "commit_at_ms".to_string(),
                "deleted".to_string(),
                "pending".to_string(),
                "success".to_string(),
                "ttl_seconds".to_string(),
            ])
        );
        assert_eq!(delete_json["success"], true);
        assert_eq!(delete_json["deleted"], key);
        let pending_id = delete_json["pending"].as_str().expect("pending id");

        let record = STORE
            .read::<SegmentTarget>(root.path(), pending_id)
            .expect("read record");
        assert_eq!(record.target.day, day);
        assert_eq!(record.target.stream, "_default");
        assert_eq!(record.target.key, key);
        assert!(record.target.manifest.files.contains_key("audio.flac"));
        assert!(!record.target.manifest.files.contains_key("stream.json"));

        let dup_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/app/transcripts/api/segment/{day}/_default/{key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("dup response");
        assert_eq!(dup_res.status(), StatusCode::OK);
        let dup_bytes = to_bytes(dup_res.into_body(), usize::MAX).await.unwrap();
        let dup_json: Value = serde_json::from_slice(&dup_bytes).unwrap();
        assert_eq!(dup_json["pending"], pending_id);

        let cancel_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/app/transcripts/api/cancel-delete/{pending_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("cancel response");
        assert_eq!(cancel_res.status(), StatusCode::OK);

        assert_eq!(
            fs::read(
                root.path()
                    .join(format!("chronicle/{day}/{key}/audio.flac"))
            )
            .unwrap(),
            direct_before_flac
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join(format!("chronicle/{day}/{named_stream}/{key}/audio.flac"))
            )
            .unwrap(),
            named_before_flac
        );

        // 2. Zero-window HTTP DELETE
        let app_zero = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()),
            shell,
            Duration::ZERO,
        );

        let zero_res = app_zero
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/app/transcripts/api/segment/{day}/_default/{key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("zero delete response");
        assert_eq!(zero_res.status(), StatusCode::OK);
        let zero_bytes = to_bytes(zero_res.into_body(), usize::MAX).await.unwrap();
        let zero_json: Value = serde_json::from_slice(&zero_bytes).unwrap();
        assert_eq!(zero_json["deleted"], key);

        let physical_dir = root.path().join(format!("chronicle/{day}/{key}"));
        assert!(physical_dir.join(TOMBSTONE_NAME).is_file());
        let tombstone_text = fs::read_to_string(physical_dir.join(TOMBSTONE_NAME)).unwrap();
        let tombstone_json: Value = serde_json::from_str(&tombstone_text).unwrap();
        let tombstone_manifest = tombstone_json["manifest"]
            .as_array()
            .expect("manifest array");
        assert!(tombstone_manifest.iter().any(|p| {
            let s = p.as_str().unwrap();
            s.contains("070000_17/audio.flac") && !s.contains(named_stream)
        }));
        assert!(
            !tombstone_manifest
                .iter()
                .any(|p| p.as_str().unwrap().contains(named_stream))
        );

        let ref_date = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let search_direct = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in direct target",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search direct");
        assert_eq!(search_direct.results.len(), 0);

        let search_named = solstone_core_indexer_query::search(
            root.path(),
            solstone_core_indexer_query::OwnerBoundary,
            &solstone_core_indexer_query::SearchRequest::new(
                "needle in named sibling",
                solstone_core_indexer_query::Order::Relevance,
            ),
            ref_date,
        )
        .expect("search named");
        assert_eq!(search_named.results.len(), 1);
        assert_eq!(
            fs::read(
                root.path()
                    .join(format!("chronicle/{day}/{named_stream}/{key}/audio.flac"))
            )
            .unwrap(),
            named_before_flac
        );
    }

    #[test]
    fn changed_flac_refuses_on_resume() {
        let root = TempDir::new().expect("journal");
        let day = "20260805";
        let key = "070000_17";
        let named_stream = "field.audio";
        setup_direct_and_named(root.path(), day, key, named_stream);

        let direct_manifest =
            SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();

        write(
            root.path(),
            &format!("chronicle/{day}/{key}/audio.flac"),
            b"modified bytes",
        );

        let pending_id = "2".repeat(32);
        let record = Record::pending(
            pending_id.clone(),
            SegmentTarget {
                day: day.to_string(),
                stream: solstone_core_journal_io::DEFAULT_STREAM.to_string(),
                key: key.to_string(),
                manifest: direct_manifest,
            },
            Duration::from_secs(10),
        );
        STORE.write(root.path(), &record).expect("write record");

        assert!(resumable(&record));

        commit_delete(root.path(), &pending_id, true);

        let settled = STORE
            .read::<SegmentTarget>(root.path(), &pending_id)
            .unwrap();
        assert_eq!(settled.state, DeleteState::NotDeleted);
        assert!(settled.reason.as_deref().unwrap().contains(CHANGED_REASON));
        assert_eq!(
            fs::read(
                root.path()
                    .join(format!("chronicle/{day}/{key}/audio.flac"))
            )
            .unwrap(),
            b"modified bytes"
        );
    }

    #[test]
    fn resume_direct_segment_cases() {
        let day = "20260805";
        let key = "070000_17";
        let named_stream = "field.audio";

        // 1. Post-claim, live unchanged
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
            let pending_id = "11".repeat(16);
            let mut record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            record.started_at = Some(Utc::now().to_rfc3339());
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::Deleted);
            let physical_dir = root.path().join(format!("chronicle/{day}/{key}"));
            assert!(physical_dir.join(TOMBSTONE_NAME).is_file());
            assert!(
                !root
                    .path()
                    .join(format!("chronicle/{day}/_default"))
                    .exists()
            );
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{named_stream}/{key}/audio.flac"))
                )
                .unwrap(),
                b"named flac"
            );
        }

        // 2. Staged before removal
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
            let staged_dir =
                crate::segment_media::physical_staged_dir(root.path(), day, "_default", key);
            fs::rename(
                root.path().join(format!("chronicle/{day}/{key}")),
                &staged_dir,
            )
            .unwrap();
            let pending_id = "22".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::Deleted);
            let physical_dir = root.path().join(format!("chronicle/{day}/{key}"));
            assert!(physical_dir.join(TOMBSTONE_NAME).is_file());
            assert!(!staged_dir.exists());
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{named_stream}/{key}/audio.flac"))
                )
                .unwrap(),
                b"named flac"
            );
        }

        // 3. Mid-removal: two direct flacs, hardlink outside, remove_segments, rename to staged, hardlink back
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            write(
                root.path(),
                &format!("chronicle/{day}/{key}/audio2.flac"),
                b"direct flac 2",
            );
            let outside = root.path().join("outside.flac");
            fs::hard_link(
                root.path()
                    .join(format!("chronicle/{day}/{key}/audio.flac")),
                &outside,
            )
            .unwrap();
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();

            let target = Target {
                day: day.into(),
                stream: "_default".into(),
                dir: key.into(),
            };
            let _outcome = door::remove_segments(
                root.path(),
                &[target],
                "2026-08-05T12:00:00Z",
                RemovalReason::OwnerSegmentDelete,
                "cid-test",
            );

            let live_dir = root.path().join(format!("chronicle/{day}/{key}"));
            let staged_dir =
                crate::segment_media::physical_staged_dir(root.path(), day, "_default", key);
            fs::rename(&live_dir, &staged_dir).unwrap();
            fs::hard_link(&outside, staged_dir.join("audio.flac")).unwrap();

            let other_staged = root
                .path()
                .join(format!("chronicle/{day}/.removing_999999_99"));
            fs::create_dir_all(&other_staged).unwrap();
            fs::write(other_staged.join("dummy.txt"), b"other staged").unwrap();

            let pending_id = "33".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::Deleted);
            assert!(live_dir.join(TOMBSTONE_NAME).is_file());
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{named_stream}/{key}/audio.flac"))
                )
                .unwrap(),
                b"named flac"
            );
            assert!(other_staged.join("dummy.txt").is_file());
        }

        // 4. Unchanged live direct segment commits
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
            let pending_id = "44".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::Deleted);
            let physical_dir = root.path().join(format!("chronicle/{day}/{key}"));
            assert!(physical_dir.join(TOMBSTONE_NAME).is_file());
        }

        // 5. Added extra flac refuses (CHANGED_REASON)
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
            write(
                root.path(),
                &format!("chronicle/{day}/{key}/extra.flac"),
                b"extra flac",
            );
            let pending_id = "55".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::NotDeleted);
            assert!(settled.reason.as_deref().unwrap().contains(CHANGED_REASON));
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{key}/audio.flac"))
                )
                .unwrap(),
                b"direct flac"
            );
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{key}/extra.flac"))
                )
                .unwrap(),
                b"extra flac"
            );
        }

        // 6. Rewritten confirmed flac refuses (CHANGED_REASON)
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
            write(
                root.path(),
                &format!("chronicle/{day}/{key}/audio.flac"),
                b"rewritten flac",
            );
            let pending_id = "66".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::NotDeleted);
            assert!(settled.reason.as_deref().unwrap().contains(CHANGED_REASON));
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{key}/audio.flac"))
                )
                .unwrap(),
                b"rewritten flac"
            );
        }

        // 7. Empty manifest and resumed (UNCONFIRMED_REASON)
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let pending_id = "77".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest: SegmentManifest::default(),
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::NotDeleted);
            assert!(
                settled
                    .reason
                    .as_deref()
                    .unwrap()
                    .contains(UNCONFIRMED_REASON)
            );
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{key}/audio.flac"))
                )
                .unwrap(),
                b"direct flac"
            );
        }

        // 8. Live direct directory present beside its staged sibling
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
            let staged_dir =
                crate::segment_media::physical_staged_dir(root.path(), day, "_default", key);
            fs::create_dir_all(&staged_dir).unwrap();
            fs::write(staged_dir.join("old.txt"), b"old staged").unwrap();

            let pending_id = "88".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{key}/audio.flac"))
                )
                .unwrap(),
                b"direct flac"
            );
            assert_eq!(fs::read(staged_dir.join("old.txt")).unwrap(), b"old staged");
        }

        // 9. Confirmed flac missing, no receipt
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest =
                SegmentManifest::of(&root.path().join(format!("chronicle/{day}/{key}"))).unwrap();
            let staged_dir =
                crate::segment_media::physical_staged_dir(root.path(), day, "_default", key);
            fs::rename(
                root.path().join(format!("chronicle/{day}/{key}")),
                &staged_dir,
            )
            .unwrap();
            fs::remove_file(staged_dir.join("audio.flac")).unwrap();
            fs::write(
                staged_dir.join(TOMBSTONE_NAME),
                format!(r#"{{"manifest":["chronicle/{day}/{key}/audio.flac"]}}"#),
            )
            .unwrap();

            let pending_id = "99".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::NotDeleted);
            assert!(settled.reason.as_deref().unwrap().contains(CHANGED_REASON));
            assert!(staged_dir.join("audio.jsonl").is_file());
        }

        // 10. Live directory absent, staged sibling exists and is empty, no receipt
        {
            let root = TempDir::new().expect("journal");
            write(
                root.path(),
                "config/journal.json",
                br#"{"setup":{"completed_at":1700000000000}}"#,
            );
            let staged_dir =
                crate::segment_media::physical_staged_dir(root.path(), day, "_default", key);
            fs::create_dir_all(&staged_dir).unwrap();

            let pending_id = "aa".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: "_default".into(),
                    key: key.into(),
                    manifest: SegmentManifest {
                        files: std::collections::BTreeMap::from([(
                            "audio.flac".to_string(),
                            crate::pending::FileStamp {
                                size: 10,
                                modified_unix_ns: 100,
                                inode: None,
                            },
                        )]),
                    },
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::NotDeleted);
            assert!(settled.reason.as_deref().unwrap().contains(CHANGED_REASON));
            assert!(staged_dir.is_dir());
            assert!(!root.path().join(format!("chronicle/{day}/{key}")).exists());
        }

        // 11. Named-stream delete of sibling removes named segment and leaves direct bytes unchanged
        {
            let root = TempDir::new().expect("journal");
            setup_direct_and_named(root.path(), day, key, named_stream);
            let manifest = SegmentManifest::of(
                &root
                    .path()
                    .join(format!("chronicle/{day}/{named_stream}/{key}")),
            )
            .unwrap();
            let pending_id = "bb".repeat(16);
            let record = Record::pending(
                pending_id.clone(),
                SegmentTarget {
                    day: day.into(),
                    stream: named_stream.into(),
                    key: key.into(),
                    manifest,
                },
                Duration::from_secs(10),
            );
            STORE.write(root.path(), &record).unwrap();
            assert!(resumable(&record));

            commit_delete(root.path(), &pending_id, true);

            let settled = STORE
                .read::<SegmentTarget>(root.path(), &pending_id)
                .unwrap();
            assert_eq!(settled.state, DeleteState::Deleted);
            let named_dir = root
                .path()
                .join(format!("chronicle/{day}/{named_stream}/{key}"));
            assert!(named_dir.join(TOMBSTONE_NAME).is_file());
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!("chronicle/{day}/{key}/audio.flac"))
                )
                .unwrap(),
                b"direct flac"
            );
        }
    }

    #[tokio::test]
    async fn refusals_stay_refused() {
        assert!(!super::valid_stream(""));
        assert!(!super::valid_stream("_defaults"));
        assert!(!super::valid_stream("_Default"));
        assert!(!super::valid_stream("a/b"));
        assert!(!super::valid_stream(".."));

        assert!(!super::valid_key("093000_300_summary"));

        let root = TempDir::new().expect("journal");
        let day = "20260805";
        let key = "070000_17";
        write(
            root.path(),
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
        write(
            root.path(),
            &format!("chronicle/{day}/{key}/audio.flac"),
            b"direct flac",
        );

        let app = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()),
            shell,
            Duration::ZERO,
        );

        // HTTP 404 for unknown segment
        let res_404 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!(
                        "/app/transcripts/api/segment/{day}/_default/090000_99"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("404 response");
        assert_eq!(res_404.status(), StatusCode::NOT_FOUND);

        // HTTP rejection for stream _defaults
        let res_bad = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!(
                        "/app/transcripts/api/segment/{day}/_defaults/{key}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("bad request response");
        assert_eq!(res_bad.status(), StatusCode::BAD_REQUEST);
    }
}
