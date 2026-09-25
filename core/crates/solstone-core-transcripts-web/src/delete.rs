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
use solstone_core_retention::door;
use solstone_core_retention::tombstone::TOMBSTONE_NAME;
use solstone_core_retention::{NoIndex, Outcome, RemovalReason, Target};
use solstone_core_system::lifecycle::SupervisorLiveness;

use crate::deferred::DeferredDeleteRegistry;
use solstone_core_journal_io::LockError;

use crate::pending::{self, DeleteRecord, DeleteState, SegmentManifest};
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

    // One request at a time decides whether a segment already has a delete
    // waiting, so two tabs or a double submit cannot both create one.
    let Ok(_create) = pending::create_lock(&state.journal_root) else {
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
    if let Some(existing) = pending::pending_for(&state.journal_root, &day, &stream, &key) {
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
        return accepted(&state, &existing, true);
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
    let commit_at_ms = Utc::now().timestamp_millis() + state.delete_window.as_millis() as i64;
    let record = DeleteRecord {
        pending_id: pending_id.clone(),
        day,
        stream,
        key,
        requested_at: Utc::now().to_rfc3339(),
        commit_at_ms,
        manifest,
        state: DeleteState::Pending,
        started_at: None,
        reason: None,
        finished_at: None,
    };
    // ⛔ Durable before the answer: a delete the page reports as under way must
    // survive the journal stopping inside its window.
    if let Err(error) = pending::write(&state.journal_root, &record) {
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

    accepted(&state, &record, false)
}

fn accepted(state: &AppState, record: &DeleteRecord, joined: bool) -> Response {
    let (search_index_warning, supervisor_liveness) =
        match (state.supervisor_liveness)(&state.journal_root) {
            SupervisorLiveness::Up => (false, None),
            SupervisorLiveness::Down => (true, None),
            SupervisorLiveness::Unverifiable => (true, Some("unverifiable")),
        };
    let mut body = json!({
        "success": true,
        "deleted": record.key,
        "pending": record.pending_id,
        "commit_at_ms": record.commit_at_ms,
        "ttl_seconds": 10,
    });
    if joined {
        // What is left of the first request's window, not a fresh one.
        let remaining = (record.commit_at_ms - Utc::now().timestamp_millis()).max(0);
        body["ttl_seconds"] = json!((remaining + 999) / 1000);
    }
    if search_index_warning {
        body["search_index_warning"] = json!(true);
    }
    if let Some(liveness) = supervisor_liveness {
        body["supervisor_liveness"] = json!(liveness);
    }
    Json(body).into_response()
}

fn schedule_commit(
    registry: &DeferredDeleteRegistry,
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

/// Resume every delete a previous run confirmed and never finished.
///
/// Called once per router, at start. A record still inside its window waits
/// out the rest of it and can still be cancelled under the same id; a record
/// whose window passed while the journal was stopped runs straight away,
/// because the owner confirmed it and let the window go by.
///
/// The router is built before Convey's runtime starts, so the wait runs on its
/// own thread rather than as a runtime task. If that thread cannot start the
/// records stay pending on disk and the next start tries again.
pub(crate) fn resume_pending(journal_root: &Path, registry: &DeferredDeleteRegistry) {
    // The records live in `config/`, which travels with backups: resume only
    // what names a segment the delete route itself would have accepted.
    let records = pending::pending_and_prune(journal_root, Utc::now())
        .into_iter()
        .filter(resumable)
        .collect::<Vec<_>>();
    if records.is_empty() {
        return;
    }
    for record in &records {
        registry.hold(record.pending_id.clone());
    }
    let root = journal_root.to_path_buf();
    let waiter = registry.clone();
    let _ = std::thread::Builder::new()
        .name("segment-delete-resume".into())
        .spawn(move || {
            for record in records {
                let wait = record.commit_at_ms - Utc::now().timestamp_millis();
                if let Ok(wait) = u64::try_from(wait) {
                    std::thread::sleep(Duration::from_millis(wait));
                }
                if waiter.claim(&record.pending_id) {
                    commit_delete(&root, &record.pending_id, true);
                }
            }
        });
}

/// Whether a record names a segment the delete route itself would accept.
fn resumable(record: &DeleteRecord) -> bool {
    valid_pending_id(&record.pending_id)
        && valid_day(&record.day)
        && valid_stream(&record.stream)
        && valid_key(&record.key)
}

pub(crate) async fn cancel_delete(
    State(state): State<Arc<AppState>>,
    RoutePath(pending_id): RoutePath<String>,
) -> Response {
    let root = state.journal_root.as_path().to_path_buf();
    // An id this journal never issued changes nothing, not even a lock file.
    if !valid_pending_id(&pending_id) || pending::read(&root, &pending_id).is_none() {
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

enum Settled {
    /// A commit holds the record: it is removing now.
    Busy,
    Failed(String),
    Record(Box<DeleteRecord>),
}

fn settle_cancel(journal_root: &Path, pending_id: &str) -> Settled {
    let _lock = match pending::lock(journal_root, pending_id, CANCEL_LOCK_WAIT) {
        Ok(lock) => lock,
        Err(LockError::Timeout(_)) => return Settled::Busy,
        Err(error) => return Settled::Failed(error.to_string()),
    };
    let Some(mut record) = pending::read(journal_root, pending_id) else {
        return Settled::Failed("the record could not be read".to_owned());
    };
    if record.state != DeleteState::Pending || record.started_at.is_some() {
        return Settled::Record(Box::new(record));
    }
    record.finish(DeleteState::Cancelled, None);
    if let Err(error) = pending::write(journal_root, &record) {
        return Settled::Failed(error);
    }
    // Intentional Python divergence: this writer files by Local::now(), not segment day.
    let _ = solstone_core_facets::append_action_log(
        journal_root,
        None,
        "app",
        "transcripts",
        "segment_delete",
        json!({"pending_id":pending_id,"phase":"cancelled"}),
    );
    Settled::Record(Box::new(record))
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
        .then(|| pending::read(&state.journal_root, &pending_id))
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
    let outcomes = pending::unfinished_outcomes(&state.journal_root, Utc::now())
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
        "day": record.day,
        "stream": record.stream,
        "segment_key": record.key,
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
            day: record.day.clone(),
            stream: record.stream.clone(),
            key: record.key.clone(),
            pending_id: record.pending_id.clone(),
        }
    }
}

/// How long a commit waits for its record. Contention means another commit of
/// the same delete holds it; the record stays pending and the next start
/// retries if that one did not finish.
const COMMIT_LOCK_WAIT: Duration = Duration::from_secs(5);
/// How long a cancel waits. A commit holds the lock while it removes, so a
/// cancel that cannot take it promptly is too late.
const CANCEL_LOCK_WAIT: Duration = Duration::from_millis(200);

/// Run one delete to its outcome, holding the record's lock from the claim to
/// the written result, so no other commit or cancel can interleave.
fn commit_delete(journal_root: &Path, pending_id: &str, resumed: bool) {
    let Ok(_lock) = pending::lock(journal_root, pending_id, COMMIT_LOCK_WAIT) else {
        return;
    };
    let Some(mut record) = pending::read(journal_root, pending_id) else {
        return;
    };
    if record.state != DeleteState::Pending {
        return;
    }
    // A claim left by a run that stopped part-way is taken over as a resume.
    let resumed = resumed || record.started_at.is_some();
    record.started_at = Some(Utc::now().to_rfc3339());
    if pending::write(journal_root, &record).is_err() {
        return;
    }
    let Some((phase, detail, state, reason)) = run_delete(journal_root, &record, resumed) else {
        // Another finisher holds the segment. Leave the record pending, with
        // its claim, so the next run settles it from what is then on disk.
        return;
    };
    let marker = if resumed {
        json!({"resumed":true})
    } else {
        json!({})
    };
    append_action(
        journal_root,
        &DeleteRequest::from(&record),
        phase,
        merge(marker, detail),
    );
    // Written last: a stop before this line leaves the record pending, and the
    // next start finds the tombstone and reports the delete as done.
    record.finish(state, reason.map(str::to_owned));
    let _ = pending::write(journal_root, &record);
}

fn run_delete(
    journal_root: &Path,
    record: &DeleteRecord,
    resumed: bool,
) -> Option<(&'static str, Value, DeleteState, Option<&'static str>)> {
    let target = Target {
        day: record.day.clone(),
        stream: record.stream.clone(),
        dir: record.key.clone(),
    };
    let segment_rel = format!("chronicle/{}/{}/{}", record.day, record.stream, record.key);
    let segment_dir = journal_root.join(&segment_rel);
    let deleted_at = Utc::now().to_rfc3339();

    // A previous run removed it and stopped before it could say so.
    let already_removed = || {
        segment_dir.join(TOMBSTONE_NAME).is_file().then(|| {
            (
                "committed",
                json!({"already_removed":true}),
                DeleteState::Deleted,
                None,
            )
        })
    };
    if let Some(done) = already_removed() {
        return Some(done);
    }
    // A previous run set it aside and stopped part-way: finish exactly that.
    // Only when nothing is back at the live name, which is the state the door
    // leaves mid-removal; a live segment beside an old leftover goes to the
    // door, which refuses it untouched.
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
        let _ = door::notify_index(&NoIndex, &outcome);
        let (phase, detail) = terminal_detail(&outcome);
        return Some((
            phase,
            merge(json!({"recovered":true}), detail),
            owner_outcome(&outcome, &segment_dir),
            None,
        ));
    }
    // Someone else may have finished it while this run waited for the lock.
    if let Some(done) = already_removed() {
        return Some(done);
    }
    if let Some(reason) = unmatched_reason(record, &segment_dir, resumed) {
        return Some((
            "refused",
            json!({"refused":[{"entry":segment_rel,"reason":reason,"staged":null}]}),
            DeleteState::NotDeleted,
            Some(reason),
        ));
    }
    let outcome = door::remove_segments(
        journal_root,
        &[target],
        &deleted_at,
        RemovalReason::OwnerSegmentDelete,
        "unknown",
    );
    // The chronicle is authoritative: this notification deliberately happens
    // only after the removal door has returned its proven outcome.
    let _ = door::notify_index(&NoIndex, &outcome);
    let (phase, detail) = terminal_detail(&outcome);
    Some((phase, detail, owner_outcome(&outcome, &segment_dir), None))
}

fn staged_dir(journal_root: &Path, record: &DeleteRecord) -> std::path::PathBuf {
    journal_root
        .join("chronicle")
        .join(&record.day)
        .join(&record.stream)
        .join(solstone_core_retention::staged_name(&record.key))
}

/// Whether anything is at `path`. An error reading it counts as present, so an
/// unreadable segment is never taken for a removed one.
fn present(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

// A door refusal carries no owner reason: its text can name paths and OS
// errors, so it stays in the action log and the page says the state in owner
// words.

/// `Some` when the segment under this name no longer holds the files the owner
/// confirmed, or they cannot be checked. Checked on every commit: within the
/// window the same key can be re-imported too. A segment with nothing to
/// compare is refused only on resume, where the window has stretched. A
/// missing segment is left to the door.
fn unmatched_reason(
    record: &DeleteRecord,
    segment_dir: &Path,
    resumed: bool,
) -> Option<&'static str> {
    if !segment_dir.is_dir() {
        return None;
    }
    if record.manifest.files.is_empty() {
        return resumed.then_some(UNCONFIRMED_REASON);
    }
    match record.manifest.still_held_by(segment_dir) {
        Ok(true) => None,
        Ok(false) => Some(CHANGED_REASON),
        Err(_) => Some(UNCONFIRMED_REASON),
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
