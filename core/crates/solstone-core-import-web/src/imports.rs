// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Map, Value, json};

use crate::{
    AppState,
    http::{import_not_found, json as json_response},
};

const TIMEOUT_SECONDS: f64 = 3600.0;

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
        display_name: "calendar",
        icon: "calendar",
        description: "import events from Google Calendar, Apple Calendar, or Outlook",
        input_type: "file",
        upload_prompt: "upload your .ics file or .zip export",
        has_guide: true,
        accept: ".ics,.zip",
    },
    SourceMetadata {
        name: "chatgpt",
        display_name: "ChatGPT",
        icon: "message-square",
        description: "import your conversation history from ChatGPT",
        input_type: "file",
        upload_prompt: "upload your ChatGPT export .zip file",
        has_guide: true,
        accept: ".zip",
    },
    SourceMetadata {
        name: "claude",
        display_name: "Claude",
        icon: "message-circle",
        description: "import your conversation history from Claude",
        input_type: "file",
        upload_prompt: "upload your Claude export .zip file",
        has_guide: true,
        accept: ".zip",
    },
    SourceMetadata {
        name: "gemini",
        display_name: "Gemini",
        icon: "sparkles",
        description: "import your activity history from Google Gemini",
        input_type: "file",
        upload_prompt: "upload your Google Takeout .zip file",
        has_guide: true,
        accept: ".zip,.json",
    },
    SourceMetadata {
        name: "obsidian",
        display_name: "notes",
        icon: "file-text",
        description: "import notes from Obsidian, Logseq, or any markdown vault",
        input_type: "path_input",
        upload_prompt: "paste the full path to your vault folder",
        has_guide: true,
        accept: "",
    },
    SourceMetadata {
        name: "kindle",
        display_name: "Kindle",
        icon: "book-open",
        description: "import highlights and clippings from your Kindle",
        input_type: "file",
        upload_prompt: "upload your My Clippings.txt file",
        has_guide: true,
        accept: ".txt",
    },
    SourceMetadata {
        name: "journal_archive",
        display_name: "journal",
        icon: "book",
        description: "import a full journal export from another journal",
        input_type: "file",
        upload_prompt: "upload your journal export .zip file",
        has_guide: true,
        accept: ".zip",
    },
    SourceMetadata {
        name: "recording",
        display_name: "meeting audio",
        icon: "mic",
        description: "import audio from meetings or conversations",
        input_type: "file",
        upload_prompt: "upload an audio, video, or image file",
        has_guide: false,
        accept: ".flac,.gif,.heic,.heif,.jpeg,.jpg,.m4a,.mov,.mp3,.mp4,.ogg,.opus,.png,.tiff,.wav,.webm,.webp",
    },
    SourceMetadata {
        name: "document",
        display_name: "document",
        icon: "file",
        description: "import a PDF document",
        input_type: "file",
        upload_prompt: "upload a PDF file",
        has_guide: false,
        accept: ".pdf",
    },
    SourceMetadata {
        name: "image",
        display_name: "image",
        icon: "image",
        description: "add a photo or screenshot and let a model describe what's in it",
        input_type: "file",
        upload_prompt: "upload an image (PNG, JPEG, WebP, GIF, TIFF)",
        has_guide: false,
        accept: ".png,.jpg,.jpeg,.webp,.gif,.tiff",
    },
    SourceMetadata {
        name: "quick",
        display_name: "quick import",
        icon: "zap",
        description: "paste text or drop any file for quick import",
        input_type: "text",
        upload_prompt: "paste text or drag and drop a file",
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
        .duration_since(UNIX_EPOCH)
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

fn decision_highlights(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    let mut staged_entities = Vec::new();
    let mut errored_segments = Vec::new();
    let mut qualifying = 0;
    for line in text.lines() {
        if qualifying >= 50 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match row.get("action").and_then(Value::as_str) {
            Some("entity_staged") => {
                staged_entities.push(json!({
                    "source_name": row["source"]["name"],
                    "target_name": row["target"]["name"],
                    "staging_path": row["staging_path"],
                }));
                qualifying += 1;
            }
            Some("segment_errored") => {
                errored_segments.push(json!({
                    "item_id": row["item_id"],
                    "reason": row["reason"],
                }));
                qualifying += 1;
            }
            _ => {}
        }
    }
    (!staged_entities.is_empty() || !errored_segments.is_empty()).then(|| {
        json!({
            "staged_entities": staged_entities,
            "errored_segments": errored_segments,
        })
    })
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
    Ok(ImportInfo {
        imported_at,
        values,
    })
}

pub(crate) fn resolve_status(info: &ImportInfo, now: f64) -> (&'static str, Value, Value) {
    resolve_status_with_timeout(info, now, TIMEOUT_SECONDS)
}

pub(crate) fn resolve_status_with_timeout(
    info: &ImportInfo,
    now: f64,
    timeout_seconds: f64,
) -> (&'static str, Value, Value) {
    let error = info.values["error"].clone();
    let error_stage = info.values["error_stage"].clone();
    if !error.is_null() {
        return ("failed", error, error_stage);
    }
    if info.values["processed"] == Value::Bool(true)
        || info.values.get("processing_completed").is_some()
    {
        return ("success", error, error_stage);
    }
    if info
        .values
        .get("task_id")
        .is_some_and(|value| !value.is_null())
    {
        if now - info.imported_at > timeout_seconds {
            return ("failed", json!("Import never completed"), json!("timeout"));
        }
        return ("running", error, error_stage);
    }
    ("pending", error, error_stage)
}

fn source(name: &str) -> Option<&'static SourceMetadata> {
    SOURCES.iter().find(|item| item.name == name)
}
pub(crate) fn source_icon(name: &str) -> Option<String> {
    source(name).and_then(|item| ICONS.get(item.icon).cloned())
}
pub(crate) fn source_display(name: &str) -> Option<String> {
    source(name).map(|item| item.display_name.to_owned())
}

