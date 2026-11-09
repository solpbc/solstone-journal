// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The trash page: what waits in the trash, and recover, erase now and empty.

use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::response::Response;
use serde_json::{Value, json};
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_retention::recording_trash::Recordings;
use solstone_core_retention::tombstone::TOMBSTONE_NAME;
use solstone_core_trash::{Item, TrashError};

use crate::http::{invalid_config_value, json_response};

/// Everything in the trash, newest first, with the days the setting holds
/// for what is trashed from now on.
pub async fn list(journal_root: PathBuf) -> Response {
    match tokio::task::spawn_blocking(move || {
        let items = solstone_core_trash::list(&journal_root).map_err(|error| error.to_string())?;
        let unfinished =
            solstone_core_trash::unfinished(&journal_root).map_err(|error| error.to_string())?;
        Ok::<_, String>(json!({
            "erase_after_days": solstone_core_trash::erase_after_days(&journal_root),
            "unfinished": unfinished,
            "items": items.iter().map(|item| view(&journal_root, item)).collect::<Vec<_>>(),
        }))
    })
    .await
    {
        Ok(Ok(payload)) => json_response(payload),
        Ok(Err(detail)) => unreadable(detail),
        Err(error) => unreadable(error.to_string()),
    }
}

fn unreadable(detail: String) -> Response {
    crate::storage::read_failed("trash_unreadable", "your trash couldn't be read.", detail)
}

/// One item as the page shows it: what it is, never what it holds.
fn view(journal_root: &Path, item: &Item) -> Value {
    let place = item
        .paths
        .first()
        .and_then(|path| solstone_core_retention::layout::parse_segment_rel(path));
    let (kind, day, stream, segment) = match (&item.unit, &place) {
        (Unit::Memory { day, segment }, place) => (
            "memory",
            day.clone(),
            place.as_ref().map(|(_, stream, _)| stream.clone()),
            segment.clone(),
        ),
        (
            Unit::Segment {
                day,
                stream,
                segment,
            },
            _,
        ) => (
            "segment",
            day.clone(),
            Some(stream.clone()),
            segment.clone(),
        ),
        (Unit::PreviousJournal, _) => (
            "previous_journal",
            String::new(),
            None,
            String::new(),
        ),
        _ => ("other", String::new(), None, String::new()),
    };
    json!({
        "id": item.id,
        "kind": kind,
        "day": day,
        "stream": stream,
        "segment": segment,
        "trashed_at": item.trashed_at,
        "erase_at": item.erase_at,
        "bytes": item.bytes,
        "surface": item.surface,
        "replaces": replaces(journal_root, item),
    })
}

/// Whether something other than the item's own tombstone now sits where the
/// item was, so recovering it would replace that.
fn replaces(journal_root: &Path, item: &Item) -> bool {
    item.paths.iter().any(|path| {
        let place = journal_root.join(path);
        match std::fs::read_dir(&place) {
            Ok(entries) => entries
                .flatten()
                .any(|entry| entry.file_name() != TOMBSTONE_NAME),
            Err(_) => std::fs::symlink_metadata(&place).is_ok_and(|meta| !meta.is_dir()),
        }
    })
}

/// What a trash act was asked to do.
#[derive(serde::Deserialize)]
struct Selection {
    #[serde(default)]
    ids: Vec<String>,
    /// The page a recover came from, when not the trash page: the receipt on
    /// the transcripts or agents page.
    #[serde(default)]
    page: Option<String>,
}

fn parse(body: &Bytes) -> Option<Selection> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Some(Selection {
            ids: Vec::new(),
            page: None,
        });
    }
    serde_json::from_slice(body).ok()
}

fn unreadable_selection() -> Response {
    invalid_config_value("the selection couldn't be read")
}

fn page_surface(page: Option<&str>) -> Surface {
    let page = match page {
        Some("transcripts") => Page::Transcripts,
        Some("agents") => Page::Agents,
        _ => Page::Settings,
    };
    Surface::Web { page }
}

/// Put each selected item back where it was.
pub async fn recover(journal_root: PathBuf, body: Bytes) -> Response {
    let Some(selection) = parse(&body) else {
        return unreadable_selection();
    };
    let surface = page_surface(selection.page.as_deref());
    run(journal_root, selection.ids, move |root, id| {
        let index = RetentionIndex::new(root);
        let recordings = Recordings::new(&index);
        solstone_core_trash::attach(
            root,
            &[&recordings, &solstone_core_trash::previous_journal::PreviousJournal],
            id,
            Action::Recover,
            surface,
        ).map(|_| ())
    })
    .await
}

/// Erase each selected item now.
pub async fn erase(journal_root: PathBuf, body: Bytes) -> Response {
    let Some(selection) = parse(&body) else {
        return unreadable_selection();
    };
    run(journal_root, selection.ids, |root, id| {
        solstone_core_trash::erase(root, id, Action::Erase, page_surface(None))
    })
    .await
}

/// Erase everything in the trash.
pub async fn empty(journal_root: PathBuf) -> Response {
    let ids = match tokio::task::spawn_blocking({
        let root = journal_root.clone();
        move || solstone_core_trash::list(&root)
    })
    .await
    {
        Ok(Ok(items)) => items.into_iter().map(|item| item.id).collect(),
        Ok(Err(error)) => return unreadable(error.to_string()),
        Err(error) => return unreadable(error.to_string()),
    };
    run(journal_root, ids, |root, id| {
        solstone_core_trash::erase(root, id, Action::EmptyTrash, page_surface(None))
    })
    .await
}

