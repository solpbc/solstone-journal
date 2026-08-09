// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only speaker keep-separate assertions.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{json, Value};
use solstone_core_journal_io::{append_jsonl, hold_lock, AppendError, LockError, LockOptions};
use thiserror::Error;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepSeparateSource {
    pub source_kind: String,
    pub operation_id: Option<String>,
    pub detection_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepSeparateAssertion {
    pub pair_key: String,
    pub sources: Vec<KeepSeparateSource>,
}

#[derive(Debug, Error)]
pub enum KeepSeparateError {
    #[error("keep-separate store read failed at {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("malformed keep-separate JSONL at {path}:{line}")]
    MalformedJson { path: PathBuf, line: usize },
    #[error("invalid keep-separate row at {path}:{line}: {detail}")]
    InvalidRow {
        path: PathBuf,
        line: usize,
        detail: String,
    },
    #[error("keep-separate lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("keep-separate append failed: {0}")]
    Append(#[from] AppendError),
}

pub fn pair_key(entity_id_a: &str, entity_id_b: &str) -> String {
    let (left, right) = if entity_id_a <= entity_id_b {
        (entity_id_a, entity_id_b)
    } else {
        (entity_id_b, entity_id_a)
    };
    format!("{left}|{right}")
}

pub fn find_assertion(
    journal_root: &Path,
    entity_id_a: &str,
    entity_id_b: &str,
) -> Result<Option<KeepSeparateAssertion>, KeepSeparateError> {
    let wanted = pair_key(entity_id_a, entity_id_b);
    Ok(fold_assertions(journal_root)?.remove(&wanted))
}

pub fn record_keep_separate_assertion(
    journal_root: &Path,
    entity_id_a: &str,
    entity_id_b: &str,
    source_kind: &str,
    operation_id: Option<&str>,
    detection_count: i64,
) -> Result<(), KeepSeparateError> {
    let path = keep_separate_path(journal_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| KeepSeparateError::Read {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let _lock = hold_lock(&path, LockOptions::default())?;
    let (left, right) = if entity_id_a <= entity_id_b {
        (entity_id_a, entity_id_b)
    } else {
        (entity_id_b, entity_id_a)
    };
    let row = json!({
        "schema_version": SCHEMA_VERSION,
        "event_kind": "assert_source",
        "pair_key": pair_key(left, right),
        "entity_id_a": left,
        "entity_id_b": right,
        "source_kind": source_kind,
        "operation_id": operation_id,
        "detection_count": detection_count,
        "ts": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    validate_row(&row).map_err(|detail| KeepSeparateError::InvalidRow {
        path: path.clone(),
        line: 0,
        detail,
    })?;
    append_jsonl(&path, &row)?;
    Ok(())
}

fn fold_assertions(
    journal_root: &Path,
) -> Result<BTreeMap<String, KeepSeparateAssertion>, KeepSeparateError> {
    let path = keep_separate_path(journal_root);
    let rows = load_rows(&path)?;
    let mut sources =
        BTreeMap::<String, BTreeMap<(String, Option<String>), KeepSeparateSource>>::new();
    for row in rows {
        let object = row.as_object().expect("validated object");
        let key = object["pair_key"]
            .as_str()
            .expect("validated key")
            .to_owned();
        let source_kind = object["source_kind"]
            .as_str()
            .expect("validated kind")
            .to_owned();
        let operation_id = object
            .get("operation_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let source_key = (source_kind.clone(), operation_id.clone());
        let by_pair = sources.entry(key).or_default();
        if object["event_kind"].as_str() == Some("source_removed") {
            by_pair.remove(&source_key);
            continue;
        }
        let detection_count = object["detection_count"].as_i64().expect("validated count");
        if by_pair
            .get(&source_key)
            .is_some_and(|current| current.detection_count >= detection_count)
        {
            continue;
        }
        by_pair.insert(
            source_key,
            KeepSeparateSource {
                source_kind,
                operation_id,
                detection_count,
            },
        );
    }
    Ok(sources
        .into_iter()
        .filter_map(|(pair_key, sources)| {
            (!sources.is_empty()).then(|| KeepSeparateAssertion {
                pair_key,
                sources: sources.into_values().collect(),
            })
        })
        .map(|assertion| (assertion.pair_key.clone(), assertion))
        .collect())
}

fn keep_separate_path(journal_root: &Path) -> PathBuf {
    journal_root.join("speakers/keep-separate.jsonl")
}

fn load_rows(path: &Path) -> Result<Vec<Value>, KeepSeparateError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(KeepSeparateError::Read {
                path: path.to_owned(),
                source,
            })
        }
    };
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let line_number = index + 1;
            let row: Value =
                serde_json::from_str(line).map_err(|_| KeepSeparateError::MalformedJson {
                    path: path.to_owned(),
                    line: line_number,
                })?;
            validate_row(&row).map_err(|detail| KeepSeparateError::InvalidRow {
                path: path.to_owned(),
                line: line_number,
                detail,
            })?;
            Ok(row)
        })
        .collect()
}

fn validate_row(row: &Value) -> Result<(), String> {
    let object = row
        .as_object()
        .ok_or_else(|| "row must be an object".to_owned())?;
    if object.get("schema_version").and_then(Value::as_i64) != Some(SCHEMA_VERSION) {
        return Err("invalid schema_version".to_owned());
    }
    let event_kind = required_string(object, "event_kind")?;
    if event_kind != "assert_source" && event_kind != "source_removed" {
        return Err("unknown event_kind".to_owned());
    }
    let pair = required_string(object, "pair_key")?;
    let source_kind = required_string(object, "source_kind")?;
    let _ = source_kind;
    let _ = required_string(object, "ts")?;
    if event_kind == "assert_source" {
        let left = required_string(object, "entity_id_a")?;
        let right = required_string(object, "entity_id_b")?;
        if pair != pair_key(left, right) {
            return Err("pair_key does not match entity ids".to_owned());
        }
        if object
            .get("operation_id")
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return Err("operation_id must be string or null".to_owned());
        }
        if object
            .get("detection_count")
            .and_then(Value::as_i64)
            .filter(|count| *count >= 1)
            .is_none()
        {
            return Err("detection_count must be a positive int".to_owned());
        }
    } else if object
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err("source_removed operation_id is required".to_owned());
    }
    Ok(())
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing or invalid {field}"))
}
