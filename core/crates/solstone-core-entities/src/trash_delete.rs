// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deleting a person or topic: it moves to the trash with its facet links, at
//! once, and the owner can recover it until it is erased. A set chosen
//! together moves under one confirmation, each as its own thing.
//!
//! A refusal answers `{"success": false, "reason": <code>, "error": <words>}`;
//! the page words it from the code.

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path as RoutePath, State};
use axum::http::header::USER_AGENT;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_facets::entity_trash::{
    Entities, chosen_entity, preview as entity_preview, refusal_code,
};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_removals::{Action, Page, Surface};
use solstone_core_trash::{SetResult, TrashError};

use crate::router::{RouterState, admitted};

/// The most entities one set may hold.
const SET_LIMIT: usize = 200;

#[derive(Debug, Default, serde::Deserialize)]
struct DeleteBody {
    /// Erase it now rather than leave it in the trash.
    #[serde(default)]
    erase: bool,
}

#[derive(Debug, serde::Deserialize)]
struct SetBody {
    ids: Vec<String>,
    #[serde(default)]
    erase: bool,
}

fn surface_of(headers: &HeaderMap) -> Surface {
    Surface::of_request(
        Page::Entities,
        headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    )
}

/// Short words for a refusal code, beside the code the page words itself.
fn words(code: &str) -> &'static str {
    match code {
        "principal" => "you can't delete yourself",
        "record_damaged" => "your journal's record of deleted entities couldn't be updated",
        "busy" => "your journal is busy; try again in a moment",
        "gone" => "that entity isn't in your journal",
        "link_unreadable" => "a facet link couldn't be read",
        "ambiguities_unreadable" => "your journal's ambiguity records couldn't be read",
        "bad_request" => "that delete couldn't be read",
        _ => "your journal couldn't move it to the trash",
    }
}

fn status(code: &str) -> StatusCode {
    match code {
        "gone" => StatusCode::NOT_FOUND,
        "principal" | "bad_request" => StatusCode::BAD_REQUEST,
        "failed" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    }
}

fn refused(code: &str) -> Response {
    (
        status(code),
        Json(json!({"success": false, "reason": code, "error": words(code)})),
    )
        .into_response()
}

fn moved(id: &str, erase_at: &str, asked: bool, erased: bool) -> Value {
    match (asked, erased) {
        (true, true) => json!({"success": true, "erased": true}),
        (true, false) => json!({
            "success": true,
            "erased": false,
            "erase_failed": true,
            "trash": {"id": id, "erase_at": erase_at},
        }),
        (false, _) => json!({"success": true, "trash": {"id": id, "erase_at": erase_at}}),
    }
}

/// Erase item `id` now, as the owner asked. A refusal leaves it in the trash.
fn erase_now(root: &Path, id: &str, surface: Surface) -> bool {
    match solstone_core_trash::erase(root, id, Action::EntityDelete, surface) {
        Ok(()) => true,
        Err(error) => {
            log::warn!("trash item {id}: erase now was refused: {error}");
            false
        }
    }
}

/// Move one entity to the trash; `{"erase": true}` erases it at once.
pub(crate) async fn delete_entity(
    Extension(b): Extension<AccessBasis>,
    State(state): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let request: DeleteBody = if body.iter().all(u8::is_ascii_whitespace) {
        DeleteBody::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => return refused("bad_request"),
        }
    };
    let surface = surface_of(&headers);
    let root = state.journal_root.clone();
    let outcome = solstone_core_serving::seam::run_blocking(move || {
        let index = RetentionIndex::new(&root);
        let item =
            match Entities::new(&index).trash(&root, &entity_id, Action::EntityDelete, surface) {
                Ok(item) => item,
                // It left the journal; the sweep finishes what remains.
                Err(TrashError::Unfinished(item_id)) => solstone_core_trash::item(&root, &item_id)?
                    .ok_or(TrashError::Unfinished(item_id))?,
                Err(error) => return Err(error),
            };
        let erased = request.erase && erase_now(&root, &item.id, surface);
        Ok((item, erased))
    })
    .await;
    match outcome {
        Ok(Ok((item, erased))) => (
            StatusCode::OK,
            Json(moved(&item.id, &item.erase_at, request.erase, erased)),
        )
            .into_response(),
        Ok(Err(error)) => refused(&refusal_code(&error)),
        Err(_) => refused("failed"),
    }
}

/// Move a set of entities to the trash under one confirmation, each as its
/// own thing. One refused never stops the rest; a stop part way through is
/// finished when the journal next sweeps its trash.
pub(crate) async fn delete_set(
    Extension(b): Extension<AccessBasis>,
    State(state): State<Arc<RouterState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let Ok(request) = serde_json::from_slice::<SetBody>(&body) else {
        return refused("bad_request");
    };
    if request.ids.is_empty() || request.ids.len() > SET_LIMIT {
        return refused("bad_request");
    }
    let mut ids: Vec<String> = Vec::new();
    for id in request.ids {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let surface = surface_of(&headers);
    let root = state.journal_root.clone();
    let erase = request.erase;
    let outcome = solstone_core_serving::seam::run_blocking(move || {
        let mut failed = Vec::new();
        let mut chosen = Vec::new();
        let mut order = Vec::new();
        for id in ids {
            match chosen_entity(&root, &id) {
                Ok(Some(thing)) => {
                    chosen.push(thing);
                    order.push(id);
                }
                Ok(None) => failed.push(refusal_row(&id, "gone")),
                Err(_) => failed.push(refusal_row(&id, "link_unreadable")),
            }
        }
        let mut done = Vec::new();
        if !chosen.is_empty() {
            let index = RetentionIndex::new(&root);
            let kind = Entities::new(&index);
            let results = solstone_core_trash::detach_set(
                &root,
                &[&kind],
                chosen,
                Action::EntityDelete,
                surface,
                erase,
            )?;
            for (id, result) in order.into_iter().zip(results) {
                match result {
                    SetResult::Moved { item, erased } => done.push(json!({
                        "id": id,
                        "result": moved(&item.id, &item.erase_at, erase, erased),
                    })),
                    SetResult::Refused(error) => {
                        failed.push(refusal_row(&id, &refusal_code(&error)));
                    }
                }
            }
        }
        Ok::<_, TrashError>(json!({"success": true, "done": done, "failed": failed}))
    })
    .await;
    match outcome {
        Ok(Ok(payload)) => (StatusCode::OK, Json(payload)).into_response(),
        Ok(Err(error)) => refused(&refusal_code(&error)),
        Err(_) => refused("failed"),
    }
}

fn refusal_row(id: &str, code: &str) -> Value {
    json!({"id": id, "reason": code, "error": words(code)})
}

/// What deleting an entity would take: its type, how many facets link it,
/// and whether it has a voice.
pub(crate) async fn preview(
    Extension(b): Extension<AccessBasis>,
    State(state): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let root = state.journal_root.clone();
    match solstone_core_serving::seam::run_blocking(move || entity_preview(&root, &entity_id)).await
    {
        Ok(Ok(Some(preview))) => (StatusCode::OK, Json(json!(preview))).into_response(),
        Ok(Ok(None)) => refused("gone"),
        Ok(Err(_)) => refused("link_unreadable"),
        Err(_) => refused("failed"),
    }
}
