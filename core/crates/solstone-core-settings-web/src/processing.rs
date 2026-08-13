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
    let processing = config.get("processing").and_then(Value::as_object);
    let gate = processing
        .and_then(|value| value.get("gate"))
        .and_then(Value::as_object);
    let time_window = gate
        .and_then(|value| value.get("time_window"))
        .and_then(Value::as_object);
    let display = gate
        .and_then(|value| value.get("display_powersave"))
        .and_then(Value::as_object);
    json_response(json!({
        "mode": processing.and_then(|value| value.get("mode")).cloned().unwrap_or(json!("realtime")),
        "gate": {"time_window": {
            "enabled": time_window.and_then(|value| value.get("enabled")).cloned().unwrap_or(json!(true)),
            "start": time_window.and_then(|value| value.get("start")).cloned().unwrap_or(json!("02:00")),
            "end": time_window.and_then(|value| value.get("end")).cloned().unwrap_or(json!("06:00")),
        }, "display_powersave": {"enabled": display.and_then(|value| value.get("enabled")).cloned().unwrap_or(json!(false))}},
    }))
}
