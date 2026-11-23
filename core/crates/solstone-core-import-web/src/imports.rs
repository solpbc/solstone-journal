// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{BufRead, BufReader},
    path::Path,
    sync::LazyLock,
};

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Map, Value, json};

use axum::Extension;
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    window_catalog::{response_locale, text},
};

use crate::{
    AppState,
    http::{import_not_found, json as json_response},
};

#[derive(Clone, Copy)]
struct SourceMetadata {
    name: &'static str,
    display_name: &'static str,
    icon: &'static str,
    description: &'static str,
    input_type: &'static str,
    upload_prompt: &'static str,
    has_guide: bool,
    accept: &'static str,
}

const SOURCES: &[SourceMetadata] = &[
    SourceMetadata {
        name: "ics",
        display_name: "journal.window.import.source.ics.name",
        icon: "calendar",
        description: "journal.window.import.source.ics.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.ics.upload.prompt",
        has_guide: true,
        accept: ".ics,.zip",
    },
    SourceMetadata {
        name: "chatgpt",
        display_name: "journal.window.import.source.chatgpt.name",
        icon: "message-square",
        description: "journal.window.import.source.chatgpt.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.chatgpt.upload.prompt",
        has_guide: true,
        accept: ".zip",
    },
    SourceMetadata {
        name: "claude",
        display_name: "journal.window.import.source.claude.name",
        icon: "message-circle",
        description: "journal.window.import.source.claude.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.claude.upload.prompt",
        has_guide: true,
        accept: ".zip",
    },
    SourceMetadata {
        name: "gemini",
        display_name: "journal.window.import.source.gemini.name",
        icon: "sparkles",
        description: "journal.window.import.source.gemini.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.gemini.upload.prompt",
        has_guide: true,
        accept: ".zip,.json",
    },
    SourceMetadata {
        name: "journal_archive",
        display_name: "journal.window.import.source.journal.archive.name",
        icon: "book",
        description: "journal.window.import.source.journal.archive.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.journal.archive.upload.prompt",
        has_guide: true,
        accept: ".zip",
    },
    SourceMetadata {
        name: "audio_archive",
        display_name: "journal.window.import.source.audio.archive.name",
        icon: "archive",
        description: "journal.window.import.source.audio.archive.description",
        input_type: "path_input",
        upload_prompt: "journal.window.import.source.audio.archive.upload.prompt",
        has_guide: false,
        accept: "",
    },
    SourceMetadata {
        name: "recording",
        display_name: "journal.window.import.source.recording.name",
        icon: "mic",
        description: "journal.window.import.source.recording.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.recording.upload.prompt",
        has_guide: false,
        accept: ".flac,.gif,.jpeg,.jpg,.m4a,.mov,.mp3,.mp4,.ogg,.opus,.png,.wav,.webm,.webp",
    },
    SourceMetadata {
        name: "document",
        display_name: "journal.window.import.source.document.name",
        icon: "file",
        description: "journal.window.import.source.document.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.document.upload.prompt",
        has_guide: false,
        accept: ".pdf",
    },
    SourceMetadata {
        name: "image",
        display_name: "journal.window.import.source.image.name",
        icon: "image",
        description: "journal.window.import.source.image.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.image.upload.prompt",
        has_guide: false,
        accept: ".png,.jpg,.jpeg,.webp,.gif",
    },
    SourceMetadata {
        name: "strava",
        display_name: "journal.window.import.source.strava.name",
        icon: "activity",
        description: "journal.window.import.source.strava.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.strava.upload.prompt",
        has_guide: true,
        accept: ".zip,.csv",
    },
    SourceMetadata {
        name: "apple_health",
        display_name: "journal.window.import.source.apple.health.name",
        icon: "heart-pulse",
        description: "journal.window.import.source.apple.health.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.apple.health.upload.prompt",
        has_guide: true,
        accept: ".zip",
    },
    SourceMetadata {
        name: "evernote",
        display_name: "journal.window.import.source.evernote.name",
        icon: "file",
        description: "journal.window.import.source.evernote.description",
        input_type: "file",
        upload_prompt: "journal.window.import.source.evernote.upload.prompt",
        has_guide: false,
        accept: ".enex",
    },
    SourceMetadata {
        name: "quick",
        display_name: "journal.window.import.source.quick.name",
        icon: "zap",
        description: "journal.window.import.source.quick.description",
        input_type: "text",
        upload_prompt: "journal.window.import.source.quick.upload.prompt",
        has_guide: false,
        accept: "",
    },
];

static ICONS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../solstone-core-convey-shell/assets/static/icons/lucide.json"
    ))
    .expect("embedded Lucide icons")
});

#[derive(Clone, Debug)]
pub(crate) struct ImportInfo {
    /// The sole time value read by list ordering and status timeout arithmetic.
    pub(crate) imported_at: f64,
    pub(crate) values: Map<String, Value>,
}

#[cfg(unix)]
fn ctime(path: &Path) -> std::io::Result<f64> {
    use std::os::unix::fs::MetadataExt;

    let metadata = path.metadata()?;
    Ok(metadata.ctime() as f64 + metadata.ctime_nsec() as f64 / 1_000_000_000.0)
}

#[cfg(windows)]
fn ctime(path: &Path) -> std::io::Result<f64> {
    let metadata = path.metadata()?;
    let timestamp = metadata.created().or_else(|_| metadata.modified())?;
    timestamp
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .map_err(std::io::Error::other)
}

