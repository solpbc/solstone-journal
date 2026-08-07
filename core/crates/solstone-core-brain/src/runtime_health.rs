// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::fixture::local_contract;

// Mirrors SCHEMA_VERSION in solstone/think/providers/runtime_health.py. The
// runtime-health schema version is distinct from brain_state.schema_version.
const RUNTIME_HEALTH_SCHEMA_VERSION: u64 = 1;

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
            record_kind: Some("health".to_owned()),
            path,
            record: Some(synthetic_stopped_record()),
            reason_code: None,
            error: None,
        },
        Err(error) => RuntimeRecordInspection {
            status: "unavailable".to_owned(),
            provider: "local".to_owned(),
            record_kind: Some("health".to_owned()),
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
                record_kind: Some("health".to_owned()),
                path,
                record: Some(record),
                reason_code: None,
                error: None,
            },
            Err(error) => RuntimeRecordInspection {
                status: "corrupt".to_owned(),
                provider: "local".to_owned(),
                record_kind: Some("health".to_owned()),
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
    let schema_version = object
        .get("schema_version")
        .map_or(Ok(RUNTIME_HEALTH_SCHEMA_VERSION), required_u64)
        .map_err(|error| format!("runtime health {error}"))?;
    if schema_version != RUNTIME_HEALTH_SCHEMA_VERSION {
        return Err("unsupported runtime health schema_version for local".to_owned());
    }
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
    let detail = object
        .get("detail")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !detail.is_object() {
        return Err("runtime health detail must be an object".to_owned());
    }
    let process = optional_object(object, "process")?;
    let owner = optional_object(object, "owner")?;
    let reason_code = optional_reason_code(object.get("reason_code"))?;
    let desired_fingerprint = optional_string(object, "desired_fingerprint_sha256")?;
    let incarnation = optional_string(object, "incarnation")?;
    let updated_at = optional_string(object, "updated_at")?;
    let display_deadline_at = optional_string(object, "display_deadline_at")?;
    Ok(Value::Object(Map::from_iter([
        (
            "schema_version".to_owned(),
            Value::from(RUNTIME_HEALTH_SCHEMA_VERSION),
        ),
        ("provider".to_owned(), Value::String("local".to_owned())),
        (
            "revision".to_owned(),
            Value::from(default_nonnegative_u64(object, "revision")?),
        ),
        ("phase".to_owned(), Value::String(phase.to_owned())),
        (
            "reason_code".to_owned(),
            reason_code.map_or(Value::Null, Value::String),
        ),
        ("detail".to_owned(), detail),
        (
            "desired_fingerprint_sha256".to_owned(),
            desired_fingerprint.map_or(Value::Null, Value::String),
        ),
        (
            "incarnation".to_owned(),
            incarnation.map_or(Value::Null, Value::String),
        ),
        (
            "generation".to_owned(),
            Value::from(default_nonnegative_u64(object, "generation")?),
        ),
        (
            "attempt".to_owned(),
            Value::from(default_nonnegative_u64(object, "attempt")?),
        ),
        (
            "process".to_owned(),
            process.map_or(Value::Null, Value::Object),
        ),
        (
            "updated_at".to_owned(),
            updated_at.map_or(Value::Null, Value::String),
        ),
        (
            "display_deadline_at".to_owned(),
            display_deadline_at.map_or(Value::Null, Value::String),
        ),
        ("owner".to_owned(), owner.map_or(Value::Null, Value::Object)),
    ])))
}

fn required_u64(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .ok_or_else(|| "value must be a nonnegative integer".to_owned())
}

fn default_nonnegative_u64(object: &Map<String, Value>, field: &str) -> Result<u64, String> {
    object
        .get(field)
        .map(required_u64)
        .unwrap_or(Ok(0))
        .map_err(|_| format!("{field} must be a nonnegative integer"))
}

fn optional_string(object: &Map<String, Value>, field: &str) -> Result<Option<String>, String> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{field} must be a string or null")),
    }
}

fn optional_object(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<Map<String, Value>>, String> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("runtime health {field} must be object/null")),
    }
}

fn optional_reason_code(value: Option<&Value>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(reason) = value.as_str() else {
        return Err("invalid runtime health reason_code".to_owned());
    };
    if local_contract()
        .brain_state
        .runtime_reason_codes
        .iter()
        .any(|candidate| candidate == reason)
    {
        Ok(Some(reason.to_owned()))
    } else {
        Err("invalid runtime health reason_code".to_owned())
    }
}

fn synthetic_stopped_record() -> Value {
    Value::Object(Map::from_iter([
        (
            "schema_version".to_owned(),
            Value::from(RUNTIME_HEALTH_SCHEMA_VERSION),
        ),
        ("provider".to_owned(), Value::String("local".to_owned())),
        ("revision".to_owned(), Value::from(0)),
        ("phase".to_owned(), Value::String("stopped".to_owned())),
        ("reason_code".to_owned(), Value::Null),
        ("detail".to_owned(), Value::Object(Map::new())),
        ("desired_fingerprint_sha256".to_owned(), Value::Null),
        ("incarnation".to_owned(), Value::Null),
        ("generation".to_owned(), Value::from(0)),
        ("attempt".to_owned(), Value::from(0)),
        ("process".to_owned(), Value::Null),
        ("updated_at".to_owned(), Value::Null),
        ("display_deadline_at".to_owned(), Value::Null),
        ("owner".to_owned(), Value::Null),
    ]))
}
