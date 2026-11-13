// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The trash page: what waits in the trash, and recover, erase now and empty.

use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::response::Response;
use serde_json::{Value, json};
use solstone_core_removals::{Action, OperationId, Page, Surface, Unit};
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
        let stopped =
            solstone_core_trash::stopped(&journal_root).map_err(|error| error.to_string())?;
        Ok::<_, String>(json!({
            "erase_after_days": solstone_core_trash::erase_after_days(&journal_root),
            "unfinished": unfinished,
            "items": items.iter().map(|item| view(&journal_root, item)).collect::<Vec<_>>(),
            "stopped": stopped.iter().map(|item| stopped_view(&journal_root, item)).collect::<Vec<_>>(),
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
        (Unit::Import { source, id }, _) => return import_view(journal_root, item, source, id),
        (Unit::Workout { source, day, .. }, _) => {
            return workout_view(journal_root, item, source, day.as_deref());
        }
        (Unit::Entity, _) => return entity_view(journal_root, item),
        (Unit::Facet, _) => return facet_view(journal_root, item),
        (Unit::FacetFile, _) => return facet_file_view(journal_root, item),
        (Unit::Speakers, _) => return speakers_view(item),
        (
            Unit::Originals {
                day,
                stream,
                segment,
                ..
            },
            _,
        ) => return originals_view(journal_root, item, day, stream, segment),
        _ => ("other", String::new(), None, String::new()),
    };
    let mut shown = json!({
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
    });
    if let Some(import) = needed_import(journal_root, item) {
        shown["import"] = import;
    }
    shown
}

/// The import a segment needs to come back, and where it stands: `live` in
/// the journal, `trashed` waiting in the trash (recover it first), or `gone`.
fn needed_import(journal_root: &Path, item: &Item) -> Option<Value> {
    let record = item
        .needs
        .iter()
        .find(|need| need.starts_with("imports/"))?;
    let id = record.strip_prefix("imports/")?;
    let holder = solstone_core_trash::list(journal_root)
        .ok()?
        .into_iter()
        .find(|other| other.id != item.id && other.paths.iter().any(|path| path == record));
    let (state, label) = match holder {
        Some(holder) => ("trashed", holder.label),
        None if journal_root.join(record).is_dir() => (
            "live",
            solstone_core_import::import_trash::import_label(journal_root, id),
        ),
        None => ("gone", None),
    };
    Some(json!({"id": id, "label": label, "state": state}))
}

/// An item stopped part way: what it is, and which way it was going.
fn stopped_view(journal_root: &Path, item: &Item) -> Value {
    let mut shown = view(journal_root, item);
    if let Value::Object(fields) = &mut shown {
        let state = match item.state {
            solstone_core_trash::State::Detaching => "detaching",
            solstone_core_trash::State::Attaching => "attaching",
            solstone_core_trash::State::Erasing => "erasing",
            solstone_core_trash::State::Trashed => "trashed",
        };
        fields.insert("state".to_owned(), json!(state));
        fields.remove("replaces");
    }
    shown
}

/// The fields every row carries.
fn common(item: &Item, kind: &str) -> serde_json::Map<String, Value> {
    let mut fields = serde_json::Map::new();
    fields.insert("id".to_owned(), json!(item.id));
    fields.insert("kind".to_owned(), json!(kind));
    fields.insert("trashed_at".to_owned(), json!(item.trashed_at));
    fields.insert("erase_at".to_owned(), json!(item.erase_at));
    fields.insert("bytes".to_owned(), json!(item.bytes));
    fields.insert("surface".to_owned(), json!(item.surface));
    fields
}

/// Originals released from a segment that stays, as the page shows them: the
/// segment, what was released (`audio`, `screen`), and where the segment
/// stands, since a recover needs it back as itself.
fn originals_view(
    journal_root: &Path,
    item: &Item,
    day: &str,
    stream: &str,
    segment: &str,
) -> Value {
    use solstone_core_retention::original_trash;
    let mut fields = common(item, "originals");
    fields.insert("day".to_owned(), json!(day));
    fields.insert("stream".to_owned(), json!(stream));
    fields.insert("segment".to_owned(), json!(segment));
    fields.insert(
        "modalities".to_owned(),
        json!(original_trash::modalities(item)),
    );
    fields.insert(
        "segment_state".to_owned(),
        json!(original_trash::segment_state(journal_root, item).tag()),
    );
    Value::Object(fields)
}

/// A person or topic as the page shows it: its name, its type, how many
/// facets it was in, whether another of its name is in the journal now,
/// which recovering puts it back beside, and whether it was hidden.
fn entity_view(journal_root: &Path, item: &Item) -> Value {
    let facts = solstone_core_facets::entity_trash::trash_view(journal_root, item);
    let mut fields = common(item, "entity");
    fields.insert("label".to_owned(), json!(item.label));
    fields.insert("entity_type".to_owned(), json!(facts.entity_type));
    fields.insert("facets".to_owned(), json!(facts.facets));
    fields.insert("same_name".to_owned(), json!(facts.same_name));
    fields.insert("hidden".to_owned(), json!(facts.hidden));
    Value::Object(fields)
}

/// A facet as the page shows it: its title, its folder name, and whether a
/// facet of that name is in the journal now, which recovering replaces.
fn facet_view(journal_root: &Path, item: &Item) -> Value {
    let name = item
        .paths
        .first()
        .and_then(|path| path.strip_prefix("facets/"))
        .unwrap_or_default();
    let mut fields = common(item, "facet");
    fields.insert("label".to_owned(), json!(item.label));
    fields.insert("name".to_owned(), json!(name));
    fields.insert(
        "replaces".to_owned(),
        json!(
            !name.is_empty()
                && solstone_core_facets::facet_declaration_file(
                    &journal_root.join("facets").join(name)
                )
                .is_file()
        ),
    );
    Value::Object(fields)
}

/// What a facet merge dropped: the facet it was merged into, from the item's
/// own manifest (a leftover the doctor kept has none), and how many files it
/// holds.
fn facet_file_view(journal_root: &Path, item: &Item) -> Value {
    let folder = item.paths.first().map(String::as_str).unwrap_or_default();
    let held = |rel: &str| {
        solstone_core_trash::held_file(journal_root, &item.id, &format!("{folder}/{rel}")).ok()
    };
    let manifest = held("manifest.json")
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let (state, title) = match manifest
        .as_ref()
        .and_then(|manifest| manifest.get("dest_id"))
        .and_then(Value::as_str)
    {
        None => ("none", None),
        Some(id) => dest_of(journal_root, id),
    };
    let files = held("files").map_or(0, |dir| count_files(&dir));
    let mut fields = common(item, "facet_file");
    fields.insert(
        "dest".to_owned(),
        manifest
            .as_ref()
            .and_then(|manifest| manifest.get("dest_name"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    fields.insert("dest_state".to_owned(), json!(state));
    fields.insert("dest_title".to_owned(), json!(title));
    fields.insert("files".to_owned(), json!(files));
    Value::Object(fields)
}

/// Where the facet a merge's files belong to is now, by its id: in the
/// journal (with its title), waiting in the trash (with the item's label), or
/// gone.
fn dest_of(journal_root: &Path, id: &str) -> (&'static str, Option<String>) {
    let read = |path: &Path| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    };
    let is_it = |facet: &Value| facet.get("id").and_then(Value::as_str) == Some(id);
    if let Ok(entries) = std::fs::read_dir(journal_root.join("facets")) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if let Some(facet) = read(&solstone_core_facets::facet_declaration_file(&entry.path()))
                && is_it(&facet)
            {
                let title = facet
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| Some(entry.file_name().to_string_lossy().into_owned()));
                return ("live", title);
            }
        }
    }
    for waiting in solstone_core_trash::list(journal_root).unwrap_or_default() {
        if waiting.unit != Unit::Facet {
            continue;
        }
        let Some(folder) = waiting.paths.first() else {
            continue;
        };
        let held = solstone_core_trash::held_file(journal_root, &waiting.id, folder);
        if held
            .ok()
            .and_then(|path| read(&solstone_core_facets::facet_declaration_file(&path)))
            .is_some_and(|facet| is_it(&facet))
        {
            return ("trashed", waiting.label.clone());
        }
    }
    ("gone", None)
}

fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => count_files(&entry.path()),
                Ok(_) => 1,
                Err(_) => 0,
            })
            .sum()
    })
}

/// The speakers wipe: how many files it took.
fn speakers_view(item: &Item) -> Value {
    let mut fields = common(item, "speakers");
    fields.insert("files".to_owned(), json!(item.paths.len()));
    Value::Object(fields)
}

/// A workout as the page shows it: its day and the time it began, from where
/// its tiles were, never what they hold.
fn workout_view(journal_root: &Path, item: &Item, source: &str, day: Option<&str>) -> Value {
    let start = solstone_core_import::body_trash::workout_start(&item.paths);
    let clock = start.as_ref().map(|(_, clock)| clock.as_str());
    json!({
        "id": item.id,
        "kind": "workout",
        "source": source,
        "day": day.or(start.as_ref().map(|(day, _)| day.as_str())),
        "start": clock.and_then(|clock| Some(format!("{}:{}", clock.get(..2)?, clock.get(2..4)?))),
        "trashed_at": item.trashed_at,
        "erase_at": item.erase_at,
        "bytes": item.bytes,
        "surface": item.surface,
        "replaces": replaces(journal_root, item),
    })
}

/// An import as the page shows it: its source, its import, and how much of
/// the journal it took with it.
fn import_view(journal_root: &Path, item: &Item, source: &str, id: &str) -> Value {
    let segments: Vec<(String, String, String)> = item
        .paths
        .iter()
        .filter_map(|path| solstone_core_retention::layout::parse_segment_rel(path))
        .collect();
    // A body import's rows split out of a window it shared count toward its days.
    let split_days: Vec<String> = item
        .paths
        .iter()
        .filter_map(|path| solstone_core_import::body_trash::split_of(path))
        .filter_map(|(window, _)| window.split('/').nth(1).map(str::to_owned))
        .collect();
    let days: std::collections::BTreeSet<&str> = segments
        .iter()
        .map(|(day, _, _)| day.as_str())
        .chain(split_days.iter().map(String::as_str))
        .collect();
    json!({
        "id": item.id,
        "kind": "import",
        "source": source,
        "import_id": id,
        "label": item.label,
        "imported_day": id.get(..8),
        "segments": segments.len(),
        "days": [days.first(), days.last()],
        "trashed_at": item.trashed_at,
        "erase_at": item.erase_at,
        "bytes": item.bytes,
        "surface": item.surface,
        "replaces": replaces(journal_root, item),
        // Its segments deleted earlier, still in the trash: erasing it erases them.
        "takes": solstone_core_trash::needing(journal_root, item).map_or(0, |items| items.len()),
    })
}

