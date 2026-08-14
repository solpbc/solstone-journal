// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as RoutePath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use solstone_core_journal_io::paths::contained_path;

use crate::day::valid_day;
use crate::{AppState, legacy_error_response};

pub(crate) async fn serve_file(
    State(state): State<Arc<AppState>>,
    RoutePath((day, rel_path)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !valid_day(&day) {
        return error(
            "invalid_day",
            "I couldn't use that day.",
            "Day not found",
            StatusCode::NOT_FOUND,
        );
    }
    let day_root = state.journal_root.join("chronicle").join(&day);
    let path = match contained_path(&day_root, &rel_path) {
        Ok(path) => path,
        Err(_) => {
            return error(
                "invalid_path",
                "I couldn't use that path.",
                "",
                StatusCode::FORBIDDEN,
            );
        }
    };
    if !path.is_file() {
        return error(
            "file_not_found",
            "I couldn't find that file.",
            "File not found",
            StatusCode::NOT_FOUND,
        );
    }
    let Some(mime) = mime_type(&path) else {
        return error(
            "invalid_request_value",
            "I couldn't use one of those values.",
            "Unregistered media extension",
            StatusCode::BAD_REQUEST,
        );
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(
                "file_not_found",
                "I couldn't find that file.",
                "File not found",
                StatusCode::NOT_FOUND,
            );
        }
    };
    if bytes.is_empty() {
        return media(StatusCode::OK, mime, &path, bytes, None);
    }
    match headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, bytes.len()))
    {
        Some(ParsedRange::Valid { start, end }) => media(
            StatusCode::PARTIAL_CONTENT,
            mime,
            &path,
            bytes[start..=end].to_vec(),
            Some(format!("bytes {start}-{end}/{}", bytes.len())),
        ),
        Some(ParsedRange::Unsatisfiable) => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{}", bytes.len()))
            .body(Body::empty())
            .expect("response"),
        _ => media(StatusCode::OK, mime, &path, bytes, None),
    }
}

fn error(reason: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    legacy_error_response(reason, message, detail, status)
}

fn media(
    status: StatusCode,
    mime: &str,
    path: &Path,
    bytes: Vec<u8>,
    range: Option<String>,
) -> Response {
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename={filename}"),
        )
        .header(header::CACHE_CONTROL, "public, max-age=300");
    if let Some(range) = range {
        response = response
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_RANGE, range);
    }
    response.body(Body::from(bytes)).expect("response")
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
        return ParsedRange::Valid {
            start: total.saturating_sub(length),
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
