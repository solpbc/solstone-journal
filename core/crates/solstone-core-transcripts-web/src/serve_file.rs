// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as RoutePath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use solstone_core_convey_http::media::{
    MediaBody, ParsedRange, media_response, mime_type, parse_range,
};
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
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsServeFile,
        move || {
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
                return media_response(StatusCode::OK, mime, &path, MediaBody::empty(), None);
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
                    match open_interval(&path, start, end - start + 1) {
                        Ok(body) => media_response(
                            StatusCode::PARTIAL_CONTENT,
                            mime,
                            &path,
                            body,
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
                _ => match open_interval(&path, 0, total) {
                    Ok(body) => media_response(StatusCode::OK, mime, &path, body, None),
                    Err(_) => error(
                        "file_not_found",
                        "that file isn't available.",
                        "File not found",
                        StatusCode::NOT_FOUND,
                    ),
                },
            }
        },
    )
    .await
}

fn open_interval(path: &Path, start: usize, length: usize) -> std::io::Result<MediaBody> {
    MediaBody::from_reader(fs::File::open(path)?, start, length)
}

fn error(reason: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    legacy_error_response(reason, message, detail, status)
}