fn object_from_file(path: &Path) -> Result<Map<String, Value>, ()> {
    let text = fs::read_to_string(path).map_err(|_| ())?;
    serde_json::from_str::<Value>(&text)
        .map_err(|_| ())?
        .as_object()
        .cloned()
        .ok_or(())
}

fn value_from_file(path: &Path) -> Result<Value, ()> {
    let text = fs::read_to_string(path).map_err(|_| ())?;
    serde_json::from_str(&text).map_err(|_| ())
}

fn duration_minutes(files: &Value) -> Option<i64> {
    let mut times: Vec<&str> = files
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|path| Path::new(path).file_name().and_then(|name| name.to_str()))
        .filter(|name| {
            name.get(..6)
                .is_some_and(|time| time.bytes().all(|byte| byte.is_ascii_digit()))
        })
        .map(|name| &name[..6])
        .collect();
    times.sort_unstable();
    let (first, last) = (times.first()?, times.last()?);
    let minutes = |time: &str| -> i64 {
        time[..2].parse::<i64>().expect("digit-only hour") * 60
            + time[2..4].parse::<i64>().expect("digit-only minute")
    };
    let duration = minutes(last) - minutes(first);
    (duration > 0).then_some(duration)
}

pub(crate) fn load_import_info(root: &Path, timestamp: &str) -> Result<ImportInfo, std::io::Error> {
    let directory = root.join("imports").join(timestamp);
    let created_at = ctime(&directory)?;
    let mut values = Map::new();
    values.insert("timestamp".into(), json!(timestamp));
    values.insert("created_at".into(), json!(created_at));
    values.insert("imported_at".into(), json!(created_at));
    let mut imported_at = created_at;
    // Reference parity: a malformed optional import.json is swallowed and contributes no ten fields.
    if let Ok(metadata) = object_from_file(&directory.join("import.json")) {
        if crate::audio_archive::selected(&metadata) {
            match crate::audio_archive::run_id(root, &metadata) {
                Ok(Some(id)) => {
                    values.insert("archive_run_id".into(), json!(id));
                }
                Ok(None) => (),
                Err(_) => {
                    values.insert("archive_lookup_unknown".into(), json!(true));
                }
            }
        }
        for key in [
            "original_filename",
            "file_size",
            "mime_type",
            "facet",
            "setting",
            "user_timestamp",
            "imported_via",
            "link_id",
            "observer_handle",
        ] {
            values.insert(
                key.into(),
                metadata.get(key).cloned().unwrap_or(Value::Null),
            );
        }
        if let Some(hint) = metadata.get("source_hint").filter(|hint| hint.is_string()) {
            values.insert("source_hint".into(), hint.clone());
        }
        values.insert(
            "task_id".into(),
            metadata.get("task_id").cloned().unwrap_or(Value::Null),
        );
        if let Some(upload) = metadata.get("upload_timestamp").and_then(Value::as_f64) {
            imported_at = upload / 1000.0;
            values.insert("imported_at".into(), json!(imported_at));
        }
    }
    values.insert("processed".into(), Value::Bool(false));
    values.insert("error".into(), Value::Null);
    values.insert("error_stage".into(), Value::Null);
    let imported = object_from_file(&directory.join("imported.json")).ok();
    if let Some(result) = &imported {
        values.insert("processed".into(), Value::Bool(true));
        for key in [
            "total_files_created",
            "target_day",
            "source_type",
            "source_display",
            "entries_written",
            "entities_seeded",
            "date_range",
        ] {
            let default = if key == "total_files_created" {
                json!(0)
            } else {
                Value::Null
            };
            values.insert(key.into(), result.get(key).cloned().unwrap_or(default));
        }
        if result.contains_key("error") {
            values.insert(
                "error".into(),
                result.get("error").cloned().unwrap_or(Value::Null),
            );
            values.insert(
                "error_stage".into(),
                result.get("error_stage").cloned().unwrap_or(Value::Null),
            );
        }
        if let Some(duration) = result.get("all_created_files").and_then(duration_minutes) {
            values.insert("duration_minutes".into(), json!(duration));
        }
    }
    let proj = solstone_core_import::project_import_result(root, timestamp);
    values.insert("status".into(), json!(proj.status.as_str()));
    values.insert(
        "error".into(),
        proj.error.as_ref().map_or(Value::Null, |err| json!(err)),
    );
    values.insert(
        "error_stage".into(),
        proj.error_stage
            .as_ref()
            .map_or(Value::Null, |stage| json!(stage)),
    );
    if let Ok(finish) = std::fs::read_to_string(directory.join("finish.txt")) {
        let finish = finish.trim();
        if !finish.is_empty() {
            values.insert("finish".into(), json!(finish));
        }
    }
    if values.get("source_type").is_none_or(Value::is_null) && source(&proj.source_type).is_some() {
        values.insert("source_type".into(), json!(proj.source_type));
        values.insert("source_display".into(), json!(proj.source_display));
    }
    // Imports the native producer wrote carry the projection's facts (counts, generation,
    // gaps); every other importer's row keeps exactly the shape it always had.
    if proj.is_native() {
        values.extend(proj.native_row_overlay());
        // A source_type with no upload tile (e.g. `audio`) has no SOURCES entry to name
        // it, so the row would otherwise carry no source_display at all. Fall back to the
        // projection's own computed display; a source_type with a tile keeps the tile's
        // own wording untouched.
        if values.get("source_display").is_none_or(Value::is_null)
            && source(&proj.source_type).is_none()
        {
            values.insert("source_display".into(), json!(proj.source_display));
        }
    }
    Ok(ImportInfo {
        imported_at,
        values,
    })
}