/// Whether something other than the item's own tombstone now sits where the
/// item was, so recovering it would replace that.
fn replaces(journal_root: &Path, item: &Item) -> bool {
    item.paths.iter().any(|path| {
        // Rows a body import split out of a window it shared: back if the
        // window holds any of them again.
        if solstone_core_import::body_trash::split_of(path).is_some() {
            return solstone_core_import::body_trash::split_returned(journal_root, &item.id, path);
        }
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
    /// the transcripts, agents, import, body or entities page.
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
        Some("import") => Page::Import,
        Some("body") => Page::Body,
        Some("entities") => Page::Entities,
        _ => Page::Settings,
    };
    Surface::Web { page }
}

/// Put each selected item back where it was, all under one operation.
pub async fn recover(journal_root: PathBuf, body: Bytes) -> Response {
    let Some(selection) = parse(&body) else {
        return unreadable_selection();
    };
    let surface = page_surface(selection.page.as_deref());
    let ids = recover_order(&journal_root, selection.ids);
    run(journal_root, ids, move |root, id, operation| {
        solstone_core_import::trash_kinds::with_kinds(root, |kinds| {
            solstone_core_trash::attach_in(root, kinds, id, Action::Recover, surface, operation)
                .map(|_| ())
        })
    })
    .await
}

/// A set recovered together puts homes back before what waits on them: a
/// facet before the people linked in it and a merge's files, and recordings,
/// imports and people before the voice data a wipe took from them. Otherwise
/// in the order given.
fn recover_order(journal_root: &Path, ids: Vec<String>) -> Vec<String> {
    let rank = |id: &String| match solstone_core_trash::item(journal_root, id) {
        Ok(Some(item)) => match item.unit {
            Unit::Facet => 0,
            Unit::Entity => 2,
            Unit::FacetFile => 3,
            Unit::Speakers => 4,
            // After the recording they were released from, which can wait
            // on its import.
            Unit::Originals { .. } => 3,
            // After the import whose upload it needs.
            Unit::Segment { .. } if !item.needs.is_empty() => 2,
            _ => 1,
        },
        _ => 1,
    };
    let mut ranked: Vec<(usize, String)> = ids.into_iter().map(|id| (rank(&id), id)).collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, id)| id).collect()
}

