// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deleting one Strava workout: every tile it is, on every day, moves to the
//! trash as one thing, under Strava's lock, and waits there to be recovered or
//! erased like anything else the owner removes. A whole Strava import deletes
//! from the Import page, as every import does.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as RoutePath, State};
use axum::http::header::USER_AGENT;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use solstone_core_import::import_trash::Imports;
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_removals::{Action, Page, Surface};
use solstone_core_trash::TrashError;

use crate::{AppState, legacy_error_response};

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteBody {
    /// Erase it now rather than leave it in the trash.
    #[serde(default)]
    erase: bool,
}

fn valid_workout_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn not_found() -> Response {
    legacy_error_response(
        "strava_workout_not_found",
        "that workout isn't in your journal.",
        "Workout not found",
        StatusCode::NOT_FOUND,
    )
}

pub(crate) async fn delete_workout(
    State(state): State<Arc<AppState>>,
    RoutePath(activity_id): RoutePath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !valid_workout_id(&activity_id) {
        return not_found();
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
    let surface = Surface::of_request(
        Page::Body,
        headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    );
    let root = state.journal_root.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let index = RetentionIndex::new(&root);
        // A workout is body data: nothing this reaches is made from it.
        let made = solstone_core_retention::made_from::NothingMade;
        let item = match Imports::new(&index, &made).trash_workout(
            &root,
            &activity_id,
            Action::WorkoutDelete,
            surface,
        ) {
            Ok(item) => item,
            // It left the journal and is in the trash; the sweep finishes it.
            Err(TrashError::Unfinished(id)) => {
                solstone_core_trash::item(&root, &id)?.ok_or(TrashError::Unfinished(id))?
            }
            Err(error) => return Err(error),
        };
        let erased = request.erase
            && match solstone_core_trash::erase(&root, &item.id, Action::WorkoutDelete, surface) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("trash item {}: erase now was refused: {error}", item.id);
                    false
                }
            };
        Ok((item, erased))
    })
    .await;
    match outcome {
        Ok(Ok((item, erased))) => {
            let mut answer = json!({"success": true});
            if request.erase && erased {
                answer["erased"] = json!(true);
            } else {
                if request.erase {
                    answer["erased"] = json!(false);
                    answer["erase_failed"] = json!(true);
                }
                answer["trash"] = json!({"id": item.id, "erase_at": item.erase_at});
            }
            Json(answer).into_response()
        }
        Ok(Err(TrashError::Missing(_))) => not_found(),
        Ok(Err(TrashError::Held(reason))) => legacy_error_response(
            "strava_delete_held",
            format!("{reason}. nothing was deleted."),
            reason,
            StatusCode::CONFLICT,
        ),
        Ok(Err(error)) => legacy_error_response(
            "strava_delete_failed",
            "your journal couldn't move that to the trash, so nothing was deleted.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        Err(_) => legacy_error_response(
            "strava_delete_failed",
            "your journal couldn't finish that delete. reload to check.",
            "the delete task stopped",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}