fn source(name: &str) -> Option<&'static SourceMetadata> {
    SOURCES.iter().find(|item| item.name == name)
}
pub(crate) fn source_icon(name: &str) -> Option<String> {
    source(name).and_then(|item| ICONS.get(item.icon).cloned())
}
/// Sources with no upload tile that the Import page still names in its own words.
const UNTILED_SOURCE_NAMES: &[(&str, &str)] = &[(
    "text",
    "journal.window.import.import.source.names.text.text",
)];

pub(crate) fn source_display_localized(locale: &str, name: &str) -> Option<String> {
    source(name)
        .map(|item| text(locale, item.display_name))
        .or_else(|| {
            UNTILED_SOURCE_NAMES
                .iter()
                .find(|(untiled, _)| *untiled == name)
                .map(|(_, key)| text(locale, key))
        })
}

/// The zone names an audio archive can be placed in, in byte order: every name
/// in the bundled time zone database, each of which the journal reads, except
/// `Factory`, which names no place.
fn time_zone_names() -> &'static [String] {
    static NAMES: LazyLock<Vec<String>> = LazyLock::new(|| {
        let mut names: Vec<String> = jiff::tz::TimeZoneDatabase::bundled()
            .available()
            .map(|name| name.as_str().to_owned())
            .filter(|name| name != "Factory")
            .collect();
        names.sort();
        names
    });
    &NAMES
}

/// Sources the journal on a phone doesn't take: an Apple Health export is
/// brought in by a journal on a computer.
const DESKTOP_ONLY: &[&str] = &["apple_health"];

pub(crate) async fn sources(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<AppState>,
) -> Response {
    let locale = response_locale(selected_locale);
    let on_phone = state.route.is_some();
    let offered = SOURCES
        .iter()
        .filter(|item| !(on_phone && DESKTOP_ONLY.contains(&item.name)));
    let items: Vec<Value> = offered.map(|item| {
        let mut value = json!({
        "name": item.name, "display_name": text(locale, item.display_name), "icon": item.icon,
        "description": text(locale, item.description), "input_type": item.input_type, "upload_prompt": text(locale, item.upload_prompt),
        "has_guide": item.has_guide, "accept": item.accept, "icon_svg": ICONS.get(item.icon),
        });
        if item.name == "apple_health" {
            value["too_big_words"] = json!(solstone_core_import_sources::body_status::too_big_to_upload_words_localized(locale));
        }
        if item.name == "audio_archive" {
            value["time_zones"] = json!(time_zone_names());
        }
        value
    }).collect();
    let total = items.len();
    json_response(StatusCode::OK, json!({"items": items, "total": total}))
}

