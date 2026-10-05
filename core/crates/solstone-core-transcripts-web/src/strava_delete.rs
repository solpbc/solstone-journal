// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner deletes for Strava workouts.
//!
//! A Strava workout lives in the journal as consecutive pieces of about five
//! minutes under `import.strava`, each holding one `workout.json` that names the
//! workout (`activity_id`) and the import that brought it in (`import_id`).
//! There are two deletes, both held for the owner's cancel window like the
//! segment delete and resumed at start:
//!
//! - **one workout**: every piece with its `activity_id`, on every day it covers,
//!   removed as an owner delete, so a later import of any download leaves it out;
//! - **one whole import**: every piece that import brought in, removed with the
//!   release reason, then the import's own records. Importing that download again
//!   brings its workouts back, one key past where they were; a piece the owner
//!   deleted on its own still stays deleted.
//!
//! The pieces are resolved when the owner confirms and recorded with the delete.
//! The removal runs under the Strava importer's lock, so an import can't place a
//! piece while a delete is removing them.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_journal_io::{LockOptions, hold_lock};
use solstone_core_retention::door;
use solstone_core_retention::tombstone::TOMBSTONE_NAME;
use solstone_core_retention::{Outcome, RemovalReason, Target};
use solstone_core_serving::held_delete::{
    DeleteState, Record, Registry, Settled, Store, valid_pending_id,
};

use crate::{AppState, legacy_error_response};

