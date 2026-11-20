// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{Json, extract::Query, http::StatusCode, response::IntoResponse};
use serde_json::json;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::window_catalog::text;

pub async fn get(
    root: std::path::PathBuf,
    Query(query): Query<std::collections::HashMap<String, String>>,
    locale: &'static str,
) -> axum::response::Response {
    let Some(path) = query.get("path").filter(|s| !s.is_empty()) else {
        return error_envelope(
            "missing_required_field",
            text(
                locale,
                "journal.window.health.logs.a.required.field.is.missing",
            ),
            text(locale, "journal.window.health.logs.missing.path.parameter"),
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    };
    if !valid(path) {
        return error_envelope(
            "invalid_path",
            text(
                locale,
                "journal.window.health.logs.that.path.couldn.t.be.used",
            ),
            text(locale, "journal.window.health.logs.invalid.path"),
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    let path = path.to_owned();
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::HealthLog,
        move || {
            let root = match root.canonicalize() {
                Ok(root) => root,
                Err(_) => root,
            };
            let file = root.join(&path);
            let Ok(file) = file.canonicalize() else {
                return error_envelope(
                    "file_not_found",
                    text(
                        locale,
                        "journal.window.health.logs.that.file.isn.t.available",
                    ),
                    text(locale, "journal.window.health.logs.log.file.not.found"),
                    StatusCode::NOT_FOUND,
                )
                .into_response();
            };
            if !file.starts_with(&root) {
                return error_envelope(
                    "invalid_path",
                    text(
                        locale,
                        "journal.window.health.logs.that.path.couldn.t.be.used",
                    ),
                    text(locale, "journal.window.health.logs.invalid.path"),
                    StatusCode::BAD_REQUEST,
                )
                .into_response();
            }
            match std::fs::read_to_string(file) {
                Ok(content) => Json(json!({"content":content,"path":path})).into_response(),
                Err(_) => error_envelope(
                    "file_read_failed",
                    text(
                        locale,
                        "journal.window.health.logs.that.file.couldn.t.be.read",
                    ),
                    text(locale, "journal.window.health.logs.failed.to.read.log.file"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
                .into_response(),
            }
        },
    )
    .await
}
fn valid(path: &str) -> bool {
    let mut parts = path.split('/');
    matches!((parts.next(),parts.next(),parts.next(),parts.next(),parts.next()),(Some("chronicle"),Some(day),Some("health"),Some(file),None) if day.len()==8 && day.bytes().all(|x|x.is_ascii_digit()) && file.ends_with(".log") && !file.contains('/'))
}
