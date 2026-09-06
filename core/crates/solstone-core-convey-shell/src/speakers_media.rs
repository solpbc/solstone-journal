// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! People lookup and journal-contained media serving for speakers.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Extension, Path as RoutePath, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::JournalRoot;
use crate::speakers_calendar::{
    is_day, journal_principal_id, load_all_journal_entities, value_truthy,
};

const PEOPLE_SEARCH_LIMIT: usize = 8;

#[derive(Debug, Deserialize)]
pub struct PeopleSearchQuery {
    q: Option<String>,
}

pub async fn people_search(
    Extension(root): Extension<Arc<JournalRoot>>,
    Query(query): Query<PeopleSearchQuery>,
) -> Response {
    let query = query.q.unwrap_or_default().trim().to_owned();
    if query.is_empty() {
        return Json(json!({"query": "", "people": []})).into_response();
    }
    // The frozen fixture is ASCII; lowercase is the intentionally narrow stand-in
    // for Python's Unicode-aware casefold on this wave's read-only surface.
    let folded_query = query.to_lowercase();
    let principal_id = journal_principal_id(&root.0);
    let mut people = load_all_journal_entities(&root.0)
        .into_iter()
        .filter_map(|(directory_id, entity)| {
            let entity_id = entity.get("id").and_then(Value::as_str)?.to_owned();
            let name = entity.get("name").and_then(Value::as_str)?.to_owned();
            is_speaker_attach_candidate(&entity, &entity_id, principal_id.as_deref()).then_some((directory_id, entity_id, name, entity))
        })
        .filter(|(_, _, _, entity)| person_search_strings(entity).iter().any(|value| value.to_lowercase().contains(&folded_query)))
        .map(|(_, entity_id, name, _)| {
            // Read-only presence badge. Merge bookkeeping resolves through
            // entity_memory_path; this listing does not write.
            json!({
                "entity_id": entity_id,
                "name": name,
                "has_voice": root.0.join("entities").join(&entity_id).join("voiceprints.npz").is_file(),
            })
        })
        .collect::<Vec<_>>();
    people.sort_by(|left, right| {
        left["name"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .cmp(&right["name"].as_str().unwrap_or_default().to_lowercase())
            .then_with(|| left["entity_id"].as_str().cmp(&right["entity_id"].as_str()))
    });
    people.truncate(PEOPLE_SEARCH_LIMIT);
    Json(json!({"query": query, "people": people})).into_response()
}

pub async fn serve_audio(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath((day, rel_path)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !is_day(&day) {
        return media_error(
            "invalid_day",
            "that day couldn't be used.",
            "Day not found",
            StatusCode::NOT_FOUND,
        )
        .into_response();
    }
    let day_root = root.0.join("chronicle").join(&day);
    let path = match safe_day_path(&day_root, &rel_path) {
        Some(path) => path,
        None => {
            return media_error(
                "invalid_path",
                "that path couldn't be used.",
                "",
                StatusCode::FORBIDDEN,
            )
            .into_response();
        }
    };
    if !path.is_file() {
        return media_error(
            "file_not_found",
            "that file isn't available.",
            "File not found",
            StatusCode::NOT_FOUND,
        )
        .into_response();
    }
    let Some(mimetype) = mime_type(&path) else {
        // Declared frozen-oracle deviation: Python lets this unregistered,
        // existing file reach a global 500. Refuse cleanly instead of panicking.
        return media_error(
            "invalid_request_value",
            "one of those values couldn't be used.",
            "Unregistered media extension",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            return media_error(
                "file_not_found",
                "that file isn't available.",
                "File not found",
                StatusCode::NOT_FOUND,
            )
            .into_response();
        }
    };
    if bytes.is_empty() {
        return media_response(StatusCode::OK, mimetype, &path, Vec::new(), None);
    }
    match headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(|value| parse_range(value, bytes.len()))
    {
        Some(ParsedRange::Valid { start, end }) => {
            let total = bytes.len();
            media_response(
                StatusCode::PARTIAL_CONTENT,
                mimetype,
                &path,
                bytes[start..=end].to_vec(),
                Some(format!("bytes {start}-{end}/{total}")),
            )
        }
        Some(ParsedRange::Unsatisfiable) => media_error(
            "http_error",
            "that request didn't finish.",
            "",
            StatusCode::RANGE_NOT_SATISFIABLE,
        )
        .into_response(),
        Some(ParsedRange::Ignore) | None => {
            media_response(StatusCode::OK, mimetype, &path, bytes, None)
        }
    }
}

fn is_speaker_attach_candidate(
    entity: &Value,
    entity_id: &str,
    principal_id: Option<&str>,
) -> bool {
    !entity_id.is_empty()
        && entity
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty())
        && entity.get("type").and_then(Value::as_str) == Some("Person")
        && !entity.get("blocked").is_some_and(value_truthy)
        && !entity.get("is_principal").is_some_and(value_truthy)
        && principal_id != Some(entity_id)
}

fn person_search_strings(entity: &Value) -> Vec<&str> {
    std::iter::once(
        entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .chain(
        entity
            .get("aka")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    )
    .collect()
}

enum ParsedRange {
    Valid { start: usize, end: usize },
    Unsatisfiable,
    Ignore,
}

fn parse_range(value: &str, total: usize) -> ParsedRange {
    let Some(spec) = value.strip_prefix("bytes=") else {
        return ParsedRange::Ignore;
    };
    if spec.contains(',') {
        return ParsedRange::Ignore;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return ParsedRange::Ignore;
    };
    if start.is_empty() {
        let Ok(length) = end.parse::<usize>() else {
            return ParsedRange::Ignore;
        };
        if length == 0 {
            return ParsedRange::Unsatisfiable;
        }
        let start = total.saturating_sub(length);
        return ParsedRange::Valid {
            start,
            end: total - 1,
        };
    }
    let Ok(start) = start.parse::<usize>() else {
        return ParsedRange::Ignore;
    };
    if start >= total {
        return ParsedRange::Unsatisfiable;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        let Ok(end) = end.parse::<usize>() else {
            return ParsedRange::Ignore;
        };
        end.min(total - 1)
    };
    if end < start {
        ParsedRange::Unsatisfiable
    } else {
        ParsedRange::Valid { start, end }
    }
}

fn media_response(
    status: StatusCode,
    mimetype: &str,
    path: &Path,
    bytes: Vec<u8>,
    content_range: Option<String>,
) -> Response {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mimetype)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename={filename}"),
        )
        .header(header::CACHE_CONTROL, "public, max-age=300");
    if let Some(content_range) = content_range {
        response = response
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_RANGE, content_range);
    }
    response
        .body(Body::from(bytes))
        .expect("media response builds")
}

