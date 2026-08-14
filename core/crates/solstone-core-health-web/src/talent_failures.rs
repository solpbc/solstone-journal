// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{Local, TimeZone};
use serde_json::{Value, json};

pub fn today(root: &std::path::Path) -> (Vec<Value>, bool) {
    let wanted = Local::now().format("%Y%m%d").to_string();
    let directory = root.join("talents");
    let Ok(entries) = std::fs::read_dir(directory) else {
        return (Vec::new(), true);
    };
    let mut all = Vec::new();
    let mut ok = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("jsonl")
            || path
                .file_stem()
                .and_then(|x| x.to_str())
                .is_none_or(|x| x.len() != 8)
        {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(lines) => {
                for line in lines.lines() {
                    if let Ok(value) = serde_json::from_str::<Value>(line)
                        && execution_day(&value).as_deref() == Some(&wanted)
                    {
                        all.push(value);
                    }
                }
            }
            Err(_) => ok = false,
        }
    }
    let mut successes: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut errors = Vec::new();
    for value in all {
        let Some(obj) = value.as_object() else {
            continue;
        };
        let Some(name) = obj
            .get("name")
            .and_then(Value::as_str)
            .filter(|x| !x.is_empty())
        else {
            continue;
        };
        let Some(ts) = timestamp(obj.get("ts")) else {
            continue;
        };
        if obj.get("status").and_then(Value::as_str) == Some("completed") {
            successes
                .entry(name.to_owned())
                .and_modify(|old| *old = (*old).max(ts))
                .or_insert(ts);
        } else if obj.get("status").and_then(Value::as_str) == Some("error") {
            errors.push((name.to_owned(), ts, obj.clone()));
        }
    }
    errors.sort_by_key(|(_, ts, _)| *ts);
    (errors.into_iter().filter(|(name,ts,_)| successes.get(name).copied().unwrap_or(0)<=*ts).map(|(_,ts,obj)| json!({"type":"agent","id":obj.get("use_id").and_then(Value::as_str).unwrap_or(""),"name":obj.get("name"),"ts":ts,"service":"cortex","error":"talent error","reason_code":string_or_null(obj.get("reason_code")),"provider":string_or_null(obj.get("provider")),"model":string_or_null(obj.get("model"))})).collect(),ok)
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Bool(_) => None,
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}
fn execution_day(value: &Value) -> Option<String> {
    let ts = timestamp(value.get("ts"))?;
    if ts <= 0 {
        return None;
    };
    Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|v| v.format("%Y%m%d").to_string())
}
fn string_or_null(value: Option<&Value>) -> Value {
    value
        .and_then(Value::as_str)
        .map(|s| Value::String(s.to_owned()))
        .unwrap_or(Value::Null)
}
