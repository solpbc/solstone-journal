// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{Json, extract::Query, http::StatusCode, response::IntoResponse};
use serde_json::json;
use solstone_core_convey_http::envelope::error_envelope;

pub async fn get(
    root: std::path::PathBuf,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let Some(path) = query.get("path").filter(|s| !s.is_empty()) else {
        return error_envelope(
            "missing_required_field",
            "I couldn't find a required field.",
            "Missing path parameter",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    };
    if !valid(path) {
        return error_envelope(
            "invalid_path",
            "I couldn't use that path.",
            "Invalid path",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(_) => root,
    };
    let file = root.join(path);
    let Ok(file) = file.canonicalize() else {
        return error_envelope(
            "file_not_found",
            "I couldn't find that file.",
            "Log file not found",
            StatusCode::NOT_FOUND,
        )
        .into_response();
    };
    if !file.starts_with(&root) {
        return error_envelope(
            "invalid_path",
            "I couldn't use that path.",
            "Invalid path",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    match std::fs::read_to_string(file) {
        Ok(content) => Json(json!({"content":content,"path":path})).into_response(),
        Err(_) => error_envelope(
            "file_read_failed",
            "I couldn't read that file.",
            "Failed to read log file",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}
fn valid(path: &str) -> bool {
    let mut parts = path.split('/');
    matches!((parts.next(),parts.next(),parts.next(),parts.next(),parts.next()),(Some("chronicle"),Some(day),Some("health"),Some(file),None) if day.len()==8 && day.bytes().all(|x|x.is_ascii_digit()) && file.ends_with(".log") && !file.contains('/'))
}