fn media_error(reason_code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    // Flask's jsonify appends a newline, including on these error envelopes.
    let body = format!(
        "{}\n",
        json!({"error": message, "reason_code": reason_code, "detail": detail})
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("media error response builds")
}

fn mime_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "flac" => Some("audio/flac"),
        "opus" => Some("audio/opus"),
        "ogg" => Some("audio/ogg"),
        "m4a" => Some("audio/mp4"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "webm" => Some("video/webm"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "heic" => Some("image/heic"),
        "heif" => Some("image/heif"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "tiff" => Some("image/tiff"),
        _ => None,
    }
}

fn safe_day_path(day_root: &Path, rel_path: &str) -> Option<PathBuf> {
    if rel_path.is_empty() || rel_path.starts_with('/') || rel_path.contains('\\') {
        return None;
    }
    let parts = rel_path.split('/').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| part.is_empty() || matches!(*part, "." | ".."))
    {
        return None;
    }
    let candidate = if parts[0].len() == 8 && parts[0].bytes().all(|byte| byte.is_ascii_digit()) {
        day_root.join("chronicle").join(rel_path)
    } else {
        day_root.join(rel_path)
    };
    let root_real = real_path_non_strict(day_root)?;
    let candidate_real = real_path_non_strict(&candidate)?;
    candidate_real
        .starts_with(&root_real)
        .then_some(candidate_real)
}

fn real_path_non_strict(path: &Path) -> Option<PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        suffix.push(existing.file_name()?.to_owned());
        existing = existing.parent()?;
    }
    let mut resolved = fs::canonicalize(existing).ok()?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Some(resolved)
}
