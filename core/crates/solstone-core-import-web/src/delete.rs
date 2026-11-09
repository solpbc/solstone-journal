// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deleting a whole import: its record and everything it brought in move to
//! the trash as one thing, at once. A set of imports chosen together moves
//! under one confirmation, each as its own thing.

use std::path::Path;

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::header::USER_AGENT;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};
use solstone_core_import::import_trash::{ImportThings, Imports, import_things, is_import_id};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_trash::{Chosen, SetResult, TrashError};

use crate::AppState;
use crate::http::{error, import_not_found, json as json_response};

/// The most imports one set may hold.
const SET_LIMIT: usize = 200;

/// Sources whose imports delete elsewhere for now: Strava's workouts are
/// shared across its imports and move with the body pages.
fn deletes_elsewhere(source: &str) -> bool {
    source == "strava"
}

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

fn bad_body() -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "that delete couldn't be read, so nothing was deleted.",
        "invalid_request",
        "Invalid delete body".to_owned(),
        None,
    )
}

fn surface_of(headers: &HeaderMap) -> Surface {
    Surface::of_request(
        Page::Import,
        headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    )
}

fn imported_at(root: &Path, id: &str) -> Option<String> {
    let dir = root.join("imports").join(id);
    ["manifest.json", "import.json"].iter().find_map(|name| {
        let value: Value = serde_json::from_slice(&std::fs::read(dir.join(name)).ok()?).ok()?;
        ["imported_at", "created_at", "processed_timestamp"]
            .iter()
            .find_map(|key| value.get(*key).map(Value::to_string))
            .map(|text| text.trim_matches('"').to_owned())
    })
}

fn preview_of(root: &Path, id: &str, things: &ImportThings) -> Value {
    let bytes: u64 = things
        .paths()
        .iter()
        .map(|path| tree_bytes(&root.join(path)))
        .sum();
    let days = things.days();
    json!({
        "id": id,
        "source": things.source,
        "imported_at": imported_at(root, id),
        "segments": things.segments.len(),
        "days": days.map(|(first, last)| json!([first, last])),
        "files": things.files.len(),
        "shared": things.shared.len(),
        "bytes": bytes,
        "deletes_here": !deletes_elsewhere(&things.source),
    })
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|listing| {
            listing
                .flatten()
                .map(|entry| tree_bytes(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// What deleting an import would take: its source, when it came in, and how
/// much of the journal it brought.
pub(crate) async fn preview(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !is_import_id(&id) {
        return import_not_found("Import not found");
    }
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || {
        import_things(&root, &id).map(|things| preview_of(&root, &id, &things))
    })
    .await
    {
        Ok(Some(preview)) => json_response(StatusCode::OK, preview),
        Ok(None) => import_not_found("Import not found"),
        Err(_) => failed("the delete preview stopped"),
    }
}

fn failed(detail: &str) -> Response {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "your journal couldn't finish that delete. reload to check.",
        "import_delete_failed",
        detail.to_owned(),
        None,
    )
}

fn trash_error(error_value: &TrashError) -> Response {
    match error_value {
        TrashError::Missing(_) | TrashError::NotInTrash => import_not_found("Import not found"),
        TrashError::Held(reason) => error(
            StatusCode::CONFLICT,
            &format!("{reason}. nothing was deleted."),
            "import_delete_held",
            reason.clone(),
            None,
        ),
        other => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't move that to the trash, so nothing was deleted.",
            "import_delete_failed",
            other.to_string(),
            None,
        ),
    }
}

fn elsewhere() -> Response {
    error(
        StatusCode::CONFLICT,
        "a Strava import is deleted from its own page for now.",
        "import_delete_elsewhere",
        "strava".to_owned(),
        None,
    )
}

/// Move one import to the trash; `{"erase": true}` erases it at once.
pub(crate) async fn delete(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_import_id(&id) {
        return import_not_found("Import not found");
    }
    let request: DeleteBody = if body.iter().all(u8::is_ascii_whitespace) {
        DeleteBody::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => return bad_body(),
        }
    };
    let surface = surface_of(&headers);
    let root = state.root.clone();
    let guard = crate::start_lock::acquire(&root, &id).await;
    let outcome = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let Some(things) = import_things(&root, &id) else {
            return Err(TrashError::Missing("import".to_owned()));
        };
        if deletes_elsewhere(&things.source) {
            return Ok(None);
        }
        let index = RetentionIndex::new(&root);
        let item = match Imports::new(&index).trash(&root, &id, Action::ImportDelete, surface) {
            Ok(item) => item,
            Err(TrashError::Unfinished(item_id)) => solstone_core_trash::item(&root, &item_id)?
                .ok_or(TrashError::Unfinished(item_id))?,
            Err(error) => return Err(error),
        };
        let erased = request.erase
            && match solstone_core_trash::erase(&root, &item.id, Action::ImportDelete, surface) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("trash item {}: erase now was refused: {error}", item.id);
                    false
                }
            };
        Ok(Some((item, erased)))
    })
    .await;
    match outcome {
        Ok(Ok(Some((item, erased)))) => json_response(
            StatusCode::OK,
            moved(&item.id, &item.erase_at, request.erase, erased),
        ),
        Ok(Ok(None)) => elsewhere(),
        Ok(Err(error_value)) => trash_error(&error_value),
        Err(_) => failed("the delete task stopped"),
    }
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