/// Erase each selected item now, all under one operation.
pub async fn erase(journal_root: PathBuf, body: Bytes) -> Response {
    let Some(selection) = parse(&body) else {
        return unreadable_selection();
    };
    let ids = erase_order(&journal_root, selection.ids);
    run(journal_root, ids, |root, id, operation| {
        solstone_core_trash::erase_in(root, id, Action::Erase, page_surface(None), operation)
    })
    .await
}

/// Erased together, what needs another item goes first: erasing an import
/// erases its segments waiting in the trash, so each still answers as itself.
fn erase_order(journal_root: &Path, ids: Vec<String>) -> Vec<String> {
    let needs = |id: &String| {
        solstone_core_trash::item(journal_root, id)
            .ok()
            .flatten()
            .is_some_and(|item| !item.needs.is_empty())
    };
    let (first, rest): (Vec<String>, Vec<String>) = ids.into_iter().partition(needs);
    first.into_iter().chain(rest).collect()
}

/// Erase everything in the trash.
pub async fn empty(journal_root: PathBuf) -> Response {
    let ids = match tokio::task::spawn_blocking({
        let root = journal_root.clone();
        move || solstone_core_trash::list(&root)
    })
    .await
    {
        Ok(Ok(items)) => erase_order(
            &journal_root,
            items.into_iter().map(|item| item.id).collect(),
        ),
        Ok(Err(error)) => return unreadable(error.to_string()),
        Err(error) => return unreadable(error.to_string()),
    };
    run(journal_root, ids, |root, id, operation| {
        solstone_core_trash::erase_in(root, id, Action::EmptyTrash, page_surface(None), operation)
    })
    .await
}

