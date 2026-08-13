// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{body::Bytes, extract::Query, response::Response};
use chrono::{DateTime, Local, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

use crate::{
    http::{invalid_config_value, json_response, settings_operation_failed},
    request_body::{JsonBody, json_body},
    state::sol_voice_constants,
};

pub async fn get(journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    json_response(response(&journal_root, &config, now_ms()))
}

pub async fn update(journal_root: PathBuf, lock_options: LockOptions, body: Bytes) -> Response {
    let JsonBody::Value(Value::Object(updates)) = json_body(body) else {
        return invalid_config_value("sol_voice update must be an object");
    };
    let allowed = [
        "daily_cap",
        "category_caps",
        "rate_floor_minutes",
        "mute_window",
        "category_self_mute_hours",
        "category_self_mute_clear_markers",
        "default_dedupe_window",
        "system_notifications",
        "debug_show_throttled",
    ];
    if let Some(key) = updates.keys().find(|key| !allowed.contains(&key.as_str())) {
        return invalid_config_value(format!("sol_voice.{key} is not a recognized setting"));
    }
    if !valid_settings(&updates) {
        return invalid_config_value("invalid sol_voice settings");
    }
    match mutate_journal_config(&journal_root, lock_options, |config| {
        let section = config
            .entry("sol_voice".to_owned())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut();
        let Some(section) = section else {
            return JournalConfigMutation {
                changed: false,
                value: false,
            };
        };
        let mut changed = false;
        for (key, value) in default_settings() {
            if !section.contains_key(&key) {
                section.insert(key, value);
                changed = true;
            }
        }
        changed |= merge_object(section, &updates);
        JournalConfigMutation {
            changed,
            value: true,
        }
    }) {
        Ok(_) => {
            let config = solstone_core_journal_config::read_journal_config(&journal_root)
                .ok()
                .and_then(|value| value.config)
                .unwrap_or_default();
            json_response(response(&journal_root, &config, now_ms()))
        }
        Err(_) => settings_operation_failed(),
    }
}

fn default_settings() -> Map<String, Value> {
    serde_json::from_value(json!({
        "daily_cap": 5,
        "category_caps": {"arrival":3,"briefing":3,"commitment":2,"error":2,"notice":2,"pattern":2},
        "rate_floor_minutes": 20,
        "mute_window": {"enabled":false,"start_hour_local":22,"end_hour_local":7},
        "category_self_mute_hours": 24,
        "category_self_mute_clear_markers": {},
        "default_dedupe_window": "24h",
        "system_notifications": {"macos":false,"linux":false},
        "debug_show_throttled": false,
    }))
    .expect("sol voice defaults")
}

fn valid_settings(updates: &Map<String, Value>) -> bool {
    let natural = |key: &str| updates.get(key).is_none_or(Value::is_u64);
    if !natural("daily_cap")
        || !natural("rate_floor_minutes")
        || !natural("category_self_mute_hours")
        || !updates
            .get("debug_show_throttled")
            .is_none_or(Value::is_boolean)
        || !updates
            .get("default_dedupe_window")
            .is_none_or(|value| value.as_str().is_some_and(valid_dedupe_window))
    {
        return false;
    }
    let valid_bool_object = |key: &str, allowed: &[&str]| {
        updates.get(key).is_none_or(|value| {
            value.as_object().is_some_and(|object| {
                object.keys().all(|key| allowed.contains(&key.as_str()))
                    && object.values().all(Value::is_boolean)
            })
        })
    };
    if !valid_bool_object("system_notifications", &["macos", "linux"]) {
        return false;
    }
    if let Some(value) = updates.get("category_caps") {
        let Some(caps) = value.as_object() else {
            return false;
        };
        if caps.keys().any(|key| !categories().contains(key))
            || caps.values().any(|value| !value.is_u64())
        {
            return false;
        }
    }
    if let Some(value) = updates.get("category_self_mute_clear_markers")
        && !value.as_object().is_some_and(|markers| {
            markers.keys().all(|key| categories().contains(key))
                && markers.values().all(Value::is_u64)
        })
    {
        return false;
    }
    if let Some(value) = updates.get("mute_window") {
        let Some(window) = value.as_object() else {
            return false;
        };
        if window
            .keys()
            .any(|key| !["enabled", "start_hour_local", "end_hour_local"].contains(&key.as_str()))
            || window
                .get("enabled")
                .is_some_and(|value| !value.is_boolean())
            || window
                .get("start_hour_local")
                .is_some_and(|value| value.as_u64().is_none_or(|hour| hour > 23))
            || window
                .get("end_hour_local")
                .is_some_and(|value| value.as_u64().is_none_or(|hour| hour > 23))
        {
            return false;
        }
    }
    true
}

fn valid_dedupe_window(value: &str) -> bool {
    let Some(unit) = value.as_bytes().last() else {
        return false;
    };
    if !matches!(unit, b's' | b'm' | b'h' | b'd') || value.len() == 1 {
        return false;
    }
    value[..value.len() - 1]
        .parse::<u64>()
        .is_ok_and(|amount| amount > 0)
}

fn merge_object(target: &mut Map<String, Value>, updates: &Map<String, Value>) -> bool {
    let mut changed = false;
    for (key, value) in updates {
        if let (Some(existing), Some(nested)) = (
            target.get_mut(key).and_then(Value::as_object_mut),
            value.as_object(),
        ) {
            changed |= merge_object(existing, nested);
        } else {
            changed |= target.get(key) != Some(value);
            target.insert(key.clone(), value.clone());
        }
    }
    changed
}

fn response(journal_root: &Path, config: &Map<String, Value>, now: i64) -> Value {
    let configured = config.get("sol_voice").and_then(Value::as_object);
    let categories = categories();
    let category_caps = configured
        .and_then(|settings| settings.get("category_caps"))
        .and_then(Value::as_object);
    let mut caps = Map::new();
    for category in categories {
        caps.insert(
            category.clone(),
            category_caps
                .and_then(|caps| caps.get(&category))
                .filter(|value| value.is_u64())
                .cloned()
                .unwrap_or_else(|| default_cap(&category)),
        );
    }
    let clear_markers = configured
        .and_then(|settings| settings.get("category_self_mute_clear_markers"))
        .and_then(Value::as_object);
    let mute_hours = unsigned(configured, "category_self_mute_hours", 24)
        .as_u64()
        .unwrap_or(24);
    let category_mute_state = category_mute_state(
        journal_root,
        &owner_day(config, now),
        caps.keys(),
        clear_markers,
        mute_hours,
        now,
    );
    json!({
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
        "category_mute_state": category_mute_state,
    })
}

pub async fn throttled(
    journal_root: PathBuf,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let limit = query
        .get("limit")
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let limit = usize::try_from(limit).unwrap_or(200);
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

fn category_mute_state<'a>(
    journal_root: &Path,
    day: &str,
    categories: impl Iterator<Item = &'a String>,
    clear_markers: Option<&Map<String, Value>>,
    mute_hours: u64,
    now: i64,
) -> Map<String, Value> {
    let events = chat_events(journal_root, day);
    let mute_ms = mute_hours.saturating_mul(3_600_000);
    categories
        .map(|category| {
            let latest_dismissed = latest_dismissed(
                &events,
                category,
                clear_markers
                    .and_then(|markers| markers.get(category))
                    .and_then(Value::as_i64)
                    .filter(|marker| *marker >= 0)
                    .unwrap_or(0),
                mute_ms,
                now,
            );
            let value = latest_dismissed.map_or_else(
                || json!({"muted": false, "expires_ts": null}),
                |dismissed| {
                    json!({
                        "muted": true,
                        "expires_ts": dismissed.saturating_add(i64::try_from(mute_ms).unwrap_or(i64::MAX)),
                    })
                },
            );
            (category.clone(), value)
        })
        .collect()
}

fn latest_dismissed(
    events: &[Value],
    category: &str,
    clear_marker: i64,
    mute_ms: u64,
    now: i64,
) -> Option<i64> {
    if mute_ms == 0 {
        return None;
    }
    let mute_ms = i64::try_from(mute_ms).unwrap_or(i64::MAX);
    let mut requests = HashMap::new();
    let mut latest = None;
    for event in events {
        match event.get("kind").and_then(Value::as_str) {
            Some("sol_chat_request") => {
                let request_id = event
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !request_id.is_empty() {
                    requests.insert(
                        request_id,
                        event
                            .get("category")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                }
            }
            Some("owner_chat_dismissed") => {
                let dismissed = event.get("ts").and_then(Value::as_i64).unwrap_or(0);
                if dismissed <= clear_marker || now.saturating_sub(dismissed) > mute_ms {
                    continue;
                }
                let request_id = event
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if requests.get(request_id) == Some(&category) {
                    latest = Some(latest.unwrap_or(0).max(dismissed));
                }
            }
            _ => {}
        }
    }
    latest
}

fn chat_events(journal_root: &Path, day: &str) -> Vec<Value> {
    let mut events = iter_segments(journal_root, PathOrDay::Day(day))
        .unwrap_or_default()
        .into_iter()
        .filter(|segment| segment.stream == "chat")
        .flat_map(|segment| {
            fs::read_to_string(segment.path.join("chat.jsonl"))
                .unwrap_or_default()
                .lines()
                .enumerate()
                .filter_map(|(line, source)| {
                    serde_json::from_str::<Value>(source)
                        .ok()
                        .map(|event| (line, event))
                })
                .map(move |(line, event)| {
                    (event_timestamp(&event), segment.key.clone(), line, event)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    events.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    events.into_iter().map(|(_, _, _, event)| event).collect()
}

fn event_timestamp(event: &Value) -> i64 {
    event.get("ts").and_then(Value::as_i64).unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn owner_day(config: &Map<String, Value>, now: i64) -> String {
    let timestamp = DateTime::<Utc>::from_timestamp_millis(now).unwrap_or_else(Utc::now);
    let timezone = config
        .get("identity")
        .and_then(Value::as_object)
        .and_then(|identity| identity.get("timezone"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<Tz>().ok());
    timezone.map_or_else(
        || timestamp.with_timezone(&Local).format("%Y%m%d").to_string(),
        |timezone| {
            timestamp
                .with_timezone(&timezone)
                .format("%Y%m%d")
                .to_string()
        },
    )
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

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::{body::to_bytes, http::Request};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{now_ms, owner_day};

    #[tokio::test]
    async fn sol_voice_reports_the_current_category_self_mute_state() {
        let root = crate::test_support::established_root();
        let now = now_ms();
        let config = json!({
            "setup": {"completed_at": 1_700_000_000_000_i64},
            "identity": {"timezone": "UTC"},
        });
        fs::write(
            root.path().join("config/journal.json"),
            serde_json::to_vec(&config).expect("config JSON"),
        )
        .expect("config writes");
        let day = owner_day(config.as_object().expect("config object"), now);
        let chat = root
            .path()
            .join("chronicle")
            .join(day)
            .join("chat/090000_300/chat.jsonl");
        fs::create_dir_all(chat.parent().expect("chat parent")).expect("chat directory");
        fs::write(
            chat,
            format!(
                "{{\"kind\":\"sol_chat_request\",\"request_id\":\"request\",\"category\":\"briefing\",\"ts\":{}}}\n{{\"kind\":\"owner_chat_dismissed\",\"request_id\":\"request\",\"ts\":{}}}\n",
                now - 2_000,
                now - 1_000,
            ),
        )
        .expect("chat events");
        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/sol_voice")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["category_mute_state"]["briefing"]["muted"], true);
        assert_eq!(
            body["category_mute_state"]["briefing"]["expires_ts"],
            now - 1_000 + 24 * 3_600_000,
        );
    }

    #[tokio::test]
    async fn throttled_negative_limit_clamps_to_one() {
        let root = crate::test_support::established_root();
        let push = root.path().join("push/nudge_log.jsonl");
        fs::create_dir_all(push.parent().expect("push parent")).expect("push directory");
        fs::write(
            push,
            concat!(
                "{\"kind\":\"sol_chat_request\",\"ts\":1,\"outcome\":\"throttled\"}\n",
                "{\"kind\":\"sol_chat_request\",\"ts\":2,\"outcome\":\"throttled\"}\n",
            ),
        )
        .expect("nudge log");
        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/sol_voice/throttled?limit=-1")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["total"], 1);
        assert_eq!(body["items"][0]["ts"], 2);
    }
}