pub(crate) async fn sources() -> Response {
    let items: Vec<Value> = SOURCES.iter().map(|item| json!({
        "name": item.name, "display_name": item.display_name, "icon": item.icon,
        "description": item.description, "input_type": item.input_type, "upload_prompt": item.upload_prompt,
        "has_guide": item.has_guide, "accept": item.accept, "icon_svg": ICONS.get(item.icon),
    })).collect();
    json_response(
        StatusCode::OK,
        json!({"items": items, "total": SOURCES.len()}),
    )
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let filter = query
        .get("source")
        .filter(|value| !value.is_empty())
        .map(String::as_str);
    let imports_dir = state.root.join("imports");
    let Ok(entries) = fs::read_dir(&imports_dir) else {
        return json_response(
            StatusCode::OK,
            json!({"imports": [], "total": 0, "page": 1, "per_page": 25, "pages": 0, "total_entries_written": 0, "total_entities_seeded": 0}),
        );
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let timestamp = entry.file_name().to_string_lossy().into_owned();
        if timestamp.len() != 15 || timestamp.matches('_').count() != 1 || !entry.path().is_dir() {
            continue;
        }
        let Ok(mut info) = load_import_info(&state.root, &timestamp) else {
            continue;
        };
        let source_type = info
            .values
            .get("source_type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(display) = source_type
            .as_deref()
            .and_then(source)
            .map(|item| item.display_name)
        {
            info.values.insert("source_display".into(), json!(display));
        }
        let (status, err, stage) = resolve_status(
            &info,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        );
        info.values.insert("status".into(), json!(status));
        info.values.insert("error".into(), err);
        info.values.insert("error_stage".into(), stage);
        if filter.is_none_or(|wanted| source_type.as_deref() == Some(wanted)) {
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
        .skip((page - 1) * per_page)
        .take(per_page)
        .map(|row| Value::Object(row.values))
        .collect();
    json_response(
        StatusCode::OK,
        json!({"imports": page_rows, "total": total, "page": page, "per_page": per_page, "pages": if total == 0 { 0 } else { total.div_ceil(per_page) }, "total_entries_written": total_entries_written, "total_entities_seeded": total_entities_seeded}),
    )
}

pub(crate) async fn detail(
    State(state): State<AppState>,
    AxumPath(timestamp): AxumPath<String>,
) -> Response {
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
    if let Ok(segments) = value_from_file(&directory.join("segments.json")) {
        body.insert("segments_json".into(), segments);
    }
    let imported = body.get("imported_json").cloned();
    if let Some(imported) = imported.as_ref().and_then(Value::as_object)
        && !imported
            .get("merge_summary")
            .unwrap_or(&Value::Null)
            .is_null()
        && let (Some(decisions), Some(staging)) = (
            imported
                .get("merge_log_path")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            imported
                .get("merge_staging_path")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        )
    {
        body.insert(
            "merge_artifact_paths".into(),
            json!({"decisions": decisions, "staging": staging}),
        );
        if let Some(highlights) = decision_highlights(Path::new(&decisions)) {
            body.insert("decision_highlights".into(), highlights);
        }
    }
    if let Some(errors) = imported
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|imported| imported.get("summary_errors"))
        .and_then(Value::as_array)
        .filter(|errors| !errors.is_empty())
    {
        body.insert("summary_errors".into(), Value::Array(errors.clone()));
    }
    let (status, error_value, stage) = resolve_status(
        &info,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64(),
    );
    body.insert("status".into(), json!(status));
    body.insert("error".into(), error_value);
    body.insert("error_stage".into(), stage);
    json_response(StatusCode::OK, Value::Object(body))
}