pub(crate) async fn list(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let locale = response_locale(selected_locale);
    let filter = query
        .get("source")
        .filter(|value| !value.is_empty())
        .map(String::as_str);
    let client_item = query
        .get("client_item_id")
        .filter(|value| !value.is_empty());
    let imports_dir = state.root.join("imports");
    crate::lifecycle::maybe_sweep_abandoned_uploads(&state.root);
    let Ok(entries) = fs::read_dir(&imports_dir) else {
        return json_response(
            StatusCode::OK,
            json!({"imports": [], "source_options": [], "total": 0, "page": 1, "per_page": 25, "pages": 0, "total_entries_written": 0, "total_entities_seeded": 0}),
        );
    };
    let mut rows = Vec::new();
    let waiting = solstone_core_import::segment_trash::waiting_by_import(&state.root);
    let mut source_options = BTreeSet::new();
    let apple = crate::body_state::apple_imports(&state.root);
    let sync_made = solstone_core_import::sync_state::SyncMadeImports::read(&state.root);
    for entry in entries.flatten() {
        let timestamp = entry.file_name().to_string_lossy().into_owned();
        if timestamp.len() != 15 || timestamp.matches('_').count() != 1 || !entry.path().is_dir() {
            continue;
        }
        let Ok(mut info) = load_import_info(&state.root, &timestamp) else {
            continue;
        };
        // Keep the selected-source row for reference recovery, and show its
        // indexed run once in ordinary history after native admission.
        if client_item.is_none()
            && info
                .values
                .get("archive_run_id")
                .and_then(Value::as_str)
                .is_some()
        {
            continue;
        }
        // A sync's runs are not imports: Plaud's recordings and the audio
        // folder's files stay out of history, where they are set up instead.
        if client_item.is_none() && sync_made.contains(&state.root, &timestamp) {
            continue;
        }
        // An Apple Health row is named and read as one before anything below
        // looks at its kind.
        crate::body_state::overlay(locale, &state.root, &timestamp, &mut info.values, &apple);
        let source_type = info
            .values
            .get("source_type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(display) = source_type
            .as_deref()
            .and_then(|name| source_display_localized(locale, name))
        {
            info.values.insert("source_display".into(), json!(display));
        }
        if let Some(name) = source_type.as_ref().filter(|name| !name.is_empty()) {
            source_options.insert(name.clone());
        }
        if filter.is_none_or(|wanted| source_type.as_deref() == Some(wanted))
            && client_item.is_none_or(|wanted| {
                let Ok(metadata) = object_from_file(&entry.path().join("import.json")) else {
                    return false;
                };
                if metadata.get("client_item_id").and_then(Value::as_str) != Some(wanted) {
                    return false;
                }
                info.values.insert("client_item_id".into(), json!(wanted));
                true
            })
        {
            let status = info.values.get("status").and_then(Value::as_str);
            let deletable = !matches!(status, Some("running" | "pending"))
                && solstone_core_import::import_trash::deletes_here(
                    &solstone_core_import::import_trash::source_of(&state.root, &timestamp),
                );
            let needs_review = row_needs_review(&state, &entry.path(), &timestamp, status);
            info.values.insert("deletable".into(), json!(deletable));
            // Its segments waiting in the trash, which an erase now takes too;
            // only when there are any, so a row's fields are as they were.
            if let Some(count) = waiting.get(&timestamp).filter(|count| **count > 0) {
                info.values.insert("waiting".into(), json!(count));
            }
            if needs_review {
                info.values.insert("needs_review".into(), json!(true));
            }
            rows.push(info);
        }
    }
    rows.sort_by(|left, right| {
        right
            .imported_at
            .total_cmp(&left.imported_at)
            // Filesystems with coarse ctime resolution can create several seeded
            // import directories in one tick. Preserve their chronological name
            // order for that otherwise indistinguishable case.
            .then_with(|| {
                right.values["timestamp"]
                    .as_str()
                    .cmp(&left.values["timestamp"].as_str())
            })
    });
    let page = query
        .get("page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let per_page = query
        .get("per_page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(25)
        .clamp(1, 100);
    let total = rows.len();
    let total_entries_written: i64 = rows
        .iter()
        .map(|row| {
            row.values
                .get("entries_written")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        })
        .sum();
    let total_entities_seeded: i64 = rows
        .iter()
        .map(|row| {
            row.values
                .get("entities_seeded")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        })
        .sum();
    let page_rows: Vec<Value> = rows
        .into_iter()
        .skip((page - 1).saturating_mul(per_page))
        .take(per_page)
        .map(|row| Value::Object(row.values))
        .collect();
    json_response(
        StatusCode::OK,
        json!({"imports": page_rows, "source_options": source_options, "total": total, "page": page, "per_page": per_page, "pages": if total == 0 { 0 } else { total.div_ceil(per_page) }, "total_entries_written": total_entries_written, "total_entities_seeded": total_entities_seeded}),
    )
}

fn row_needs_review(
    state: &AppState,
    directory: &Path,
    timestamp: &str,
    status: Option<&str>,
) -> bool {
    if status != Some("pending") {
        return false;
    }
    if state
        .route
        .as_ref()
        .is_some_and(|slot| slot.retains(timestamp))
    {
        return false;
    }
    if directory.join("imported.json").exists() {
        return false;
    }
    let Ok(metadata) = solstone_core_import::read_import_metadata(&state.root, timestamp) else {
        return false;
    };
    if metadata.get("is_local_path").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let hint = metadata
        .get("source_hint")
        .and_then(Value::as_str)
        .map(str::trim);
    if hint == Some("audio_archive") {
        return false;
    }
    let confirmation_source = hint == Some("apple_health")
        || metadata
            .get("confirmation_required")
            .and_then(Value::as_bool)
            == Some(true);
    if !confirmation_source {
        return false;
    }
    if solstone_core_import::saved_start::saved_start_state(&metadata, timestamp)
        != solstone_core_import::saved_start::SavedStartState::Eligible
    {
        return false;
    }
    let Some(file_path) = metadata.get("file_path").and_then(Value::as_str) else {
        return false;
    };
    if !Path::new(file_path).exists() {
        return false;
    }
    if crate::lifecycle::apple_card_already_ran(&state.root, timestamp, false) {
        return false;
    }
    let projection = solstone_core_import::project_import_result(&state.root, timestamp);
    projection.status == solstone_core_import::ProjectionStatus::Pending
        && projection.raw_publication.is_none()
}

pub(crate) async fn detail(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<AppState>,
    AxumPath(timestamp): AxumPath<String>,
) -> Response {
    let locale = response_locale(selected_locale);
    if solstone_core_import::validate_timestamp(&timestamp).is_err() {
        return import_not_found("Import not found");
    }
    crate::lifecycle::maybe_sweep_abandoned_uploads(&state.root);
    let guard = crate::start_lock::acquire(&state.root, &timestamp).await;
    match tokio::task::spawn_blocking(move || {
        let _guard = guard;
        detail_sync(state, timestamp, locale)
    })
    .await
    {
        Ok(response) => response,
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"could not read import progress"}),
        ),
    }
}

