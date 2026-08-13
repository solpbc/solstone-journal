// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::response::Response;
use serde_json::{Value, json};

use crate::http::json_response;

pub async fn get(journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let tmux = config
        .get("observe")
        .and_then(|value| value.get("tmux"))
        .and_then(Value::as_object);
    json_response(json!({
        "tmux": {
            "enabled": tmux.and_then(|values| values.get("enabled")).cloned().unwrap_or(Value::Bool(true)),
            "capture_interval": tmux.and_then(|values| values.get("capture_interval")).cloned().unwrap_or(json!(5)),
        },
        "defaults": {"tmux": {"enabled": true, "capture_interval": 5, "capture_interval_min": 1, "capture_interval_max": 60}},
    }))
}
