// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use chrono::{Local, TimeZone};
use serde_json::{Map, Value, json};

const KIND_OWNER_CHAT_OPEN: &str = "owner_chat_open";
const KIND_OWNER_CHAT_DISMISSED: &str = "owner_chat_dismissed";
const KIND_SOL_CHAT_REQUEST: &str = "sol_chat_request";
const KIND_SOL_CHAT_REQUEST_SUPERSEDED: &str = "sol_chat_request_superseded";

#[derive(Debug)]
pub(crate) enum ReadEventsError {
    Read,
    Malformed,
}

pub(crate) fn read_events(journal_root: &Path, day: &str) -> Result<Vec<Value>, ReadEventsError> {
    let chat_root = journal_root.join("chronicle").join(day).join("chat");
    if !chat_root.is_dir() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(chat_root).map_err(|_| ReadEventsError::Read)?;
    let mut ordered = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let segment = entry.file_name().to_string_lossy().into_owned();
        if !is_segment_key(&segment) || !entry.path().is_dir() {
            continue;
        }
        let path = entry.path().join("chat.jsonl");
        if !path.is_file() {
            continue;
        }
        let source = fs::read_to_string(&path).map_err(|_| ReadEventsError::Read)?;
        for (line_number, line) in source.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(line).map_err(|_| ReadEventsError::Malformed)?;
            let object = value.as_object().ok_or(ReadEventsError::Malformed)?;
            let timestamp = object.get("ts").and_then(Value::as_i64).unwrap_or(0);
            ordered.push((timestamp, segment.clone(), line_number, value));
        }
    }
    ordered.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
    });
    Ok(ordered.into_iter().map(|(_, _, _, event)| event).collect())
}

pub fn day_counts(journal_root: &Path) -> BTreeMap<String, usize> {
    let Ok(days) = solstone_core_journal_io::day_dirs(journal_root) else {
        return BTreeMap::new();
    };
    days.into_keys()
        .map(|day| {
            let count = read_events(journal_root, &day).map_or(0, |events| events.len());
            (day, count)
        })
        .collect()
}

pub fn sol_open_request_id(events: &[Value], requested_day: &str, today: &str) -> Option<String> {
    if requested_day != today {
        return None;
    }
    let visible = events
        .iter()
        .filter(|event| event["kind"] != KIND_OWNER_CHAT_OPEN)
        .cloned()
        .collect::<Vec<_>>();
    latest_unresolved_request(&visible)
        .and_then(|request| request["request_id"].as_str().map(ToOwned::to_owned))
}

pub fn message_origins(events: &[Value]) -> BTreeMap<String, Value> {
    let mut origins = BTreeMap::new();
    let mut origins_by_request_id = HashMap::new();
    let mut pending: Option<Map<String, Value>> = None;

    for (index, event) in events.iter().enumerate() {
        let kind = event["kind"].as_str();
        if kind == Some(KIND_SOL_CHAT_REQUEST) {
            pending = Some(Map::from_iter([
                ("request_id".into(), event["request_id"].clone()),
                ("summary".into(), event["summary"].clone()),
                ("trigger_talent".into(), event["trigger_talent"].clone()),
                ("dedupe".into(), event["dedupe"].clone()),
                ("since_ts".into(), event["since_ts"].clone()),
                ("ts".into(), event["ts"].clone()),
                (
                    "time".into(),
                    Value::String(format_origin_time(&event["ts"])),
                ),
                ("category".into(), event["category"].clone()),
            ]));
            continue;
        }
        if kind == Some("sol_message") && pending.is_some() {
            let origin = pending.take().expect("checked pending");
            let value = Value::Object(origin.clone());
            if let Some(request_id) = origin
                .get("request_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                origins_by_request_id.insert(request_id.to_owned(), index.to_string());
            }
            origins.insert(index.to_string(), value);
            continue;
        }
        if kind == Some(KIND_SOL_CHAT_REQUEST_SUPERSEDED) {
            let request_id = event["request_id"].as_str().unwrap_or_default();
            if pending
                .as_ref()
                .and_then(|origin| origin.get("request_id"))
                .and_then(Value::as_str)
                == Some(request_id)
            {
                pending = None;
            }
            if let Some(index) = origins_by_request_id.get(request_id)
                && let Some(origin) = origins.get_mut(index).and_then(Value::as_object_mut)
            {
                origin.insert("superseded_by_id".into(), event["replaced_by"].clone());
                origin.insert("superseded_at".into(), event["ts"].clone());
                origin.insert(
                    "superseded_time".into(),
                    Value::String(format_origin_time(&event["ts"])),
                );
            }
        }
    }
    origins
}