/// Run one act per id, each on its own: one that fails never stops the rest.
/// The ids are one owner act, so every item records under one operation.
async fn run(
    journal_root: PathBuf,
    ids: Vec<String>,
    act: impl Fn(&Path, &str, &OperationId) -> Result<(), TrashError> + Send + 'static,
) -> Response {
    let outcome = tokio::task::spawn_blocking(move || {
        let operation = OperationId::fresh(chrono::Utc::now());
        let mut done = Vec::new();
        let mut failed = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            match act(&journal_root, &id, &operation) {
                Ok(()) => done.push(id),
                Err(error) => {
                    log::warn!("trash item {id}: {error}");
                    let mut row = json!({"id": id, "reason": owner_reason(&error)});
                    if let Some(detail) = refusal(&error) {
                        row["reason"] = detail.get("code").cloned().unwrap_or(Value::Null);
                        row["detail"] = detail;
                    }
                    failed.push(row);
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

/// A kind's refusal, when it gave one as a code the page maps to its own
/// words: a JSON object with `code` and what the words need.
fn refusal(error: &TrashError) -> Option<Value> {
    let TrashError::Held(text) = error else {
        return None;
    };
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(|value| value.get("code").is_some_and(Value::is_string))
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
    use solstone_core_indexer_store::RetentionIndex;
    use solstone_core_retention::Target;
    use solstone_core_retention::recording_trash::Recordings;
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
        trash_segment(root, "090000_300")
    }

    fn trash_segment(root: &Path, key: &str) -> String {
        let index = RetentionIndex::new(root);
        Recordings::new(&index)
            .trash(
                root,
                &Target {
                    day: "20260731".into(),
                    stream: "field".into(),
                    dir: key.into(),
                },
                Unit::Segment {
                    day: "20260731".into(),
                    stream: "field".into(),
                    segment: key.into(),
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

    /// An audio import whose two segments were deleted one at a time: the
    /// first waits as a segment needing the import, the second took it whole.
    fn import_deleted_by_segments(root: &Path) -> (String, String) {
        let id = "20260731_090000";
        let segments = ["090000_300", "090500_300"];
        let listed: Vec<Value> = segments
            .iter()
            .map(|key| json!({"day": "20260731", "stream": "import.audio", "segment": key}))
            .collect();
        let write = |rel: &str, bytes: &[u8]| {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        };
        write(
            &format!("imports/{id}/imported.json"),
            json!({"source_type": "audio", "segments": listed})
                .to_string()
                .as_bytes(),
        );
        write(
            &format!("imports/{id}/import.json"),
            json!({"original_filename": "talk.m4a", "file_path": format!("imports/{id}/talk.m4a")})
                .to_string()
                .as_bytes(),
        );
        write(&format!("imports/{id}/talk.m4a"), b"upload");
        let mut ids = Vec::new();
        for key in segments {
            write(
                &format!("chronicle/20260731/import.audio/{key}/audio.flac"),
                b"raw",
            );
        }
        let index = RetentionIndex::new(root);
        for key in segments {
            let (item, _) = solstone_core_import::segment_trash::trash_segment(
                root,
                &index,
                Unit::Segment {
                    day: "20260731".into(),
                    stream: "import.audio".into(),
                    segment: key.into(),
                },
                format!("chronicle/20260731/import.audio/{key}"),
                Action::SegmentDelete,
                Surface::Web {
                    page: Page::Transcripts,
                },
            )
            .unwrap();
            ids.push(item.id);
        }
        (ids[0].clone(), ids[1].clone())
    }

    #[tokio::test]
    async fn an_import_row_says_what_its_erase_takes_and_its_segment_waits_for_it() {
        let root = journal();
        let (segment, import) = import_deleted_by_segments(root.path());
        let (_, listed) = call(root.path(), Method::GET, "/app/settings/api/trash", "").await;
        let row = |id: &str| {
            listed["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .cloned()
                .unwrap()
        };
        assert_eq!(row(&import)["kind"], "import");
        assert_eq!(row(&import)["takes"], 1);
        assert_eq!(row(&segment)["import"]["state"], "trashed");
        assert_eq!(row(&segment)["import"]["label"], "talk.m4a");
        let body = json!({"ids": [segment.clone()]}).to_string();
        let (_, refused) = call(
            root.path(),
            Method::POST,
            "/app/settings/api/trash/recover",
            &body,
        )
        .await;
        assert_eq!(refused["failed"][0]["reason"], "import_in_trash");
        // Together, the import comes back before the segment that needs it.
        let body = json!({"ids": [segment.clone(), import.clone()]}).to_string();
        let (_, both) = call(
            root.path(),
            Method::POST,
            "/app/settings/api/trash/recover",
            &body,
        )
        .await;
        assert_eq!(both["failed"], json!([]), "{both}");
        assert!(
            root.path()
                .join("imports/20260731_090000/talk.m4a")
                .is_file()
        );
    }

    #[tokio::test]
    async fn emptying_the_trash_answers_for_a_segment_its_import_took() {
        let root = journal();
        let (segment, import) = import_deleted_by_segments(root.path());
        let (_, emptied) = call(
            root.path(),
            Method::POST,
            "/app/settings/api/trash/empty",
            "",
        )
        .await;
        assert_eq!(emptied["failed"], json!([]), "{emptied}");
        let done = emptied["done"].as_array().unwrap();
        assert!(done.contains(&json!(segment)) && done.contains(&json!(import)));
        assert!(!root.path().join("imports/20260731_090000").exists());
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
    async fn a_selection_recovered_together_is_one_operation_in_the_history() {
        let root = journal();
        let other = "chronicle/20260731/field/091000_300";
        fs::create_dir_all(root.path().join(other)).unwrap();
        fs::write(root.path().join(other).join("audio.flac"), b"raw").unwrap();
        let ids = [
            trash_one(root.path()),
            trash_segment(root.path(), "091000_300"),
        ];
        let (_, body) = call(
            root.path(),
            Method::POST,
            "/app/settings/api/trash/recover",
            &json!({"ids": ids}).to_string(),
        )
        .await;
        assert_eq!(body["done"].as_array().unwrap().len(), 2);
        let history = solstone_core_removals::history(root.path(), 10).unwrap();
        let recovered: std::collections::BTreeSet<&str> = history
            .entries
            .iter()
            .filter(|entry| entry.event == solstone_core_removals::Event::Recovered)
            .map(|entry| entry.operation.as_str())
            .collect();
        assert_eq!(recovered.len(), 1);
        // Each was trashed on its own, so the two moves are two operations.
        let trashed: std::collections::BTreeSet<&str> = history
            .entries
            .iter()
            .filter(|entry| entry.event == solstone_core_removals::Event::Trashed)
            .map(|entry| entry.operation.as_str())
            .collect();
        assert_eq!(trashed.len(), 2);
    }

    #[test]
    fn a_recover_from_the_entities_page_is_recorded_there() {
        assert_eq!(
            page_surface(Some("entities")),
            Surface::Web {
                page: Page::Entities
            }
        );
        assert_eq!(
            page_surface(Some("nowhere")),
            Surface::Web {
                page: Page::Settings
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