fn detail_sync(state: AppState, timestamp: String, locale: &str) -> Response {
    let Ok(info) = load_import_info(&state.root, &timestamp) else {
        return import_not_found("Import not found");
    };
    let directory = state.root.join("imports").join(&timestamp);
    let mut body = Map::new();
    body.insert("timestamp".into(), json!(timestamp));
    body.insert(
        "import_json".into(),
        object_from_file(&directory.join("import.json"))
            .map(Value::Object)
            .unwrap_or(Value::Null),
    );
    body.insert(
        "imported_json".into(),
        object_from_file(&directory.join("imported.json"))
            .map(Value::Object)
            .unwrap_or(Value::Null),
    );
    let mut saved_start = body
        .get("import_json")
        .and_then(|value| {
            serde_json::from_value::<solstone_core_import::ImportMetadata>(value.clone()).ok()
        })
        .map_or(
            solstone_core_import::saved_start::SavedStartState::Unknown,
            |metadata| solstone_core_import::saved_start::saved_start_state(&metadata, &timestamp),
        );
    if body
        .get("import_json")
        .and_then(Value::as_object)
        .is_some_and(crate::audio_archive::selected)
    {
        body.insert(
            "archive_active".into(),
            json!(
                state
                    .route
                    .as_ref()
                    .is_some_and(|slot| slot.retains(&timestamp))
            ),
        );
        for key in ["archive_run_id", "archive_lookup_unknown"] {
            if let Some(value) = info.values.get(key) {
                body.insert(key.into(), value.clone());
            }
        }
    }
    if let Ok(segments) = value_from_file(&directory.join("segments.json")) {
        body.insert("segments_json".into(), segments);
    }
    if untimed_transcript(&state.root, body.get("imported_json")) {
        body.insert("untimed_transcript".into(), json!(true));
    }
    let projection = solstone_core_import::project_import_result(&state.root, &timestamp);
    // Whether the page offers deleting it whole: not while it runs or waits to.
    body.insert(
        "deletable".into(),
        json!(
            !matches!(
                projection.status,
                solstone_core_import::ProjectionStatus::Running
                    | solstone_core_import::ProjectionStatus::Pending
            ) && solstone_core_import::import_trash::deletes_here(
                &solstone_core_import::import_trash::source_of(&state.root, &timestamp)
            )
        ),
    );
    let active = state
        .route
        .as_ref()
        .is_some_and(|slot| slot.retains(&timestamp));
    if saved_start == solstone_core_import::saved_start::SavedStartState::Eligible
        && (active
            || projection.status != solstone_core_import::ProjectionStatus::Pending
            || projection.raw_publication.is_some()
            || body
                .get("imported_json")
                .is_some_and(|value| !value.is_null()))
    {
        saved_start = solstone_core_import::saved_start::SavedStartState::Unknown;
    }
    body.insert(
        "saved_start".into(),
        json!({"state": saved_start, "active": active}),
    );
    // A journal-archive merge records its results in import.json; imports from earlier
    // builds carry them in imported.json.
    let records = ["imported_json", "import_json"].map(|record| body.get(record).cloned());
    let recorded = |key: &str| {
        records
            .iter()
            .flatten()
            .find_map(|record| record.get(key).filter(|value| !value.is_null()))
            .cloned()
    };
    if projection.merge_summary.is_some()
        && let (Some(decisions), Some(staging)) = (
            recorded("merge_log_path").and_then(|value| value.as_str().map(ToOwned::to_owned)),
            recorded("merge_staging_path").and_then(|value| value.as_str().map(ToOwned::to_owned)),
        )
    {
        body.insert(
            "merge_artifact_paths".into(),
            json!({"decisions": decisions, "staging": staging}),
        );
    }
    if let Some(errors) = recorded("summary_errors")
        .and_then(|errors| errors.as_array().cloned())
        .filter(|errors| !errors.is_empty())
    {
        body.insert("summary_errors".into(), Value::Array(errors));
    }
    let finished_at = projection
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.finished_at_ms)
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms as i64).ok())
        .map(|time| {
            let local = solstone_core_journal_config::owner_zone(&state.root).to_datetime(time);
            json!(local.strftime("%Y-%m-%dT%H:%M:%S").to_string())
        })
        .or_else(|| {
            recorded("processing_completed")
                .or_else(|| recorded("processing_failed"))
                .filter(Value::is_string)
        });
    for (key, value) in [
        ("finished_at", finished_at),
        (
            "principal_collision",
            projection.principal_collision.clone(),
        ),
        ("merge_summary", projection.merge_summary.clone()),
        (
            "staged_entities",
            projection
                .staged_entities
                .as_ref()
                .and_then(|s| serde_json::to_value(s).ok()),
        ),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), value);
        }
    }
    if projection.is_native() {
        body.extend(projection.native_row_overlay());
        // Same lowercase display name the list row carries for this source, when the
        // source has an upload tile. A source_type with no tile (e.g. `audio`) falls
        // back to the projection's own computed display rather than the raw source_type.
        if let Some(display) = source_display_localized(locale, &projection.source_type) {
            body.insert("source_display".into(), json!(display));
        } else {
            body.insert("source_display".into(), json!(projection.source_display));
        }
    }
    body.insert(
        "status".into(),
        info.values
            .get("status")
            .cloned()
            .unwrap_or_else(|| json!("pending")),
    );
    body.insert(
        "error".into(),
        info.values.get("error").cloned().unwrap_or(Value::Null),
    );
    body.insert(
        "error_stage".into(),
        info.values
            .get("error_stage")
            .cloned()
            .unwrap_or(Value::Null),
    );
    body.insert(
        "thinking".into(),
        crate::progress::read(&state.root, &timestamp),
    );
    let apple = crate::body_state::apple_imports(&state.root);
    crate::body_state::overlay(locale, &state.root, &timestamp, &mut body, &apple);
    json_response(StatusCode::OK, Value::Object(body))
}

