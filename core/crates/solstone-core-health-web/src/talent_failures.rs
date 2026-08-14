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
    (errors.into_iter().filter(|(name,ts,_)| successes.get(name).copied().unwrap_or(0)<=*ts).map(|(_,ts,obj)| json!({"type":"agent","id":use_id(obj.get("use_id")),"name":obj.get("name"),"ts":ts,"service":"cortex","error":"talent error","reason_code":string_or_null(obj.get("reason_code")),"provider":string_or_null(obj.get("provider")),"model":string_or_null(obj.get("model"))})).collect(),ok)
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Bool(_) => None,
        Value::Number(number) => number
            .as_i64()
            .or_else(|| {
                number
                    .as_u64()
                    .and_then(|number| i64::try_from(number).ok())
            })
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|number| number.is_finite())
                    .and_then(|number| {
                        (number >= i64::MIN as f64 && number <= i64::MAX as f64)
                            .then_some(number as i64)
                    })
            }),
        Value::String(value) => value.trim().parse().ok(),
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

fn use_id(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) if !value.is_empty() => value.to_owned(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::Number(value)) if value.as_f64().is_some_and(|value| value != 0.0) => {
            value.to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{timestamp, use_id};
    use serde_json::json;

    #[test]
    fn timestamp_and_use_id_match_python_scalar_coercions() {
        assert_eq!(timestamp(Some(&json!(12.9))), Some(12));
        assert_eq!(timestamp(Some(&json!(-12.9))), Some(-12));
        assert_eq!(timestamp(Some(&json!(" 12 "))), Some(12));
        assert_eq!(use_id(Some(&json!(42))), "42");
        assert_eq!(use_id(Some(&json!(true))), "True");
        assert_eq!(use_id(Some(&json!(0))), "");
    }
}
