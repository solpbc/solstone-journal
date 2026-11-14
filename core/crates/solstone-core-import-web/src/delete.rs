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
use solstone_core_import::import_trash::{
    ImportThings, Imports, deletes_here, failure_key, import_label, import_things, is_import_id,
    kept_originals,
};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_trash::{Chosen, SetResult, TrashError};

use crate::AppState;
use crate::http::{error, import_not_found, json as json_response};

/// The most imports one set may hold.
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
    let mut preview = json!({
        "id": id,
        "source": things.source,
        "imported_at": imported_at(root, id),
        "segments": things.segments.len(),
        "days": days.map(|(first, last)| json!([first, last])),
        "originals": kept_originals(root, things),
        "kept": solstone_core_import::import_trash::kept_uploads(root, things),
        "shared": things.shared.len(),
        "bytes": bytes,
        "deletes_here": deletes_here(&things.source),
        // Its segments deleted earlier, waiting in the trash: an erase now of
        // it erases them too.
        "waiting": solstone_core_import::segment_trash::waiting_by_import(root)
            .get(id)
            .copied()
            .unwrap_or(0),
    });
    // A Strava import is its workouts; an Apple Health or Oura import can leave
    // some of what it wrote on days another import here also wrote to.
    if things.source == solstone_core_import::body_trash::STRAVA {
        preview["workouts"] = json!(things.workouts.len());
    } else if solstone_core_import::body_trash::is_body_source(&things.source) {
        preview["shared_days"] = json!(things.shared_days);
    }
    preview
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
        TrashError::Held(_) | TrashError::InFlight => {
            let mut body = crate::http::error_body(
                "that import couldn't be moved, so nothing was deleted.",
                "import_delete_held",
                error_value.to_string(),
                None,
            );
            body["key"] = json!(failure_key(error_value));
            json_response(StatusCode::CONFLICT, body)
        }
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
        "this import can't be deleted here.",
        "import_delete_elsewhere",
        "not deleted as a whole here".to_owned(),
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
        if !deletes_here(&things.source) {
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
    let mut ids: Vec<String> = Vec::new();
    for id in request.ids {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    // As for one import, each waits for a start of it already under way. In
    // one order, so two sets can't wait on each other.
    let mut gates: Vec<String> = ids.iter().filter(|id| is_import_id(id)).cloned().collect();
    gates.sort();
    let mut guards = Vec::with_capacity(gates.len());
    for id in &gates {
        guards.push(crate::start_lock::acquire(&root, id).await);
    }
    let outcome = tokio::task::spawn_blocking(move || {
        let _guards = guards;
        let mut failed = Vec::new();
        let mut chosen = Vec::new();
        let mut order = Vec::new();
        for id in ids {
            let things = is_import_id(&id)
                .then(|| import_things(&root, &id))
                .flatten();
            match things {
                None => failed.push(json!({"id": id, "key": "other", "reason": "that import isn't in your journal."})),
                Some(things) if !deletes_here(&things.source) => failed.push(
                    json!({"id": id, "key": "other", "reason": "this import can't be deleted here."}),
                ),
                Some(things) if things.unreadable > 0 => failed.push(json!({
                    "id": id,
                    "key": "other",
                    "reason": solstone_core_import::import_trash::PART_UNREADABLE,
                })),
                Some(things) => {
                    chosen.push(Chosen {
                        unit: Unit::Import {
                            source: things.source.clone(),
                            id: id.clone(),
                        },
                        paths: things.paths(),
                        label: import_label(&root, &id),
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
                        failed.push(json!({
                            "id": id,
                            "key": failure_key(&error),
                            "reason": crate_reason(&error),
                        }));
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;
    use tower::ServiceExt;

    const ID: &str = "20260801_090000";
    const SEGMENT: &str = "chronicle/20260801/import.document/090000_0";

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn journal() -> TempDir {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
        write(
            root.path(),
            &format!("imports/{ID}/import.json"),
            json!({"import_id": ID, "original_filename": "report.pdf", "source_hint": "document"})
                .to_string()
                .as_bytes(),
        );
        write(
            root.path(),
            &format!("imports/{ID}/content_manifest.jsonl"),
            json!({"id": "1", "segments": [{"day": "20260801", "key": "090000_0"}]})
                .to_string()
                .as_bytes(),
        );
        write(
            root.path(),
            &format!("imports/{ID}/manifest.json"),
            br#"{"source_type": "document"}"#,
        );
        write(root.path(), &format!("{SEGMENT}/original.pdf"), b"%PDF");
        write(
            root.path(),
            &format!("{SEGMENT}/document_transcript.md"),
            b"text",
        );
        root
    }

    async fn call(root: &Path, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
        let response = crate::routes(root.to_path_buf())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
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

    #[tokio::test]
    async fn the_preview_says_what_the_import_holds() {
        let root = journal();
        let (status, preview) = call(
            root.path(),
            Method::GET,
            &format!("/app/import/api/imports/{ID}/delete"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        assert_eq!(preview["source"], "document");
        assert_eq!(preview["segments"], 1);
        assert_eq!(preview["originals"], 1);
        assert_eq!(preview["days"], json!(["20260801", "20260801"]));
        assert_eq!(preview["deletes_here"], true);
    }

    #[tokio::test]
    async fn a_delete_moves_the_whole_import_to_the_trash_with_its_file_name() {
        let root = journal();
        let (status, body) = call(
            root.path(),
            Method::DELETE,
            &format!("/app/import/api/imports/{ID}"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["trash"]["id"].is_string());
        assert!(!root.path().join("imports").join(ID).exists());
        assert!(!root.path().join(SEGMENT).join("original.pdf").exists());
        let items = solstone_core_trash::list(root.path()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label.as_deref(), Some("report.pdf"));
        // Gone from the page, so a second delete finds nothing.
        let (again, _) = call(
            root.path(),
            Method::DELETE,
            &format!("/app/import/api/imports/{ID}"),
            json!({}),
        )
        .await;
        assert_eq!(again, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_file_whose_import_is_in_the_trash_imports_again() {
        let root = journal();
        let file = root.path().join("report.pdf");
        fs::write(&file, b"%PDF report").unwrap();
        let hash = solstone_core_import::hash_source(&file).unwrap();
        write(
            root.path(),
            &format!("imports/{ID}/manifest.json"),
            json!({"source_type": "document", "source_hash": hash.as_str(), "entry_count": 1})
                .to_string()
                .as_bytes(),
        );
        let save = |item: &str| json!({"client_item_id": item, "path": file});

        // While its import is still in the journal, the same file is refused.
        let (status, live) = call(
            root.path(),
            Method::POST,
            "/app/import/api/save-path",
            save("again"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{live}");
        assert_eq!(live["kind"], "already_imported");

        let (status, body) = call(
            root.path(),
            Method::DELETE,
            &format!("/app/import/api/imports/{ID}"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // In the trash, it is no longer what's in the journal: the file comes back.
        let (status, again) = call(
            root.path(),
            Method::POST,
            "/app/import/api/save-path",
            save("back"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_ne!(again["timestamp"], ID);
    }

    #[tokio::test]
    async fn erase_now_leaves_nothing_in_the_trash() {
        let root = journal();
        let (status, body) = call(
            root.path(),
            Method::DELETE,
            &format!("/app/import/api/imports/{ID}"),
            json!({"erase": true}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["erased"], true);
        assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_set_reports_each_import_and_keys_what_it_refused() {
        let root = journal();
        let (status, body) = call(
            root.path(),
            Method::POST,
            "/app/import/api/imports/delete",
            json!({"ids": [ID, "20260802_090000"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["done"].as_array().unwrap().len(), 1);
        assert_eq!(body["done"][0]["id"], ID);
        assert_eq!(body["failed"][0]["id"], "20260802_090000");
        assert_eq!(body["failed"][0]["key"], "other");
    }

    #[tokio::test]
    async fn a_strava_import_moves_its_own_workouts_and_a_merge_stays_out() {
        let root = journal();
        write(
            root.path(),
            &format!("imports/{ID}/manifest.json"),
            br#"{"source_type": "strava"}"#,
        );
        // Its own tile, and one an earlier import wrote that this one listed.
        let own = "chronicle/20260801/import.strava/070000_300";
        let earlier = "chronicle/20260801/import.strava/080000_300";
        write(
            root.path(),
            &format!("{own}/workout.json"),
            json!({"import_id": ID, "activity_id": 11})
                .to_string()
                .as_bytes(),
        );
        write(
            root.path(),
            &format!("{earlier}/workout.json"),
            json!({"import_id": "20260701_090000", "activity_id": 12})
                .to_string()
                .as_bytes(),
        );
        let (_, preview) = call(
            root.path(),
            Method::GET,
            &format!("/app/import/api/imports/{ID}/delete"),
            json!({}),
        )
        .await;
        assert_eq!(preview["workouts"], 1, "{preview}");
        assert_eq!(preview["segments"], 1, "{preview}");
        let (status, body) = call(
            root.path(),
            Method::DELETE,
            &format!("/app/import/api/imports/{ID}"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!root.path().join("imports").join(ID).exists());
        assert!(root.path().join(own).join("tombstone.json").is_file());
        assert!(root.path().join(earlier).join("workout.json").is_file());

        // A journal archive merge still isn't one thing.
        let merged = "20260803_090000";
        write(
            root.path(),
            &format!("imports/{merged}/manifest.json"),
            br#"{"source_type": "journal_archive"}"#,
        );
        let (status, body) = call(
            root.path(),
            Method::DELETE,
            &format!("/app/import/api/imports/{merged}"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["reason_code"], "import_delete_elsewhere");
    }
}