pub fn reduced_state(events: &[Value]) -> Value {
    let mut latest_sol_message = Value::Null;
    let mut active = BTreeMap::new();
    let mut queued = BTreeMap::new();
    let mut completed = Vec::new();
    let mut errored = Vec::new();
    let mut chat_error = Value::Null;
    let mut queue_depth = 0;

    for event in events {
        match event["kind"].as_str() {
            Some("chat_queue_depth") => queue_depth = event["depth"].as_i64().unwrap_or(0),
            Some("sol_message") => {
                latest_sol_message = json!({
                    "ts": event["ts"], "use_id": event["use_id"], "text": event["text"],
                    "notes": event["notes"], "requested_target": event["requested_target"],
                    "requested_task": event["requested_task"], "offer": event["offer"],
                    "draft": event["draft"], "origin": event["origin"],
                    "sources": event.get("sources").cloned().unwrap_or_else(|| json!([])),
                    "answer_state": event.get("answer_state").cloned().unwrap_or_else(|| json!("answered")),
                });
                chat_error = Value::Null;
            }
            Some("talent_queued") => {
                queued.insert(event["use_id"].as_str().unwrap_or_default().to_owned(), json!({"use_id": event["use_id"], "name": event["name"], "task": event["task"], "queued_at": event["queued_at"]}));
            }
            Some("talent_spawned") => {
                queued.remove(event["use_id"].as_str().unwrap_or_default());
                active.insert(event["use_id"].as_str().unwrap_or_default().to_owned(), json!({"use_id": event["use_id"], "name": event["name"], "task": event["task"], "started_at": event["started_at"], "label": event["name"]}));
            }
            Some("talent_finished") => {
                queued.remove(event["use_id"].as_str().unwrap_or_default());
                let started = active.remove(event["use_id"].as_str().unwrap_or_default());
                completed.push(json!({"use_id": event["use_id"], "name": event["name"], "task": started.as_ref().map(|value| value["task"].clone()).unwrap_or(Value::Null), "summary": event["summary"], "finished_at": event["ts"], "label": event["name"]}));
            }
            Some("talent_errored") => {
                queued.remove(event["use_id"].as_str().unwrap_or_default());
                active.remove(event["use_id"].as_str().unwrap_or_default());
                errored.push(json!({"use_id": event["use_id"], "name": event["name"], "finished_at": event["ts"], "label": event["name"]}));
            }
            Some("chat_error") => {
                chat_error = json!({"reason": event["reason"], "provider": event.get("provider").cloned().unwrap_or_else(|| json!("")), "detail": event.get("detail").cloned().unwrap_or_else(|| json!(""))})
            }
            _ => {}
        }
    }
    json!({
        "latest_sol_message": latest_sol_message,
        "active_talents": active.into_values().collect::<Vec<_>>(),
        "queued_talents": queued.into_values().collect::<Vec<_>>(),
        "completed_talents": completed,
        "errored_talents": errored,
        "chat_error": chat_error,
        "queue_depth": queue_depth,
    })
}

fn latest_unresolved_request(events: &[Value]) -> Option<Value> {
    let mut resolved = HashSet::new();
    let mut requests = Vec::new();
    for event in events {
        match event["kind"].as_str() {
            Some(
                KIND_OWNER_CHAT_OPEN | KIND_OWNER_CHAT_DISMISSED | KIND_SOL_CHAT_REQUEST_SUPERSEDED,
            ) => {
                if let Some(request_id) = event["request_id"]
                    .as_str()
                    .filter(|id| !id.trim().is_empty())
                {
                    resolved.insert(request_id.to_owned());
                }
            }
            Some(KIND_SOL_CHAT_REQUEST)
                if event["request_id"]
                    .as_str()
                    .is_some_and(|id| !id.trim().is_empty()) =>
            {
                requests.push(event.clone())
            }
            _ => {}
        }
    }
    requests
        .into_iter()
        .rev()
        .find(|request| !resolved.contains(request["request_id"].as_str().unwrap_or_default()))
}

fn format_origin_time(raw: &Value) -> String {
    let Some(timestamp) = raw.as_i64().filter(|timestamp| *timestamp > 0) else {
        return String::new();
    };
    Local
        .timestamp_millis_opt(timestamp)
        .single()
        .map_or_else(String::new, |time| time.format("%-I:%M %p").to_string())
}

fn is_segment_key(value: &str) -> bool {
    let Some((start, length)) = value.split_once('_') else {
        return false;
    };
    start.len() == 6
        && !length.is_empty()
        && start.bytes().all(|byte| byte.is_ascii_digit())
        && length.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{message_origins, sol_open_request_id};

    #[test]
    fn request_origins_keep_list_indices_and_back_patch_supersession() {
        let events = vec![
            json!({"kind":"sol_chat_request","request_id":"one","summary":"s","trigger_talent":"t","dedupe":"d","since_ts":1,"ts":2,"category":"notice"}),
            json!({"kind":"sol_message"}),
            json!({"kind":"sol_chat_request_superseded","request_id":"one","replaced_by":"two","ts":3}),
        ];
        assert_eq!(message_origins(&events)["1"]["superseded_by_id"], "two");
        assert_eq!(sol_open_request_id(&events, "20260731", "20260730"), None);
    }
}
