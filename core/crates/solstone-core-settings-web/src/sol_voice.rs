// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::HashMap, fs, path::PathBuf};

use axum::{extract::Query, response::Response};
use serde_json::{Map, Value, json};

use crate::{http::json_response, state::sol_voice_constants};

pub async fn get(journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let configured = config.get("sol_voice").and_then(Value::as_object);
    let categories = categories();
    let category_caps = configured
        .and_then(|settings| settings.get("category_caps"))
        .and_then(Value::as_object);
    let mut caps = Map::new();
    let mut mute_state = Map::new();
    for category in categories {
        caps.insert(
            category.clone(),
            category_caps
                .and_then(|caps| caps.get(&category))
                .filter(|value| value.is_u64())
                .cloned()
                .unwrap_or_else(|| default_cap(&category)),
        );
        // A missing event log is an unmuted category. The normalizer intentionally
        // erases the host/day-dependent value while retaining this complete key set.
        mute_state.insert(category, Value::Null);
    }
    json_response(json!({
        "daily_cap": unsigned(configured, "daily_cap", 5),
        "category_caps": caps,
        "rate_floor_minutes": unsigned(configured, "rate_floor_minutes", 20),
        "mute_window": {
            "enabled": configured.and_then(|settings| settings.get("mute_window")).and_then(Value::as_object).and_then(|window| window.get("enabled")).and_then(Value::as_bool).unwrap_or(false),
            "start_hour_local": unsigned(configured.and_then(|settings| settings.get("mute_window")).and_then(Value::as_object), "start_hour_local", 22),
            "end_hour_local": unsigned(configured.and_then(|settings| settings.get("mute_window")).and_then(Value::as_object), "end_hour_local", 7),
        },
        "category_self_mute_hours": unsigned(configured, "category_self_mute_hours", 24),
        "category_self_mute_clear_markers": configured.and_then(|settings| settings.get("category_self_mute_clear_markers")).filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({})),
        "default_dedupe_window": configured.and_then(|settings| settings.get("default_dedupe_window")).and_then(Value::as_str).unwrap_or("24h"),
        "system_notifications": {
            "macos": configured.and_then(|settings| settings.get("system_notifications")).and_then(Value::as_object).and_then(|settings| settings.get("macos")).and_then(Value::as_bool).unwrap_or(false),
            "linux": configured.and_then(|settings| settings.get("system_notifications")).and_then(Value::as_object).and_then(|settings| settings.get("linux")).and_then(Value::as_bool).unwrap_or(false),
        },
        "debug_show_throttled": configured.and_then(|settings| settings.get("debug_show_throttled")).and_then(Value::as_bool).unwrap_or(false),
        "category_mute_state": mute_state,
    }))
}

pub async fn throttled(
    journal_root: PathBuf,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let limit = query
        .get("limit")
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let rows = fs::read_to_string(journal_root.join("push/nudge_log.jsonl"))
        .ok()
        .map(|source| {
            source
                .lines()
                .rev()
                .take(limit.saturating_mul(4))
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|row| row.get("kind").and_then(Value::as_str) == Some("sol_chat_request"))
                .filter(|row| row.get("outcome").and_then(Value::as_str) != Some("written"))
                .take(limit)
                .map(|row| json!({"ts": row.get("ts"), "category": row.get("category"), "dedupe_key": row.get("dedupe_key"), "outcome": row.get("outcome")}))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json_response(json!({"items": rows, "total": rows.len()}))
}

fn categories() -> Vec<String> {
    sol_voice_constants()
        .remove("CATEGORIES")
        .and_then(|value| value.as_array().cloned())
        .expect("sol voice CATEGORIES list")
        .into_iter()
        .map(|value| value.as_str().expect("category string").to_owned())
        .collect()
}

fn default_cap(category: &str) -> Value {
    sol_voice_constants()
        .remove("CATEGORY_CAP_DEFAULTS")
        .and_then(|value| value.as_object().cloned())
        .and_then(|caps| caps.get(category).cloned())
        .expect("category cap default")
}

fn unsigned(settings: Option<&Map<String, Value>>, key: &str, default: u64) -> Value {
    settings
        .and_then(|settings| settings.get(key))
        .filter(|value| value.is_u64())
        .cloned()
        .unwrap_or_else(|| json!(default))
}