const STORE: Store = Store::new("config/strava-deletes");
const STREAM: &str = "import.strava";
/// The Strava importer's own lock: an import and a delete never overlap.
const STRAVA_LOCK: &str = "imports/.strava";
const BATCH: usize = 500;
/// How long a delete waits for a running Strava import before giving up.
#[cfg(not(test))]
const LOCK_WAIT: Duration = Duration::from_secs(10);
#[cfg(test)]
const LOCK_WAIT: Duration = Duration::from_millis(50);
const BUSY_REASON: &str =
    "a Strava import was running, so nothing was deleted. try again when it finishes.";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Workout,
    Import,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Piece {
    pub(crate) day: String,
    pub(crate) key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct StravaTarget {
    pub(crate) kind: Kind,
    pub(crate) id: String,
    pub(crate) pieces: Vec<Piece>,
}

type StravaRecord = Record<StravaTarget>;

/// Every live piece of one workout or one import, in day and key order. A piece
/// whose `workout.json` is missing (one already deleted) is skipped; one that
/// can't be read is an error, so a delete never reports more than it found.
pub(crate) fn pieces_of(journal: &Path, kind: Kind, id: &str) -> std::io::Result<Vec<Piece>> {
    let days = solstone_core_journal_io::day_dirs(journal)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut days = days.into_iter().collect::<Vec<_>>();
    days.sort();
    let mut pieces = Vec::new();
    for (day, dir) in days {
        let stream_dir = dir.join(STREAM);
        let entries = match std::fs::read_dir(&stream_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.file_type()?.is_dir() {
                continue;
            }
            let bytes = match std::fs::read(entry.path().join("workout.json")) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let matches = match kind {
                Kind::Workout => value
                    .get("activity_id")
                    .and_then(Value::as_u64)
                    .is_some_and(|found| found.to_string() == id),
                Kind::Import => value.get("import_id").and_then(Value::as_str) == Some(id),
            };
            if matches {
                keys.push(name);
            }
        }
        keys.sort();
        pieces.extend(keys.into_iter().map(|key| Piece {
            day: day.clone(),
            key,
        }));
    }
    Ok(pieces)
}

fn valid_workout_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

fn valid_import_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether an import id names a Strava import this journal still lists.
fn is_strava_import(journal: &Path, id: &str) -> bool {
    let dir = journal.join("imports").join(id);
    let read = |name: &str| {
        std::fs::read(dir.join(name))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    };
    let named_strava =
        |value: &Value, field: &str| value.get(field).and_then(Value::as_str) == Some("strava");
    read("manifest.json").is_some_and(|v| named_strava(&v, "source_type"))
        || read("imported.json").is_some_and(|v| {
            v.get("segments")
                .and_then(Value::as_array)
                .is_some_and(|segments| {
                    segments
                        .iter()
                        .any(|s| s.get("stream").and_then(Value::as_str) == Some(STREAM))
                })
        })
}

pub(crate) async fn delete_workout(
    State(state): State<Arc<AppState>>,
    RoutePath(activity_id): RoutePath<String>,
) -> Response {
    if !valid_workout_id(&activity_id) {
        return bad_request("that isn't a workout in your journal.");
    }
    request(&state, Kind::Workout, activity_id).await
}

pub(crate) async fn delete_import(
    State(state): State<Arc<AppState>>,
    RoutePath(import_id): RoutePath<String>,
) -> Response {
    if !valid_import_id(&import_id) {
        return bad_request("that isn't an import in your journal.");
    }
    request(&state, Kind::Import, import_id).await
}

async fn request(state: &AppState, kind: Kind, id: String) -> Response {
    let root = state.journal_root.clone();
    let lookup_id = id.clone();
    let resolved = tokio::task::spawn_blocking(move || pieces_of(&root, kind, &lookup_id)).await;
    let pieces = match resolved {
        Ok(Ok(pieces)) => pieces,
        _ => {
            return legacy_error_response(
                "strava_delete_unreadable",
                "your journal couldn't read those workouts, so nothing was deleted.",
                "Failed to read the Strava pieces",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    match kind {
        Kind::Workout if pieces.is_empty() => {
            return not_found("that workout isn't in your journal.");
        }
        Kind::Import if !is_strava_import(&state.journal_root, &id) => {
            if state.journal_root.join("imports").join(&id).is_dir() {
                return legacy_error_response(
                    "strava_delete_not_deletable",
                    "only a Strava import can be deleted here.",
                    "not a Strava import",
                    StatusCode::CONFLICT,
                );
            }
            return not_found("that import isn't in your journal.");
        }
        _ => {}
    }

    let Ok(_create) = STORE.create_lock(&state.journal_root) else {
        return not_saved("Failed to take the delete lock");
    };
    let waiting = STORE.waiting_for(&state.journal_root, |target: &StravaTarget| {
        target.kind == kind && target.id == id
    });
    if let Some(existing) = waiting {
        if !state.deferred_deletes.contains(&existing.pending_id) {
            let remaining = existing.commit_at_ms - Utc::now().timestamp_millis();
            let delay = Duration::from_millis(u64::try_from(remaining).unwrap_or(0).max(1));
            schedule(
                &state.deferred_deletes,
                &state.journal_root,
                &existing.pending_id,
                delay,
                true,
            );
        }
        return accepted(&existing);
    }
    let pending_id = match pending_id() {
        Ok(id) => id,
        Err(error) => return not_saved(&error),
    };
    let record = Record::pending(
        pending_id,
        StravaTarget { kind, id, pieces },
        state.delete_window,
    );
    if let Err(error) = STORE.write(&state.journal_root, &record) {
        return not_saved(&format!("Failed to save the pending delete: {error}"));
    }
    append_action(&state.journal_root, &record, "pending", json!({}));
    schedule(
        &state.deferred_deletes,
        &state.journal_root,
        &record.pending_id,
        state.delete_window,
        false,
    );
    accepted(&record)
}

fn accepted(record: &StravaRecord) -> Response {
    Json(json!({
        "success": true,
        "pending": record.pending_id,
        "commit_at_ms": record.commit_at_ms,
        "ttl_seconds": record.remaining_seconds(),
        "pieces": record.target.pieces.len(),
    }))
    .into_response()
}

fn schedule(registry: &Registry, journal: &Path, pending_id: &str, delay: Duration, resumed: bool) {
    let root = journal.to_path_buf();
    let id = pending_id.to_owned();
    registry.schedule(pending_id.to_owned(), delay, move || {
        commit(&root, &id, resumed)
    });
}

/// Resume every Strava delete a previous run confirmed and never finished.
pub(crate) fn resume_pending(journal: &Path, registry: &Registry) {
    STORE.resume(
        journal,
        registry,
        "strava-delete-resume",
        |record: &StravaRecord| {
            valid_pending_id(&record.pending_id)
                && match record.target.kind {
                    Kind::Workout => valid_workout_id(&record.target.id),
                    Kind::Import => valid_import_id(&record.target.id),
                }
        },
        |root, pending_id| commit(root, pending_id, true),
    );
}

fn commit(journal: &Path, pending_id: &str, resumed: bool) {
    STORE.commit(
        journal,
        pending_id,
        resumed,
        |record: &StravaRecord, claim| {
            let (state, reason, detail) = run(journal, record, claim.removal_started)?;
            let detail = if claim.resumed {
                merge(json!({"resumed": true}), detail)
            } else {
                detail
            };
            append_action(journal, record, state.as_str(), detail);
            Some((state, reason))
        },
    );
}

/// Whether a piece's directory now holds only its tombstone.
fn removed(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    let names = entries
        .filter_map(|entry| entry.ok().map(|e| e.file_name()))
        .collect::<Vec<_>>();
    names.len() == 1 && names[0] == TOMBSTONE_NAME
}

fn run(
    journal: &Path,
    record: &StravaRecord,
    removal_started: bool,
) -> Option<(DeleteState, Option<String>, Value)> {
    let _lock = match hold_lock(
        journal.join(STRAVA_LOCK),
        LockOptions {
            timeout: LOCK_WAIT,
            ..LockOptions::default()
        },
    ) {
        Ok(lock) => lock,
        // Part of it already ran: leave it pending so the next start finishes it.
        Err(_) if removal_started => return None,
        Err(_) => {
            return Some((
                DeleteState::NotDeleted,
                Some(BUSY_REASON.to_owned()),
                json!({"busy": true}),
            ));
        }
    };
    if STORE.mark_removal_started(journal, record).is_err() {
        return None;
    }
    let target = &record.target;
    let reason = match target.kind {
        Kind::Workout => RemovalReason::OwnerSegmentDelete,
        Kind::Import => RemovalReason::ImportRunRelease,
    };
    let cid = format!("strava-delete:{}", record.pending_id);
    let deleted_at = Utc::now().to_rfc3339();
    let stream_dir = |day: &str| journal.join("chronicle").join(day).join(STREAM);

    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    let mut live = Vec::new();
    for piece in &target.pieces {
        let door_target = Target {
            day: piece.day.clone(),
            stream: STREAM.to_owned(),
            dir: piece.key.clone(),
        };
        let dir = stream_dir(&piece.day);
        if dir.join(format!(".removing_{}", piece.key)).exists() {
            // This delete stopped partway through this piece: finish it.
            if let Some(row) =
                door::recover_segment(journal, &door_target, &deleted_at, reason, &cid)
            {
                outcome.targets.push(row);
            }
        } else if dir.join(&piece.key).is_dir() && !removed(&dir.join(&piece.key)) {
            live.push(door_target);
        }
    }
    for batch in live.chunks(BATCH) {
        let batch_outcome = door::remove_segments(journal, batch, &deleted_at, reason, &cid);
        outcome.targets.extend(batch_outcome.targets);
        if batch_outcome.halted.is_some() {
            outcome.halted = batch_outcome.halted;
            break;
        }
    }
    let notify = door::notify_index(&RetentionIndex::new(journal), &outcome);

    let gone = target
        .pieces
        .iter()
        .filter(|piece| removed(&stream_dir(&piece.day).join(&piece.key)))
        .count();
    let left = target.pieces.len() - gone;
    let mut detail = json!({
        "kind": target.kind,
        "id": target.id,
        "pieces": target.pieces.len(),
        "removed": gone,
        "not_removed": left,
    });
    if notify.is_err() {
        detail["search_not_updated"] = json!(true);
    }
    if left > 0 {
        let state = if gone > 0 {
            DeleteState::Incomplete
        } else {
            DeleteState::NotDeleted
        };
        let reason = match target.kind {
            Kind::Workout => format!(
                "{left} of its {} pieces couldn't be deleted. try again.",
                target.pieces.len()
            ),
            Kind::Import => {
                let workouts = target
                    .pieces
                    .iter()
                    .filter(|piece| !removed(&stream_dir(&piece.day).join(&piece.key)))
                    .filter_map(|piece| {
                        std::fs::read(stream_dir(&piece.day).join(&piece.key).join("workout.json"))
                            .ok()
                            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                            .and_then(|v| v.get("activity_id").and_then(Value::as_u64))
                    })
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    .max(1);
                format!(
                    "{workouts} {} couldn't be fully deleted. try again.",
                    if workouts == 1 { "workout" } else { "workouts" }
                )
            }
        };
        return Some((state, Some(reason), detail));
    }
    if target.kind == Kind::Import
        && let Err(error) = solstone_core_import::remove_import_records(journal, &target.id)
    {
        detail["records_kept"] = json!(true);
        detail["records_error"] = json!(format!("{error:?}"));
        return Some((
            DeleteState::Incomplete,
            Some(
                "its workouts were deleted, but this import's page couldn't be removed. try again."
                    .to_owned(),
            ),
            detail,
        ));
    }
    Some((DeleteState::Deleted, None, detail))
}

pub(crate) async fn preview_workout(
    State(state): State<Arc<AppState>>,
    RoutePath(activity_id): RoutePath<String>,
) -> Response {
    if !valid_workout_id(&activity_id) {
        return bad_request("that isn't a workout in your journal.");
    }
    preview(&state, Kind::Workout, activity_id).await
}

pub(crate) async fn preview_import(
    State(state): State<Arc<AppState>>,
    RoutePath(import_id): RoutePath<String>,
) -> Response {
    if !valid_import_id(&import_id) {
        return bad_request("that isn't an import in your journal.");
    }
    preview(&state, Kind::Import, import_id).await
}

/// What a delete would remove right now: its pieces, the workouts they belong to
/// and the days they cover.
async fn preview(state: &AppState, kind: Kind, id: String) -> Response {
    let root = state.journal_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        let pieces = pieces_of(&root, kind, &id)?;
        let mut workouts = std::collections::BTreeSet::new();
        let mut days = std::collections::BTreeSet::new();
        for piece in &pieces {
            days.insert(piece.day.clone());
            let path = root
                .join("chronicle")
                .join(&piece.day)
                .join(STREAM)
                .join(&piece.key)
                .join("workout.json");
            if let Some(activity) = std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|v| v.get("activity_id").and_then(Value::as_u64))
            {
                workouts.insert(activity);
            }
        }
        std::io::Result::Ok(json!({
            "pieces": pieces.len(),
            "workouts": workouts.len(),
            "days": days.len(),
            "first_day": days.first(),
            "last_day": days.last(),
        }))
    })
    .await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => legacy_error_response(
            "strava_delete_unreadable",
            "your journal couldn't read those workouts.",
            "Failed to read the Strava pieces",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub(crate) async fn cancel(
    State(state): State<Arc<AppState>>,
    RoutePath(pending_id): RoutePath<String>,
) -> Response {
    let root = state.journal_root.clone();
    if !valid_pending_id(&pending_id) || STORE.read::<StravaTarget>(&root, &pending_id).is_none() {
        return not_found("your journal has no record of that delete.");
    }
    let id = pending_id.clone();
    let settled = tokio::task::spawn_blocking(move || {
        STORE.settle_cancel::<StravaTarget>(&root, &id, || {
            let _ = solstone_core_facets::append_action_log(
                &root,
                None,
                "app",
                "transcripts",
                "strava_delete",
                json!({"pending_id": id, "phase": "cancelled"}),
            );
        })
    })
    .await;
    match settled {
        Ok(Settled::Record(record)) if record.state == DeleteState::Cancelled => {
            state.deferred_deletes.cancel(&pending_id);
            Json(json!({"cancelled": pending_id})).into_response()
        }
        Ok(Settled::Record(record)) => Json(status_body(&record)).into_response(),
        _ => legacy_error_response(
            "strava_delete_not_cancelled",
            "that delete couldn't be cancelled, so it will still happen.",
            "Failed to cancel",
            StatusCode::CONFLICT,
        ),
    }
}

pub(crate) async fn status(
    State(state): State<Arc<AppState>>,
    RoutePath(pending_id): RoutePath<String>,
) -> Response {
    match valid_pending_id(&pending_id)
        .then(|| STORE.read::<StravaTarget>(&state.journal_root, &pending_id))
        .flatten()
    {
        Some(record) => Json(status_body(&record)).into_response(),
        None => not_found("your journal has no record of that delete."),
    }
}

pub(crate) async fn outcomes(State(state): State<Arc<AppState>>) -> Response {
    let outcomes = STORE
        .unfinished_outcomes::<StravaTarget>(&state.journal_root, Utc::now())
        .iter()
        .map(status_body)
        .collect::<Vec<_>>();
    Json(json!({ "outcomes": outcomes })).into_response()
}

fn status_body(record: &StravaRecord) -> Value {
    let mut body = json!({
        "pending": record.pending_id,
        "state": record.state.as_str(),
        "kind": record.target.kind,
        "id": record.target.id,
        "pieces": record.target.pieces.len(),
        "commit_at_ms": record.commit_at_ms,
    });
    if let Some(reason) = &record.reason {
        body["reason"] = json!(reason);
    }
    body
}

fn append_action(journal: &Path, record: &StravaRecord, phase: &str, detail: Value) {
    let _ = solstone_core_facets::append_action_log(
        journal,
        None,
        "app",
        "transcripts",
        "strava_delete",
        merge(
            json!({
                "pending_id": record.pending_id,
                "kind": record.target.kind,
                "id": record.target.id,
                "phase": phase,
            }),
            detail,
        ),
    );
}

fn merge(mut base: Value, extra: Value) -> Value {
    if let (Value::Object(base), Value::Object(extra)) = (&mut base, extra) {
        base.extend(extra);
    }
    base
}

fn pending_id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn bad_request(message: &str) -> Response {
    legacy_error_response(
        "strava_delete_invalid",
        message,
        "invalid id",
        StatusCode::BAD_REQUEST,
    )
}

fn not_found(message: &str) -> Response {
    legacy_error_response(
        "strava_delete_unknown",
        message,
        "not found",
        StatusCode::NOT_FOUND,
    )
}

fn not_saved(detail: &str) -> Response {
    legacy_error_response(
        "strava_delete_not_saved",
        "your journal couldn't start that delete, so nothing was deleted.",
        detail.to_owned(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
    use solstone_core_journal_io::{LockOptions, hold_lock};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::{STORE, StravaTarget, commit};
    use crate::{Clock, router_with_delete_window};
    use solstone_core_serving::held_delete::DeleteState;

    fn shell() -> axum::response::Response {
        axum::response::Response::new(Body::from("shell"))
    }

    fn piece(root: &Path, day: &str, key: &str, activity: u64, import: &str) {
        let dir = root
            .join("chronicle")
            .join(day)
            .join("import.strava")
            .join(key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("workout.json"),
            json!({"schema":"solstone.import.strava.tile.v1","activity_id":activity,"import_id":import})
                .to_string(),
        )
        .unwrap();
        fs::write(dir.join("stream.json"), b"{}").unwrap();
    }

    fn journal() -> TempDir {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .unwrap();
        // Workout 101 crosses midnight; 102 shares its first day; 201 is another import's.
        piece(root.path(), "20260810", "235500_300", 101, "imp_a");
        piece(root.path(), "20260811", "000000_300", 101, "imp_a");
        piece(root.path(), "20260810", "090000_300", 102, "imp_a");
        piece(root.path(), "20260810", "120000_300", 201, "imp_b");
        for (id, source) in [
            ("imp_a", "strava"),
            ("imp_b", "strava"),
            ("notes_1", "obsidian"),
        ] {
            let dir = root.path().join("imports").join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("manifest.json"),
                json!({"source_type": source}).to_string(),
            )
            .unwrap();
        }
        root
    }

    async fn delete(root: &Path, uri: &str) -> (StatusCode, Value) {
        let app = router_with_delete_window(
            root.to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 12, 12, 0, 0).unwrap()),
            shell,
            Duration::from_secs(600),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn tombstone(root: &Path, day: &str, key: &str) -> Option<Value> {
        let dir = root
            .join("chronicle")
            .join(day)
            .join("import.strava")
            .join(key);
        let names = fs::read_dir(&dir).ok()?.count();
        (names == 1).then(|| {
            serde_json::from_slice(&fs::read(dir.join("tombstone.json")).unwrap()).unwrap()
        })
    }

    #[tokio::test]
    async fn a_workout_delete_takes_every_piece_on_every_day_and_nothing_else() {
        let root = journal();
        let (status, body) = delete(root.path(), "/app/transcripts/api/strava/workout/101").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["pieces"], 2);
        let id = body["pending"].as_str().unwrap().to_owned();
        commit(root.path(), &id, false);

        for (day, key) in [("20260810", "235500_300"), ("20260811", "000000_300")] {
            let stone = tombstone(root.path(), day, key).expect("only a tombstone left");
            assert_eq!(stone["reason"], "owner_segment_delete");
            assert_eq!(stone["cid"], format!("strava-delete:{id}"));
        }
        assert!(tombstone(root.path(), "20260810", "090000_300").is_none());
        let record = STORE.read::<StravaTarget>(root.path(), &id).unwrap();
        assert_eq!(record.state, DeleteState::Deleted);
        assert!(
            root.path().join("imports/imp_a").is_dir(),
            "a workout delete keeps the import"
        );
    }

    #[tokio::test]
    async fn an_import_delete_releases_its_pieces_and_removes_its_records() {
        let root = journal();
        let (status, body) = delete(root.path(), "/app/transcripts/api/strava/import/imp_a").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["pieces"], 3);
        let id = body["pending"].as_str().unwrap().to_owned();
        commit(root.path(), &id, false);

        for (day, key) in [
            ("20260810", "235500_300"),
            ("20260811", "000000_300"),
            ("20260810", "090000_300"),
        ] {
            let stone = tombstone(root.path(), day, key).expect("only a tombstone left");
            assert_eq!(stone["reason"], "import_run_release");
        }
        assert!(tombstone(root.path(), "20260810", "120000_300").is_none());
        assert!(!root.path().join("imports/imp_a").exists());
        assert!(root.path().join("imports/imp_b").is_dir());
        let record = STORE.read::<StravaTarget>(root.path(), &id).unwrap();
        assert_eq!(record.state, DeleteState::Deleted);
    }

    #[tokio::test]
    async fn only_a_strava_import_or_a_known_workout_can_be_deleted() {
        let root = journal();
        let (status, _) = delete(root.path(), "/app/transcripts/api/strava/import/notes_1").await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = delete(root.path(), "/app/transcripts/api/strava/import/imp_zz").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = delete(root.path(), "/app/transcripts/api/strava/workout/999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = delete(root.path(), "/app/transcripts/api/strava/import/..").await;
        assert_ne!(status, StatusCode::OK);
        assert!(root.path().join("imports/notes_1").is_dir());
    }

    #[tokio::test]
    async fn a_delete_waits_its_turn_behind_an_import() {
        let root = journal();
        let (_, body) = delete(root.path(), "/app/transcripts/api/strava/workout/101").await;
        let id = body["pending"].as_str().unwrap().to_owned();
        let _import =
            hold_lock(root.path().join("imports/.strava"), LockOptions::default()).unwrap();
        let blocked = root.path().to_path_buf();
        let blocked_id = id.clone();
        std::thread::spawn(move || commit(&blocked, &blocked_id, false))
            .join()
            .unwrap();
        let record = STORE.read::<StravaTarget>(root.path(), &id).unwrap();
        assert_eq!(record.state, DeleteState::NotDeleted);
        assert!(tombstone(root.path(), "20260810", "235500_300").is_none());
        assert!(
            root.path()
                .join("chronicle/20260810/import.strava/235500_300/workout.json")
                .is_file()
        );
    }
}