/// Whether a text import landed untimed: a segment it published says, in its header, that
/// no entry's time was read from the file.
fn untimed_transcript(root: &Path, imported: Option<&Value>) -> bool {
    let Some(segments) = imported
        .and_then(|record| record.get("segments"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let plain =
        |part: &str| !part.is_empty() && !part.contains(['/', '\\']) && !matches!(part, "." | "..");
    segments.iter().any(|segment| {
        let field = |key| segment.get(key).and_then(Value::as_str).unwrap_or("");
        let (day, stream, key) = (field("day"), field("stream"), field("segment"));
        if !stream.starts_with("import.text") || ![day, stream, key].into_iter().all(plain) {
            return false;
        }
        let path = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(key)
            .join("conversation_transcript.jsonl");
        let Ok(file) = fs::File::open(path) else {
            return false;
        };
        let mut header = String::new();
        if BufRead::read_line(&mut BufReader::new(file), &mut header).is_err() {
            return false;
        }
        serde_json::from_str::<Value>(&header)
            .ok()
            .and_then(|header| header.get("untimed").and_then(Value::as_bool))
            == Some(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // A minimal native-producer import.json: enough for `is_native()` (via the attempt
    // block) and a stamped source_type, without a publication record — the display
    // fallback under test does not depend on how source_type was derived.
    fn write_native_attempt(journal: &Path, id: &str, source_type: &str) {
        let dir = journal.join("imports").join(id);
        fs::create_dir_all(&dir).unwrap();
        let metadata = json!({
            "original_filename": "meeting.m4a",
            "source_type": source_type,
            "task_id": id,
            "attempt": {
                "attempt_id": format!("{id}:1"),
                "generation": 1,
                "state": "completed",
                "started_at_ms": 1_000,
            }
        });
        fs::write(
            dir.join("import.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn an_apple_card_upload_that_stopped_early_is_listed_as_one_and_offered_for_delete() {
        // Saved as a generic upload, stopped by the journal before it wrote a
        // manifest: found walking the card in a browser, where it read "Text",
        // the filter offered "text", and the whole-import delete was offered.
        let dir = tempdir().unwrap();
        let id = "20261109_225511";
        let import = dir.path().join("imports").join(id);
        fs::create_dir_all(&import).unwrap();
        let metadata = json!({
            "original_filename": "export.zip",
            "source": "text",
            "source_hint": "apple_health",
            "task_id": id,
            "attempt": {"attempt_id": format!("{id}:1"), "generation": 1, "state": "unconfirmed",
                "started_at_ms": 1_000, "finished_at_ms": 2_000, "failure_reason": "import failed"},
        });
        fs::write(
            import.join("import.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        let state = AppState {
            root: dir.path().to_path_buf(),
            route: None,
        };
        let response = list(None, State(state.clone()), Query(HashMap::new())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let row = &body["imports"][0];
        assert_eq!(row["source_display"], "Apple Health", "{row}");
        assert_eq!(row["body_state"], "stopped", "{row}");
        assert_eq!(row["deletable"], true, "{row}");
        assert_eq!(body["source_options"], json!(["apple_health"]), "{body}");
        let detail = detail(None, State(state), axum::extract::Path(id.to_owned())).await;
        let bytes = axum::body::to_bytes(detail.into_body(), usize::MAX)
            .await
            .unwrap();
        let detail: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(detail["deletable"], true, "{detail}");
    }

    #[tokio::test]
    async fn source_choices_cover_history_while_pages_and_totals_match_filter() {
        let dir = tempdir().unwrap();
        for (id, source_type) in [
            ("20260101_120000", "image"),
            ("20260102_120000", "text"),
            ("20260103_120000", "text"),
        ] {
            write_native_attempt(dir.path(), id, source_type);
        }
        let state = AppState {
            root: dir.path().to_path_buf(),
            route: None,
        };
        for (filter, page, expected_total, expected_source) in [
            ("", "1", 3, "text"),
            ("image", "1", 1, "image"),
            ("text", "2", 2, "text"),
        ] {
            let query = HashMap::from([
                ("source".into(), filter.into()),
                ("page".into(), page.into()),
                ("per_page".into(), "1".into()),
            ]);
            let response = list(None, State(state.clone()), Query(query)).await;
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["source_options"], json!(["image", "text"]));
            assert_eq!(body["total"], expected_total);
            assert_eq!(body["pages"], expected_total);
            assert_eq!(body["imports"].as_array().unwrap().len(), 1);
            assert_eq!(body["imports"][0]["source_type"], expected_source);
        }
    }

    #[tokio::test]
    async fn a_syncs_runs_leave_history_and_an_owners_imports_stay() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let chosen = "20260101_120000";
        let plaud = "20260102_120000";
        let folder = "20260103_120000";
        let oura = "20260104_120000";
        for id in [chosen, plaud, folder] {
            write_native_attempt(root, id, "audio");
        }
        write_native_attempt(root, oura, "oura");
        let set = |id: &str, key: &str, value: &str| {
            let path = root.join("imports").join(id).join("import.json");
            let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            record[key] = json!(value);
            fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        };
        set(chosen, "source_hash", "sha256:owner");
        set(plaud, "plaud_file_id", "plaud-1");
        set(folder, "source_hash", "sha256:folder");
        fs::write(
            root.join("imports").join("audio.json"),
            serde_json::to_vec(&json!({"backend": "audio", "files": {
                "talk.m4a": {"status": "imported", "hash": "sha256:folder"},
                "skipped.m4a": {"status": "skipped", "hash": "sha256:owner"},
            }}))
            .unwrap(),
        )
        .unwrap();
        let state = AppState {
            root: root.to_path_buf(),
            route: None,
        };
        let response = list(None, State(state.clone()), Query(HashMap::new())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let listed: Vec<&str> = body["imports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["timestamp"].as_str().unwrap())
            .collect();
        // Oura stays listed until it has a home that deletes a whole run.
        assert_eq!(listed, [oura, chosen]);
        assert_eq!(body["total"], 2);
        // A sync's run keeps its own page, where the owner deletes it whole.
        let detail = detail_sync(state.clone(), plaud.to_owned(), "en");
        assert_eq!(detail.status(), StatusCode::OK);
        let folder_detail = detail_sync(state, folder.to_owned(), "en");
        assert_eq!(folder_detail.status(), StatusCode::OK);
    }

    #[test]
    fn every_offered_time_zone_is_one_the_journal_reads() {
        let names = time_zone_names();
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!names.iter().any(|name| name == "Factory"));
        for expected in ["America/Denver", "Asia/Calcutta", "US/Mountain", "UTC"] {
            assert!(names.iter().any(|name| name == expected), "{expected}");
        }
        for name in names {
            let zone = solstone_core_journal_config::parse_zone(name);
            assert_eq!(
                zone.as_ref().and_then(jiff::tz::TimeZone::iana_name),
                Some(name.as_str())
            );
        }
    }

    #[test]
    fn a_source_type_with_no_upload_tile_still_gets_a_display_name() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120000");
        write_native_attempt(journal, id, "audio");

        let info = load_import_info(journal, id).unwrap();
        assert_eq!(info.values["source_type"], "audio");
        assert_eq!(
            info.values["source_display"], "audio",
            "no SOURCES tile is named audio; the row must not go blank, and house casing \
             stays lowercase for a served list's own display name: {info:?}"
        );
    }

    #[test]
    fn a_source_type_with_an_upload_tile_is_unaffected_by_the_no_tile_fallback() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120001");
        write_native_attempt(journal, id, "image");

        let info = load_import_info(journal, id).unwrap();
        // Set by the pre-existing tile-gated path, not by the new fallback (which never
        // fires here because a tile exists) — this pins that the fallback is additive.
        assert_eq!(info.values["source_display"], "Image", "{info:?}");
    }

    #[tokio::test]
    async fn the_detail_route_says_when_a_text_import_landed_untimed() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        for (id, segment, header, untimed) in [
            (
                "20260101_120003",
                "120003_300",
                json!({"untimed": true}),
                true,
            ),
            (
                "20260101_130000",
                "130000_300",
                json!({"topics": "plans"}),
                false,
            ),
        ] {
            write_native_attempt(journal, id, "text");
            fs::write(
                journal.join("imports").join(id).join("imported.json"),
                serde_json::to_vec(&json!({"segments": [
                    {"day": "20260101", "stream": "import.text", "segment": segment}
                ]}))
                .unwrap(),
            )
            .unwrap();
            let segment_dir = journal.join("chronicle/20260101/import.text").join(segment);
            fs::create_dir_all(&segment_dir).unwrap();
            fs::write(
                segment_dir.join("conversation_transcript.jsonl"),
                format!(
                    "{header}\n{}\n",
                    json!({"start": "00:00:00", "text": "hello", "source": "import"})
                ),
            )
            .unwrap();

            let state = AppState {
                root: journal.to_path_buf(),
                route: None,
            };
            let response = detail(None, State(state), AxumPath(id.to_owned())).await;
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                body.get("untimed_transcript") == Some(&json!(true)),
                untimed,
                "{body:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_detail_route_names_an_audio_row_instead_of_the_raw_source_type() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120002");
        write_native_attempt(journal, id, "audio");

        let state = AppState {
            root: journal.to_path_buf(),
            route: None,
        };
        let response = detail(None, State(state), AxumPath(id.to_owned())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["source_display"], "audio", "{body:?}");
    }

    #[tokio::test]
    async fn the_detail_route_names_a_text_row_from_the_catalog_not_the_importer() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260101_120004");
        write_native_attempt(journal, id, "text");

        let state = AppState {
            root: journal.to_path_buf(),
            route: None,
        };
        let response = detail(None, State(state), AxumPath(id.to_owned())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body["source_display"],
            text(
                response_locale(None),
                "journal.window.import.import.source.names.text.text"
            ),
            "{body:?}"
        );
    }

    #[tokio::test]
    async fn list_and_detail_project_completed_attempt_only_archive_as_success() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260809_120000");
        let import_dir = journal.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        let metadata = json!({
            "original_filename": "archive.zip",
            "source_type": "journal_archive",
            "source_hint": "journal_archive",
            "task_id": id,
            "attempt": {
                "attempt_id": format!("{id}:1"),
                "generation": 1,
                "state": "completed",
                "started_at_ms": 1_000,
            }
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();

        let state = AppState {
            root: journal.to_path_buf(),
            route: None,
        };

        // Test list endpoint
        let list_resp = list(None, State(state.clone()), Query(HashMap::new())).await;
        let list_bytes = axum::body::to_bytes(list_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let list_body: Value = serde_json::from_slice(&list_bytes).unwrap();
        let imports = list_body["imports"].as_array().expect("imports array");
        let row = imports
            .iter()
            .find(|r| r["timestamp"] == id)
            .expect("row exists");
        assert_eq!(row["status"], "success", "{row:?}");
        assert_eq!(row["source_type"], "journal_archive", "{row:?}");
        assert!(row["error"].is_null(), "{row:?}");

        // Test detail endpoint
        let response = detail(None, State(state), AxumPath(id.to_owned())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "success", "{body:?}");
        assert_eq!(body["source_type"], "journal_archive", "{body:?}");
        assert!(body["error"].is_null(), "{body:?}");
    }

    #[tokio::test]
    async fn lock_failed_import_surfaces_safe_error_string_without_raw_path() {
        let dir = tempdir().unwrap();
        let (journal, id) = (dir.path(), "20260809_120001");
        let import_dir = journal.join("imports").join(id);
        fs::create_dir_all(&import_dir).unwrap();
        let metadata = json!({
            "source_type": "journal_archive",
            "source_hint": "journal_archive",
            "attempt": {
                "attempt_id": format!("{id}:1"),
                "generation": 1,
                "state": "unconfirmed",
                "started_at_ms": 1_000,
                "finished_at_ms": 2_000,
                "duration_ms": 1_000,
                "failure_reason": "import failed"
            }
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();

        let state = AppState {
            root: journal.to_path_buf(),
            route: None,
        };

        // Test list endpoint
        let list_resp = list(None, State(state.clone()), Query(HashMap::new())).await;
        let list_bytes = axum::body::to_bytes(list_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let list_body: Value = serde_json::from_slice(&list_bytes).unwrap();
        let imports = list_body["imports"].as_array().expect("imports array");
        let row = imports
            .iter()
            .find(|r| r["timestamp"] == id)
            .expect("row exists");
        assert_eq!(row["status"], "failed", "{row:?}");
        assert_eq!(row["error"], "import failed", "{row:?}");

        // Test detail endpoint
        let response = detail(None, State(state), AxumPath(id.to_owned())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "failed", "{body:?}");
        assert_eq!(body["error"], "import failed", "{body:?}");

        // Assert neither error nor any detail field exposes the absolute journal path
        let raw_path = journal.to_string_lossy();
        fn assert_no_path(val: &Value, raw_path: &str) {
            match val {
                Value::String(s) => {
                    assert!(
                        !s.contains(raw_path),
                        "value contained raw journal path: {s}"
                    );
                }
                Value::Array(arr) => {
                    for item in arr {
                        assert_no_path(item, raw_path);
                    }
                }
                Value::Object(map) => {
                    for (k, v) in map {
                        assert!(!k.contains(raw_path), "key contained raw journal path: {k}");
                        assert_no_path(v, raw_path);
                    }
                }
                _ => {}
            }
        }
        assert_no_path(&body, &raw_path);
        assert_no_path(&list_body, &raw_path);
    }

    #[tokio::test]
    async fn list_emits_needs_review_only_for_eligible_confirmation_required_staged_imports() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let seed = |id: &str, filename: Option<&str>, extra: Value| {
            let import_dir = journal.join("imports").join(id);
            fs::create_dir_all(&import_dir).unwrap();
            let file_path = import_dir.join("export.zip");
            if let Some(bytes) = filename {
                fs::write(&file_path, bytes.as_bytes()).unwrap();
            }
            let mut meta: solstone_core_import::ImportMetadata = serde_json::from_value(json!({
                "import_id": id,
                "original_filename": "export.zip",
                "file_path": file_path.to_string_lossy(),
                "file_size": 12,
                "source_hash": "0123456789abcdef",
                "source_timestamp": id,
                "source_hint": "apple_health",
                "is_local_path": false,
            }))
            .unwrap();
            solstone_core_import::saved_start::initialize_saved_start(&mut meta, id).unwrap();
            if let Some(src) = extra.as_object() {
                for (k, v) in src {
                    meta.insert(k.clone(), v.clone());
                }
            }
            fs::write(
                import_dir.join("import.json"),
                serde_json::to_vec(&meta).unwrap(),
            )
            .unwrap();
            import_dir
        };

        // Positive: eligible pending Apple Health upload with file on disk.
        let eligible_id = "20261122_150001";
        seed(eligible_id, Some("apple-export"), json!({}));

        // Negative 1: is_local_path = true
        let local_id = "20261122_150002";
        seed(
            local_id,
            Some("apple-export"),
            json!({"is_local_path": true}),
        );

        // Negative 2: audio_archive source_hint
        let audio_id = "20261122_150003";
        seed(
            audio_id,
            Some("audio-zip"),
            json!({"source_hint": "audio_archive"}),
        );

        // Negative 3: raw_publication present (partial imported.json with no status)
        let raw_pub_id = "20261122_150004";
        let raw_pub_dir = seed(raw_pub_id, Some("apple-export"), json!({}));
        fs::write(
            raw_pub_dir.join("imported.json"),
            br#"{"entries_written":0}"#,
        )
        .unwrap();

        // Negative 4: imported.json exists (completed)
        let done_id = "20261122_150005";
        let done_dir = seed(done_id, Some("apple-export"), json!({}));
        fs::write(done_dir.join("imported.json"), br#"{"status":"success"}"#).unwrap();

        // Negative 5: apple_card_already_ran (body-progress.json present)
        let ran_id = "20261122_150006";
        let ran_dir = seed(ran_id, Some("apple-export"), json!({}));
        fs::write(ran_dir.join("body-progress.json"), br#"{"phase":"write"}"#).unwrap();

        // Negative 6: missing staged file on disk
        let missing_file_id = "20261122_150007";
        seed(missing_file_id, None, json!({}));

        // Negative 7: non-confirmation source (e.g. chatgpt plan_failed orphan)
        let chatgpt_id = "20261122_150008";
        seed(
            chatgpt_id,
            Some("conversations.json"),
            json!({"source_hint": "chatgpt"}),
        );

        let state = AppState {
            root: journal.to_path_buf(),
            route: None,
        };
        let list_resp = list(None, State(state), Query(HashMap::new())).await;
        let list_bytes = axum::body::to_bytes(list_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let list_body: Value = serde_json::from_slice(&list_bytes).unwrap();
        let imports = list_body["imports"].as_array().expect("imports array");
        let row_for = |id: &str| {
            imports
                .iter()
                .find(|r| r["timestamp"] == id)
                .unwrap_or_else(|| panic!("row {id} missing from list"))
        };

        assert_eq!(row_for(eligible_id).get("needs_review"), Some(&json!(true)));
        for neg in [
            local_id,
            audio_id,
            raw_pub_id,
            done_id,
            ran_id,
            missing_file_id,
            chatgpt_id,
        ] {
            assert!(
                row_for(neg).get("needs_review").is_none(),
                "negative twin {neg} must omit needs_review: {:?}",
                row_for(neg)
            );
        }
    }
}
