// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! People lookup and journal-contained media serving for speakers.

use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Extension, Path as RoutePath, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use solstone_core_convey_http::media::{
    MediaBody, ParsedRange, media_response, mime_type, parse_range,
};

use crate::JournalRoot;
use crate::speakers_calendar::{is_day, journal_principal_id, live_entities, value_truthy};

const PEOPLE_SEARCH_LIMIT: usize = 8;

#[derive(Debug, Deserialize)]
pub struct PeopleSearchQuery {
    q: Option<String>,
}

use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};

pub async fn people_search(
    Extension(root): Extension<Arc<JournalRoot>>,
    Query(query): Query<PeopleSearchQuery>,
) -> Response {
    let query = query.q.unwrap_or_default().trim().to_owned();
    if query.is_empty() {
        return Json(json!({"query": "", "people": []})).into_response();
    }
    spawn_blocking_response(OwnerReadRole::SpeakersPeopleSearch, move || {
        // The frozen fixture is ASCII; lowercase is the intentionally narrow stand-in
        // for Python's Unicode-aware casefold on this wave's read-only surface.
        let folded_query = query.to_lowercase();
        let principal_id = journal_principal_id(&root.0);
        let mut people = live_entities(&root.0)
            .into_iter()
            .filter_map(|(directory, entity)| {
                let entity_id = entity.id;
                let entity = entity.value;
                let name = entity.get("name").and_then(Value::as_str)?.to_owned();
                is_speaker_attach_candidate(&entity, &entity_id, principal_id.as_deref()).then_some((directory, entity_id, name, entity))
            })
            .filter(|(_, _, _, entity)| person_search_strings(entity).iter().any(|value| value.to_lowercase().contains(&folded_query)))
            .map(|(directory, entity_id, name, _)| {
                // Read-only presence badge, from the folder that holds the
                // entity; this listing does not write.
                json!({
                    "entity_id": entity_id,
                    "name": name,
                    "has_voice": solstone_core_entity::entity_voiceprints_path(&root.0, &directory).is_ok_and(|path| path.is_file()),
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
    })
    .await
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
    spawn_blocking_response(OwnerReadRole::SpeakersServeAudio, move || {
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
        let response = (|| {
            let file = fs::File::open(&path)?;
            let total = file.metadata()?.len();
            audio_response(file, total, mimetype, &path, &headers)
        })();
        response.unwrap_or_else(|_: std::io::Error| {
            media_error(
                "file_not_found",
                "that file isn't available.",
                "File not found",
                StatusCode::NOT_FOUND,
            )
        })
    })
    .await
}

// Length comes from the opened file. Serve only that observed interval; a
// short read refuses the response, while growth after observation is excluded.
fn audio_response(
    file: impl Read + Seek + Send + 'static,
    total: u64,
    mimetype: &str,
    path: &Path,
    headers: &HeaderMap,
) -> std::io::Result<Response> {
    if total == 0 {
        return Ok(media_response(
            StatusCode::OK,
            mimetype,
            path,
            MediaBody::empty(),
            None,
        ));
    }
    let total = usize::try_from(total).map_err(std::io::Error::other)?;
    let (status, start, length, content_range) = match headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(|value| parse_range(value, total))
    {
        Some(ParsedRange::Valid { start, end }) => (
            StatusCode::PARTIAL_CONTENT,
            start,
            end - start + 1,
            Some(format!("bytes {start}-{end}/{total}")),
        ),
        Some(ParsedRange::Unsatisfiable) => {
            return Ok(media_error(
                "http_error",
                "that request didn't finish.",
                "",
                StatusCode::RANGE_NOT_SATISFIABLE,
            ));
        }
        Some(ParsedRange::Ignore) | None => (StatusCode::OK, 0, total, None),
    };
    let body = MediaBody::from_reader(file, start, length)?;
    Ok(media_response(status, mimetype, path, body, content_range))
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
        && !solstone_core_entity::identity_is_principal(entity)
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

#[cfg(test)]
mod tests {
    use super::audio_response;
    use axum::http::{HeaderMap, StatusCode, header};
    use std::io::{Cursor, ErrorKind};
    use std::path::Path;

    #[test]
    fn audio_uses_observed_length_and_refuses_truncation_within_selected_bytes() {
        let file = || Cursor::new(b"abc".to_vec());
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=1-2".parse().unwrap());
        let response =
            audio_response(file(), 5, "audio/flac", Path::new("mic.flac"), &headers).unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 1-2/5");
        assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
        headers.insert(header::RANGE, "bytes=1-4".parse().unwrap());
        assert_eq!(
            audio_response(file(), 5, "audio/flac", Path::new("mic.flac"), &headers)
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof
        );
        headers.clear();
        assert_eq!(
            audio_response(file(), 5, "audio/flac", Path::new("mic.flac"), &headers)
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof
        );
        let response =
            audio_response(file(), 2, "audio/flac", Path::new("mic.flac"), &headers).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "2");
        assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[test]
    #[cfg(unix)]
    fn safe_day_path_refuses_a_symlink_that_escapes_the_journal() {
        use super::safe_day_path;
        use std::fs;
        use std::os::unix::fs::symlink;

        let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        let outside = tempfile::TempDir::new_in("/var/tmp").expect("outside root");
        fs::write(outside.path().join("secret.flac"), b"secret").expect("outside file");
        symlink(outside.path(), journal.path().join("speaker-media")).expect("symlink");

        assert_eq!(
            safe_day_path(journal.path(), "speaker-media/secret.flac"),
            None
        );
    }
}
