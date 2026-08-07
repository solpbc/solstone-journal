// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::fixture::local_contract;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeRecordInspection {
    pub status: String,
    pub provider: String,
    pub record_kind: Option<String>,
    pub path: PathBuf,
    pub record: Option<Value>,
    pub reason_code: Option<String>,
    pub error: Option<String>,
}

pub fn inspect_runtime_health(journal_path: &Path) -> RuntimeRecordInspection {
    let path = journal_path.join("health/providers/runtime/local.json");
    match fs::read(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RuntimeRecordInspection {
            status: "ok".to_owned(),
            provider: "local".to_owned(),
            record_kind: Some("synthetic".to_owned()),
            path,
            record: Some(synthetic_stopped_record()),
            reason_code: None,
            error: None,
        },
        Err(error) => RuntimeRecordInspection {
            status: "unavailable".to_owned(),
            provider: "local".to_owned(),
            record_kind: None,
            path,
            record: None,
            reason_code: Some("record-unavailable".to_owned()),
            error: Some(error.to_string()),
        },
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| error.to_string())
            .and_then(coerce_record)
        {
            Ok(record) => RuntimeRecordInspection {
                status: "ok".to_owned(),
                provider: "local".to_owned(),
                record_kind: Some("persisted".to_owned()),
                path,
                record: Some(record),
                reason_code: None,
                error: None,
            },
            Err(error) => RuntimeRecordInspection {
                status: "corrupt".to_owned(),
                provider: "local".to_owned(),
                record_kind: None,
                path,
                record: None,
                reason_code: Some("record-malformed".to_owned()),
                error: Some(error),
            },
        },
    }
}

#[cfg(test)]
pub(crate) fn inspection_from_fixture(value: &Value) -> RuntimeRecordInspection {
    let object = value.as_object();
    RuntimeRecordInspection {
        status: object
            .and_then(|object| object.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("ok")
            .to_owned(),
        provider: "local".to_owned(),
        record_kind: object
            .and_then(|object| object.get("record_kind"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        path: PathBuf::from("fixture"),
        record: object.and_then(|object| object.get("record")).cloned(),
        reason_code: object
            .and_then(|object| object.get("reason_code"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        error: object
            .and_then(|object| object.get("error"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn coerce_record(value: Value) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "runtime health record must be an object".to_owned())?;
    let phase = object
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| "runtime health phase must be a string".to_owned())?;
    if !local_contract()
        .brain_state
        .runtime_phases
        .iter()
        .any(|candidate| candidate == phase)
    {
        return Err("runtime health phase is unknown".to_owned());
    }
    for field in ["reason_code", "desired_fingerprint"] {
        if let Some(value) = object.get(field)
            && !value.is_null()
            && !value.is_string()
        {
            return Err(format!("runtime health {field} must be a string or null"));
        }
    }
    let mut record = object.clone();
    record.insert("provider".to_owned(), Value::String("local".to_owned()));
    record
        .entry("schema_version".to_owned())
        .or_insert(Value::from(1));
    record
        .entry("revision".to_owned())
        .or_insert(Value::from(0));
    record
        .entry("detail".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    record
        .entry("generation".to_owned())
        .or_insert(Value::from(0));
    record.entry("attempt".to_owned()).or_insert(Value::from(0));
    if !record.get("detail").is_some_and(Value::is_object) {
        return Err("runtime health detail must be an object".to_owned());
    }
    Ok(Value::Object(record))
}

fn synthetic_stopped_record() -> Value {
    Value::Object(Map::from_iter([
        ("schema_version".to_owned(), Value::from(1)),
        ("provider".to_owned(), Value::String("local".to_owned())),
        ("revision".to_owned(), Value::from(0)),
        ("phase".to_owned(), Value::String("stopped".to_owned())),
        ("reason_code".to_owned(), Value::Null),
        ("detail".to_owned(), Value::Object(Map::new())),
        ("desired_fingerprint".to_owned(), Value::Null),
        ("incarnation".to_owned(), Value::Null),
        ("generation".to_owned(), Value::from(0)),
        ("attempt".to_owned(), Value::from(0)),
        ("process".to_owned(), Value::Null),
        ("updated_at".to_owned(), Value::Null),
        ("display_deadline_at".to_owned(), Value::Null),
        ("owner".to_owned(), Value::Null),
    ]))
}
