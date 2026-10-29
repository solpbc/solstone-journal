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
    let mut body = json!({
        "mode": processing.and_then(|value| value.get("mode")).cloned().unwrap_or(json!("realtime")),
        "gate": {"time_window": {
            "enabled": time_window.and_then(|value| value.get("enabled")).cloned().unwrap_or(json!(true)),
            "start": time_window.and_then(|value| value.get("start")).cloned().unwrap_or(json!("02:00")),
            "end": time_window.and_then(|value| value.get("end")).cloned().unwrap_or(json!("06:00")),
        }, "display_powersave": {"enabled": display.and_then(|value| value.get("enabled")).cloned().unwrap_or(json!(false))}},
    });
    if cfg!(target_os = "macos") && window_needs_awake_mac(&config) {
        body["note"] = json!(MAC_WINDOW_NOTE);
    }
    json_response(body)
}

/// A Mac that sleeps through the window only wakes in short, fan-limited
/// spells, and the journal holds model work during those.
const MAC_WINDOW_NOTE: &str = "your journal only processes during this window while your mac is awake. to keep it awake, on a desktop mac open System Settings, choose Energy, and turn on \"Prevent automatic sleeping when the display is off\". on a laptop, choose Battery, then Options, and turn on \"Prevent automatic sleeping on power adapter when the display is off\", which works while it's plugged in with the lid open.";

fn window_needs_awake_mac(config: &serde_json::Map<String, Value>) -> bool {
    solstone_core_journal_config::processing::processing_settings(config)
        .is_ok_and(|settings| settings.deferred && settings.gate.time_window.enabled)
}
