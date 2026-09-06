// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Read, Seek, SeekFrom};
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
            "that day couldn't be used.",
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
                "that path couldn't be used.",
                "",
                StatusCode::FORBIDDEN,
            );
        }
    };
    if !path.is_file() {
        return error(
            "file_not_found",
            "that file isn't available.",
            "File not found",
            StatusCode::NOT_FOUND,
        );
    }
    let Some(mime) = mime_type(&path) else {
        return error(
            "invalid_request_value",
            "one of those values couldn't be used.",
            "Unregistered media extension",
            StatusCode::BAD_REQUEST,
        );
    };
    let total = match path.metadata().map(|metadata| metadata.len()) {
        Ok(total) => total,
        Err(_) => {
            return error(
                "file_not_found",
                "that file isn't available.",
                "File not found",
                StatusCode::NOT_FOUND,
            );
        }
    };
    if total == 0 {
        return media(StatusCode::OK, mime, &path, Vec::new(), None);
    }
    let Ok(total) = usize::try_from(total) else {
        return error(
            "file_not_found",
            "that file isn't available.",
            "File not found",
            StatusCode::NOT_FOUND,
        );
    };
    match headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, total))
    {
        Some(ParsedRange::Valid { start, end }) => {
            match read_bytes(&path, start, end - start + 1) {
                Ok(bytes) => media(
                    StatusCode::PARTIAL_CONTENT,
                    mime,
                    &path,
                    bytes,
                    Some(format!("bytes {start}-{end}/{total}")),
                ),
                Err(_) => error(
                    "file_not_found",
                    "that file isn't available.",
                    "File not found",
                    StatusCode::NOT_FOUND,
                ),
            }
        }
        Some(ParsedRange::Unsatisfiable) => error(
            "http_error",
            "that request didn't finish.",
            "",
            StatusCode::RANGE_NOT_SATISFIABLE,
        ),
        _ => match read_bytes(&path, 0, total) {
            Ok(bytes) => media(StatusCode::OK, mime, &path, bytes, None),
            Err(_) => error(
                "file_not_found",
                "that file isn't available.",
                "File not found",
                StatusCode::NOT_FOUND,
            ),
        },
    }
}

fn read_bytes(path: &Path, start: usize, length: usize) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(start as u64))?;
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
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
