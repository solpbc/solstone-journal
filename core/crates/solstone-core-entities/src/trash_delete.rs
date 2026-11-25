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
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    identity::AccessBasis,
    window_catalog::{response_locale, text},
};
use solstone_core_facets::entity_trash::{
    Entities, chosen_entity, preview as entity_preview, refusal_code,
};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_removals::{Action, Page, Surface};
use solstone_core_trash::{Item, SetResult, TrashError};

use crate::router::{RouterState, admitted};

/// The most entities one set may hold.
const SET_LIMIT: usize = 200;

#[derive(Debug, Default, serde::Deserialize)]
struct DeleteBody {
    /// Erase it now rather than leave it in the trash.
    #[serde(default)]
    erase: bool,
    /// And remove it from the backups too, as the owner chose with erase now.
    #[serde(default)]
    backups: bool,
}

#[derive(Debug, serde::Deserialize)]
struct SetBody {
    ids: Vec<String>,
    #[serde(default)]
    erase: bool,
    #[serde(default)]
    backups: bool,
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
fn words(locale: &str, code: &str) -> String {
    let id = match code {
        "principal" => "journal.window.entities.response.delete.refusal.principal",
        "record_damaged" => "journal.window.entities.response.delete.refusal.record.damaged",
        "busy" => "journal.window.entities.response.delete.refusal.busy",
        "gone" => "journal.window.entities.response.delete.refusal.gone",
        "link_unreadable" => "journal.window.entities.response.delete.refusal.link.unreadable",
        "ambiguities_unreadable" => {
            "journal.window.entities.response.delete.refusal.ambiguities.unreadable"
        }
        "bad_request" => "journal.window.entities.response.delete.refusal.bad.request",
        _ => "journal.window.entities.response.delete.refusal.failed",
    };
    text(locale, id)
}

fn status(code: &str) -> StatusCode {
    match code {
        "gone" => StatusCode::NOT_FOUND,
        "principal" | "bad_request" => StatusCode::BAD_REQUEST,
        "failed" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    }
}

fn refused(locale: &str, code: &str) -> Response {
    (
        status(code),
        Json(json!({"success": false, "reason": code, "error": words(locale, code)})),
    )
        .into_response()
}

/// The answer for a thing moved to the trash. `erase_day` is the owner's day
/// it is erased on, for a command line that says the day rather than the time.
fn moved(root: &Path, item: &Item, asked: bool, erased: bool) -> Value {
    let trash = || {
        json!({
            "id": item.id,
            "erase_at": item.erase_at,
            "erase_day": item
                .erase_at()
                .map(|at| solstone_core_journal_config::owner_day(root, at)),
        })
    };
    match (asked, erased) {
        (true, true) => json!({"success": true, "erased": true}),
        (true, false) => json!({
            "success": true,
            "erased": false,
            "erase_failed": true,
            "trash": trash(),
        }),
        (false, _) => json!({"success": true, "trash": trash()}),
    }
}

/// Erase item `id` now, as the owner asked. A refusal leaves it in the trash.
fn erase_now(root: &Path, id: &str, surface: Surface, backups: bool) -> bool {
    let index = RetentionIndex::new(root);
    let kind = Entities::new(&index)
        .with_on_detached(&solstone_core_speaker_resolve::cleanup_entity_references_on_detach);
    match solstone_core_trash::erase_with(
        root,
        &[&kind],
        id,
        Action::EntityDelete,
        surface,
        None,
        backups,
    ) {
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
    locale: Option<Extension<ResponseLocale>>,
    body: Bytes,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    let request: DeleteBody = if body.iter().all(u8::is_ascii_whitespace) {
        DeleteBody::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => return refused(locale, "bad_request"),
        }
    };
    let surface = surface_of(&headers);
    let root = state.journal_root.clone();
    let outcome = solstone_core_serving::seam::run_blocking(move || {
        let index = RetentionIndex::new(&root);
        let entities = Entities::new(&index)
            .with_on_detached(&solstone_core_speaker_resolve::cleanup_entity_references_on_detach);
        let item = match entities.trash(&root, &entity_id, Action::EntityDelete, surface) {
            Ok(item) => item,
            // It left the journal; the sweep finishes what remains.
            Err(TrashError::Unfinished(item_id)) => solstone_core_trash::item(&root, &item_id)?
                .ok_or(TrashError::Unfinished(item_id))?,
            Err(error) => return Err(error),
        };
        let erased = request.erase && erase_now(&root, &item.id, surface, request.backups);
        Ok(moved(&root, &item, request.erase, erased))
    })
    .await;
    match outcome {
        Ok(Ok(moved)) => (StatusCode::OK, Json(moved)).into_response(),
        Ok(Err(error)) => refused(locale, &refusal_code(&error)),
        Err(_) => refused(locale, "failed"),
    }
}

/// Move a set of entities to the trash under one confirmation, each as its
/// own thing. One refused never stops the rest; a stop part way through is
/// finished when the journal next sweeps its trash.
pub(crate) async fn delete_set(
    Extension(b): Extension<AccessBasis>,
    State(state): State<Arc<RouterState>>,
    headers: HeaderMap,
    locale: Option<Extension<ResponseLocale>>,
    body: Bytes,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    let Ok(request) = serde_json::from_slice::<SetBody>(&body) else {
        return refused(locale, "bad_request");
    };
    if request.ids.is_empty() || request.ids.len() > SET_LIMIT {
        return refused(locale, "bad_request");
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
    let backups = request.backups;
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
                Ok(None) => failed.push(refusal_row(locale, &id, "gone")),
                Err(_) => failed.push(refusal_row(locale, &id, "link_unreadable")),
            }
        }
        let mut done = Vec::new();
        if !chosen.is_empty() {
            let index = RetentionIndex::new(&root);
            let kind = Entities::new(&index).with_on_detached(
                &solstone_core_speaker_resolve::cleanup_entity_references_on_detach,
            );
            let results = solstone_core_trash::detach_set_with(
                &root,
                &[&kind],
                chosen,
                Action::EntityDelete,
                surface,
                erase,
                backups,
            )?;
            for (id, result) in order.into_iter().zip(results) {
                match result {
                    SetResult::Moved { item, erased } => done.push(json!({
                        "id": id,
                        "result": moved(&root, &item, erase, erased),
                    })),
                    SetResult::Refused(error) => {
                        failed.push(refusal_row(locale, &id, &refusal_code(&error)));
                    }
                }
            }
        }
        Ok::<_, TrashError>(json!({"success": true, "done": done, "failed": failed}))
    })
    .await;
    match outcome {
        Ok(Ok(payload)) => (StatusCode::OK, Json(payload)).into_response(),
        Ok(Err(error)) => refused(locale, &refusal_code(&error)),
        Err(_) => refused(locale, "failed"),
    }
}

fn refusal_row(locale: &str, id: &str, code: &str) -> Value {
    json!({"id": id, "reason": code, "error": words(locale, code)})
}

/// What deleting an entity would take: its type, how many facets link it,
/// and whether it has a voice.
pub(crate) async fn preview(
    Extension(b): Extension<AccessBasis>,
    State(state): State<Arc<RouterState>>,
    RoutePath(entity_id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    if let Some(response) = admitted(&b) {
        return response;
    }
    let locale = response_locale(locale);
    let root = state.journal_root.clone();
    match solstone_core_serving::seam::run_blocking(move || entity_preview(&root, &entity_id)).await
    {
        Ok(Ok(Some(preview))) => (StatusCode::OK, Json(json!(preview))).into_response(),
        Ok(Ok(None)) => refused(locale, "gone"),
        Ok(Err(_)) => refused(locale, "link_unreadable"),
        Err(_) => refused(locale, "failed"),
    }
}