/// Move a set of imports to the trash under one confirmation, each as its own
/// thing. One refused never stops the rest; a stop part-way through finishes
/// when the journal starts again.
pub(crate) async fn delete_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(request) = serde_json::from_slice::<SetBody>(&body) else {
        return bad_body();
    };
    if request.ids.is_empty() || request.ids.len() > SET_LIMIT {
        return bad_body();
    }
    let surface = surface_of(&headers);
    let root = state.root.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut ids: Vec<String> = Vec::new();
        for id in request.ids {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let mut failed = Vec::new();
        let mut chosen = Vec::new();
        let mut order = Vec::new();
        for id in ids {
            let things = is_import_id(&id)
                .then(|| import_things(&root, &id))
                .flatten();
            match things {
                None => failed.push(json!({"id": id, "reason": "that import isn't in your journal."})),
                Some(things) if deletes_elsewhere(&things.source) => failed.push(
                    json!({"id": id, "reason": "a Strava import is deleted from its own page for now."}),
                ),
                Some(things) => {
                    chosen.push(Chosen {
                        unit: Unit::Import {
                            source: things.source.clone(),
                            id: id.clone(),
                        },
                        paths: things.paths(),
                    });
                    order.push(id);
                }
            }
        }
        let mut done = Vec::new();
        if !chosen.is_empty() {
            let results = solstone_core_import::trash_kinds::with_kinds(&root, |kinds| {
                solstone_core_trash::detach_set(
                    &root,
                    kinds,
                    chosen,
                    Action::ImportDelete,
                    surface,
                    request.erase,
                )
            })?;
            for (id, result) in order.into_iter().zip(results) {
                match result {
                    SetResult::Moved { item, erased } => done.push(json!({
                        "id": id,
                        "result": moved(&item.id, &item.erase_at, request.erase, erased),
                    })),
                    SetResult::Refused(error) => {
                        failed.push(json!({"id": id, "reason": crate_reason(&error)}));
                    }
                }
            }
        }
        Ok::<_, TrashError>(json!({"done": done, "failed": failed}))
    })
    .await;
    match outcome {
        Ok(Ok(payload)) => json_response(StatusCode::OK, payload),
        Ok(Err(error_value)) => trash_error(&error_value),
        Err(_) => failed("the delete task stopped"),
    }
}

/// Why one thing of a set didn't move, in words the owner can read.
fn crate_reason(error: &TrashError) -> String {
    match error {
        TrashError::Missing(_) | TrashError::NotInTrash => {
            "that import isn't in your journal.".to_owned()
        }
        TrashError::Held(reason) => format!("{reason}."),
        _ => "your journal couldn't move it to the trash.".to_owned(),
    }
}