/// Run one act per id, each on its own: one that fails never stops the rest.
async fn run(
    journal_root: PathBuf,
    ids: Vec<String>,
    act: impl Fn(&Path, &str) -> Result<(), TrashError> + Send + 'static,
) -> Response {
    let outcome = tokio::task::spawn_blocking(move || {
        let mut done = Vec::new();
        let mut failed = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            match act(&journal_root, &id) {
                Ok(()) => done.push(id),
                Err(error) => {
                    log::warn!("trash item {id}: {error}");
                    failed.push(json!({"id": id, "reason": owner_reason(&error)}));
                }
            }
        }
        json!({"done": done, "failed": failed})
    })
    .await;
    match outcome {
        Ok(payload) => json_response(payload),
        Err(error) => crate::storage::read_failed(
            "trash_act_failed",
            "your journal couldn't finish that. reload your trash to check.",
            error.to_string(),
        ),
    }
}

/// The reason a page can show: the trash's own words, never a path.
fn owner_reason(error: &TrashError) -> String {
    match error {
        TrashError::Io(_) | TrashError::UnknownKind => "your trash couldn't be changed".to_owned(),
        error => error.to_string(),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use solstone_core_retention::Target;
    use std::fs;
    use tower::ServiceExt;

    const SEGMENT: &str = "chronicle/20260731/field/090000_300";

    fn journal() -> tempfile::TempDir {
        let root = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(root.path().join(SEGMENT)).unwrap();
        fs::write(root.path().join(SEGMENT).join("audio.flac"), b"raw").unwrap();
        root
    }

    fn trash_one(root: &Path) -> String {
        let index = RetentionIndex::new(root);
        Recordings::new(&index)
            .trash(
                root,
                &Target {
                    day: "20260731".into(),
                    stream: "field".into(),
                    dir: "090000_300".into(),
                },
                Unit::Segment {
                    day: "20260731".into(),
                    stream: "field".into(),
                    segment: "090000_300".into(),
                },
                Action::SegmentDelete,
                Surface::Web {
                    page: Page::Transcripts,
                },
            )
            .unwrap()
            .id
    }

    async fn call(root: &Path, method: Method, path: &str, body: &str) -> (StatusCode, Value) {
        let response = crate::routes(root.to_path_buf())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
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
    async fn the_trash_lists_what_waits_and_whether_it_would_replace_something() {
        let root = journal();
        let id = trash_one(root.path());
        let (status, body) = call(root.path(), Method::GET, "/app/settings/api/trash", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["erase_after_days"], 7);
        let item = &body["items"][0];
        assert_eq!(item["id"], id);
        assert_eq!(item["kind"], "segment");
        assert_eq!(item["stream"], "field");
        assert_eq!(item["bytes"], 3);
        assert_eq!(
            item["replaces"], false,
            "its own tombstone is not something"
        );
        fs::write(root.path().join(SEGMENT).join("audio.flac"), b"again").unwrap();
        let (_, body) = call(root.path(), Method::GET, "/app/settings/api/trash", "").await;
        assert_eq!(body["items"][0]["replaces"], true);
    }

    #[tokio::test]
    async fn recover_answers_per_id_and_records_the_page_it_came_from() {
        let root = journal();
        let id = trash_one(root.path());
        let (status, body) = call(
            root.path(),
            Method::POST,
            "/app/settings/api/trash/recover",
            &json!({"ids": [id, "20000101T000000000Z-00000000"], "page": "transcripts"})
                .to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["done"], json!([id]));
        assert_eq!(
            body["failed"][0]["reason"],
            "that is no longer in the trash"
        );
        assert_eq!(
            fs::read(root.path().join(SEGMENT).join("audio.flac")).unwrap(),
            b"raw"
        );
        let history = solstone_core_removals::history(root.path(), 10).unwrap();
        let recovered = history
            .entries
            .iter()
            .find(|entry| entry.event == solstone_core_removals::Event::Recovered)
            .unwrap();
        assert_eq!(
            recovered.surface,
            Surface::Web {
                page: Page::Transcripts
            }
        );
    }

    #[tokio::test]
    async fn erase_and_empty_unlink_and_say_what_they_did() {
        let root = journal();
        let first = trash_one(root.path());
        let (_, body) = call(
            root.path(),
            Method::POST,
            "/app/settings/api/trash/erase",
            &json!({"ids": [first]}).to_string(),
        )
        .await;
        assert_eq!(body["done"], json!([first]));
        fs::remove_dir_all(root.path().join(SEGMENT)).unwrap();
        fs::create_dir_all(root.path().join(SEGMENT)).unwrap();
        fs::write(root.path().join(SEGMENT).join("audio.flac"), b"raw").unwrap();
        let second = trash_one(root.path());
        let (_, body) = call(
            root.path(),
            Method::POST,
            "/app/settings/api/trash/empty",
            "",
        )
        .await;
        assert_eq!(body["done"], json!([second]));
        assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_setting_takes_one_to_thirty_days_and_refuses_the_rest() {
        let root = journal();
        for (days, ok) in [(1, true), (30, true), (0, false), (31, false)] {
            let (status, _) = call(
                root.path(),
                Method::PUT,
                "/app/settings/api/config",
                &json!({"section": "trash", "data": {"erase_after_days": days}}).to_string(),
            )
            .await;
            assert_eq!(status.is_success(), ok, "{days}");
        }
        assert_eq!(solstone_core_trash::erase_after_days(root.path()), 30);
    }
}
