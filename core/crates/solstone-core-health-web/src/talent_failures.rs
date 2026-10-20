// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

/// Today's talent failures, "today" being the journal's owner-zone day.
pub fn today(root: &std::path::Path) -> (Vec<Value>, bool) {
    today_at(root, Utc::now())
}

fn today_at(root: &std::path::Path, now: chrono::DateTime<Utc>) -> (Vec<Value>, bool) {
    let zone = solstone_core_journal_config::owner_zone(root);
    let wanted = now.with_timezone(&zone).format("%Y%m%d").to_string();
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
                    if let Some(value) = execution_row(line, &wanted, zone) {
                        all.push(value);
                    }
                }
            }
            Err(_) => ok = false,
        }
    }
    (failures(all), ok)
}

// Historical log files can hold executions from today. Read their timestamps,
// then allocate the full object only for rows this response will actually use.
struct ExecutionTimestamp(Option<Value>);

impl<'de> Deserialize<'de> for ExecutionTimestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TimestampVisitor;
        impl<'de> Visitor<'de> for TimestampVisitor {
            type Value = ExecutionTimestamp;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a talent log object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut timestamp = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "ts" {
                        // serde_json::Value keeps the last duplicate key.
                        timestamp = Some(map.next_value::<Value>()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(ExecutionTimestamp(timestamp))
            }
        }
        deserializer.deserialize_map(TimestampVisitor)
    }
}

fn execution_row(line: &str, wanted: &str, zone: Tz) -> Option<Value> {
    let timestamp = serde_json::from_str::<ExecutionTimestamp>(line).ok()?.0;
    if execution_day(timestamp.as_ref(), zone).as_deref() != Some(wanted) {
        return None;
    }
    // Preserve full-Value validation, including malformed non-timestamp fields.
    serde_json::from_str(line).ok()
}

fn failures(all: Vec<Value>) -> Vec<Value> {
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
    errors.into_iter().filter(|(name,ts,_)| successes.get(name).copied().unwrap_or(0)<=*ts).map(|(_,ts,obj)| json!({"type":"agent","id":use_id(obj.get("use_id")),"name":obj.get("name"),"ts":ts,"service":"cortex","error":"thinking error","reason_code":string_or_null(obj.get("reason_code")),"provider":string_or_null(obj.get("provider")),"model":string_or_null(obj.get("model"))})).collect()
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
fn execution_day(value: Option<&Value>, zone: Tz) -> Option<String> {
    let ts = timestamp(value)?;
    if ts <= 0 {
        return None;
    };
    zone.timestamp_millis_opt(ts)
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
    use super::{execution_day, execution_row, timestamp, use_id};
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};

    #[test]
    fn timestamp_and_use_id_match_python_scalar_coercions() {
        assert_eq!(timestamp(Some(&json!(12.9))), Some(12));
        assert_eq!(timestamp(Some(&json!(-12.9))), Some(-12));
        assert_eq!(timestamp(Some(&json!(" 12 "))), Some(12));
        assert_eq!(use_id(Some(&json!(42))), "42");
        assert_eq!(use_id(Some(&json!(true))), "True");
        assert_eq!(use_id(Some(&json!(0))), "");
    }

    #[test]
    fn timestamp_selection_matches_full_value_parsing() {
        let now = Utc.with_ymd_and_hms(2026, 10, 20, 0, 30, 0).unwrap();
        let today = now.timestamp_millis();
        let yesterday = today - 86_400_000;
        let lines = [
            format!(r#"{{"ts":{today},"name":"kept","status":"error"}}"#),
            format!(r#"{{"ts":{yesterday},"ts":{today}}}"#),
            format!(r#"{{"ts":{today},"ts":{yesterday}}}"#),
            format!(r#"{{"ts":{today},"ts":null}}"#),
            format!(r#"{{"ts":{today},"ts":"invalid"}}"#),
            format!(r#"{{"nested":{{"ts":{today}}},"ts":{yesterday}}}"#),
            format!(r#"{{"ts":" {today} "}}"#),
            format!(r#"{{"ts":{today}.9}}"#),
            format!(r#"{{"ts":{today},"other":1e999}}"#),
            format!(r#"{{"ts":{today}}} trailing"#),
            format!(r#"{{"ts":{today},"unfinished":["#),
            r#"{"ts":true}"#.into(),
            r#"{"ts":0}"#.into(),
            r#"{"ts":-1}"#.into(),
            r#"{"ts":18446744073709551615}"#.into(),
            r#"{"ts":{"nested":1}}"#.into(),
            r#"{"ts":[1]}"#.into(),
            r#"{"missing":"timestamp"}"#.into(),
            r#"[{"ts":1}]"#.into(),
            r#"null"#.into(),
            r#"123"#.into(),
            String::new(),
        ];
        let mut selected = 0;
        for zone in [chrono_tz::UTC, chrono_tz::America::Denver] {
            for wanted in ["20261019", "20261020"] {
                for line in &lines {
                    let expected = serde_json::from_str::<Value>(line).ok().filter(|row| {
                        execution_day(row.get("ts"), zone).as_deref() == Some(wanted)
                    });
                    let actual = execution_row(line, wanted, zone);
                    selected += usize::from(actual.is_some());
                    assert_eq!(actual, expected, "{zone} {wanted}: {line}");
                }
            }
        }
        assert!(selected > 0);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn current_execution_errors_cross_filename_days_and_keep_read_failures() {
        let root = tempfile::tempdir().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0).unwrap();
        let ts = now.timestamp_millis();
        std::fs::create_dir(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
        let talents = root.path().join("talents");
        std::fs::create_dir(&talents).unwrap();
        let rows = [
            json!({"ts":ts-86_400_000,"name":"old","status":"error"}),
            json!({"ts":ts-2,"name":"recovered","status":"error"}),
            json!({"ts":ts-1,"name":"still-failed","status":"error","use_id":"kept"}),
        ];
        std::fs::write(
            talents.join("20000101.jsonl"),
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        std::fs::write(
            talents.join("20000102.jsonl"),
            format!(
                "{}\n",
                json!({"ts":ts,"name":"recovered","status":"completed"})
            ),
        )
        .unwrap();
        std::fs::write(talents.join("20000103.jsonl"), [0xff]).unwrap();
        let (errors, ok) = super::today_at(root.path(), now);
        assert!(!ok);
        assert_eq!(
            errors,
            vec![json!({
                "type":"agent","id":"kept","name":"still-failed","ts":ts-1,
                "service":"cortex","error":"thinking error","reason_code":null,
                "provider":null,"model":null,
            })]
        );
    }
}
