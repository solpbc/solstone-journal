// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-directed recording delete: the recording moves to the trash at once.
//!
//! The trash is the hold: a recording leaves the journal the moment the owner
//! confirms, and waits in the trash until its erase date, or is erased now when
//! the owner asks. Agent memories are recordings on their own stream, and the
//! agents page deletes them through this route.

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as RoutePath, State};
use axum::http::header::USER_AGENT;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_retention::Target;
use solstone_core_retention::recording_trash::Recordings;
use solstone_core_retention::tombstone::TOMBSTONE_NAME;
use solstone_core_trash::TrashError;

use crate::{AppState, legacy_error_response};

/// Agent memories are segments on their own stream.
const AGENT_MEMORY_STREAM_PREFIX: &str = "agent-memory-";

/// What the owner asked for beyond the move to the trash.
#[derive(Debug, Default, serde::Deserialize)]
struct DeleteBody {
    /// Erase it now rather than leave it in the trash.
    #[serde(default)]
    erase: bool,
}

pub(crate) async fn delete_segment(
    State(state): State<Arc<AppState>>,
    RoutePath((day, stream, key)): RoutePath<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
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
    let request: DeleteBody = if body.iter().all(u8::is_ascii_whitespace) {
        DeleteBody::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => {
                return legacy_error_response(
                    "invalid_request",
                    "that delete couldn't be read, so nothing was deleted.",
                    "Invalid delete body",
                    StatusCode::BAD_REQUEST,
                );
            }
        }
    };
    let segment_dir =
        crate::segment_media::physical_segment_dir(&state.journal_root, &day, &stream, &key);
    if !segment_dir.is_dir() || is_tombstone_only(&segment_dir) {
        return invalid_segment("Segment not found", StatusCode::NOT_FOUND);
    }
    let page = page_of(&stream);
    let surface = Surface::of_request(
        page,
        headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    );
    let root = state.journal_root.clone();
    let erase = request.erase;
    let outcome = tokio::task::spawn_blocking(move || {
        trash_recording(&root, day, stream, key, page, surface, erase)
    })
    .await;
    match outcome {
        Ok(Ok(Trashed {
            key,
            id,
            erase_at,
            erased,
        })) => match (erase, erased) {
            (true, true) => {
                Json(json!({"success": true, "deleted": key, "erased": true})).into_response()
            }
            // In the trash, but the erase the owner asked for was refused:
            // say where it is.
            (true, false) => Json(json!({
                "success": true,
                "deleted": key,
                "erased": false,
                "erase_failed": true,
                "trash": {"id": id, "erase_at": erase_at},
            }))
            .into_response(),
            (false, _) => Json(json!({
                "success": true,
                "deleted": key,
                "trash": {"id": id, "erase_at": erase_at},
            }))
            .into_response(),
        },
        Ok(Err(error)) => trash_error(&error),
        Err(_) => legacy_error_response(
            "segment_delete_failed",
            "your journal couldn't finish that delete. reload to check.",
            "the delete task stopped",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

struct Trashed {
    key: String,
    id: String,
    erase_at: String,
    erased: bool,
}

fn trash_recording(
    root: &Path,
    day: String,
    stream: String,
    key: String,
    page: Page,
    surface: Surface,
    erase: bool,
) -> Result<Trashed, TrashError> {
    let (action, unit) = if page == Page::Agents {
        (
            Action::MemoryDelete,
            Unit::Memory {
                day: day.clone(),
                segment: key.clone(),
            },
        )
    } else {
        (
            Action::SegmentDelete,
            Unit::Segment {
                day: day.clone(),
                stream: stream.clone(),
                segment: key.clone(),
            },
        )
    };
    let index = RetentionIndex::new(root);
    let recordings = Recordings::new(&index);
    let target = Target {
        day,
        stream,
        dir: key.clone(),
    };
    let item = match recordings.trash(root, &target, unit, action, surface) {
        Ok(item) => item,
        // It left the journal and is in the trash; a step after the move
        // failed, and the trash's sweep finishes it. Not erased now.
        Err(TrashError::Unfinished(id)) => {
            let erase_at = solstone_core_trash::item(root, &id)?
                .map(|item| item.erase_at)
                .unwrap_or_default();
            return Ok(Trashed {
                key,
                id,
                erase_at,
                erased: false,
            });
        }
        Err(error) => return Err(error),
    };
    let erased = erase
        && match solstone_core_trash::erase(root, &item.id, action, surface) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("trash item {}: erase now was refused: {error}", item.id);
                false
            }
        };
    Ok(Trashed {
        key,
        id: item.id,
        erase_at: item.erase_at,
        erased,
    })
}

fn trash_error(error: &TrashError) -> Response {
    match error {
        TrashError::Missing(_) | TrashError::NotInTrash => {
            invalid_segment("Segment not found", StatusCode::NOT_FOUND)
        }
        TrashError::Held(reason) => legacy_error_response(
            "segment_delete_held",
            format!("{reason}. nothing was deleted."),
            reason.clone(),
            StatusCode::CONFLICT,
        ),
        error => legacy_error_response(
            "segment_delete_failed",
            "your journal couldn't move that to the trash, so nothing was deleted.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

fn page_of(stream: &str) -> Page {
    if stream.starts_with(AGENT_MEMORY_STREAM_PREFIX) {
        Page::Agents
    } else {
        Page::Transcripts
    }
}

/// A segment that holds only its tombstone was deleted before.
fn is_tombstone_only(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    let mut count = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        if entry.file_name() == TOMBSTONE_NAME {
            count += 1;
        } else {
            return false;
        }
    }
    count == 1
}

/// Finish trash steps a stop interrupted and erase what has waited its days.
/// Runs when the journal's web app starts; the maintenance schedule runs it
/// hourly.
pub(crate) fn sweep_trash(journal_root: &Path) {
    // Off the request path: after a long time away, erasing what fell due can
    // take a while, and the pages need not wait for it.
    let journal_root = journal_root.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("trash-sweep".to_owned())
        .spawn(move || {
            let index = RetentionIndex::new(&journal_root);
            let recordings = Recordings::new(&index);
            let swept =
                solstone_core_trash::sweep(&journal_root, &[&recordings, &solstone_core_trash::previous_journal::PreviousJournal], chrono::Utc::now());
            for failure in &swept.failed {
                log::warn!("trash: {failure}");
            }
        });
    if let Err(error) = spawned {
        log::warn!("trash: the start-up sweep did not start: {error}");
    }
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
