// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::Json;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::json;
use solstone_core_home::HomeContext;
use solstone_core_home::readers::get_capture_health;

pub async fn status(journal_root: PathBuf) -> Response {
    let context = HomeContext::new(journal_root, Utc::now());
    let capture = get_capture_health(&context);
    Json(json!({
        "version": {
            "current": env!("CARGO_PKG_VERSION"),
            "latest": env!("CARGO_PKG_VERSION"),
            "update_available": false,
        },
        "capture": capture,
        "ok": true,
    }))
    .into_response()
}
