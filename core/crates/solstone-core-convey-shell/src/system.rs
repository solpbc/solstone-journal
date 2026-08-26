// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::Json;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn status() -> Response {
    Json(json!({
        "version": {
            "current": "1.0.22",
            "latest": "1.0.22",
            "update_available": false,
        },
        "capture": {"clients": [], "status": "no_clients"},
        "ok": true,
    }))
    .into_response()
}
