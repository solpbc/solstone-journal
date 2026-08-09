// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use axum::Json;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use chrono::Local;
use serde_json::{Value, json};

use crate::asset_response;
use crate::assets;

fn speaker_copy() -> &'static Value {
    static COPY: OnceLock<Value> = OnceLock::new();
    COPY.get_or_init(|| {
        serde_json::from_str(assets::speaker_copy_json()).expect("generated speaker copy parses")
    })
}

pub async fn shell() -> Response {
    asset_response("/static/shell.html")
}

pub async fn shell_for_day(Path(day): Path<String>) -> Response {
    if day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()) {
        return shell().await;
    }
    crate::not_found_response()
}

pub async fn workspace() -> Response {
    asset_response("/app/speakers/workspace")
}

pub async fn who_is_this() -> Response {
    asset_response("/app/speakers/static/who_is_this.js")
}

pub async fn state() -> Response {
    Json(json!({
        "today": Local::now().format("%Y%m%d").to_string(),
        "owner_min_statements": 30,
        "owner_status_routing_tokens": {"candidate": "candidate", "confirmed": "confirmed"},
        "not_in_new_voices_copy": assets::not_in_new_voices_copy(),
        "speaker_copy": speaker_copy(),
        "speaker_filter_name": Value::Null,
    }))
    .into_response()
}
