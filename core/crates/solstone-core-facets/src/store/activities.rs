// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;

use solstone_core_journal_io::{AtomicWriteOptions, path_lexists, read_text, write_text};

use crate::hold_facet_trust_lock;

use super::error::{FacetStoreError, FacetWriteError};
use super::paths::{FacetContentKind, content_file_path};

/// Read activity JSONL or nested activity bytes without interpretation.
pub fn read_activity_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
) -> Result<Option<String>, FacetStoreError> {
    read_content_file(journal_root, facet_dir, relative_path)
}

/// Atomically replace activity JSONL or nested activity bytes without interpretation.
pub fn write_activity_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
    contents: &str,
) -> Result<(), FacetWriteError> {
    write_content_file(journal_root, facet_dir, relative_path, contents)
}

/// Add one semantic activity row, preserving existing rows and their order.
pub fn add_activity(
    journal_root: &Path,
    facet_dir: &str,
    activity: Value,
) -> Result<Value, FacetWriteError> {
    let mut rows = activity_rows(journal_root, facet_dir)?;
    let id = activity
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !rows
        .iter()
        .any(|row| row.get("id").and_then(Value::as_str) == Some(id))
    {
        rows.push(activity.clone());
        write_rows(journal_root, facet_dir, &rows)?;
    }
    Ok(rows
        .into_iter()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or(activity))
}

/// Update an existing semantic activity row.
pub fn update_activity(
    journal_root: &Path,
    facet_dir: &str,
    id: &str,
    updates: &serde_json::Map<String, Value>,
) -> Result<Option<Value>, FacetWriteError> {
    let mut rows = activity_rows(journal_root, facet_dir)?;
    let Some(row) = rows
        .iter_mut()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
    else {
        return Ok(None);
    };
    let object = row.as_object_mut().expect("activity rows are objects");
    let custom = object.get("custom").and_then(Value::as_bool) == Some(true);
    for (key, value) in updates {
        match (key.as_str(), value, custom) {
            ("description" | "instructions", Value::String(value), false) if value.is_empty() => {
                object.remove(key);
            }
            ("priority", Value::String(value), false) if value == "normal" => {
                object.remove(key);
            }
            ("emoji" | "icon", Value::String(value), true) if value.is_empty() => {
                object.remove(key);
            }
            ("name" | "emoji" | "icon", _, false) => {}
            _ => {
                object.insert(key.clone(), value.clone());
            }
        }
    }
    let output = row.clone();
    write_rows(journal_root, facet_dir, &rows)?;
    Ok(Some(output))
}

/// Remove an explicit activity row by id.
pub fn remove_activity(
    journal_root: &Path,
    facet_dir: &str,
    id: &str,
) -> Result<bool, FacetWriteError> {
    let mut rows = activity_rows(journal_root, facet_dir)?;
    let before = rows.len();
    rows.retain(|row| row.get("id").and_then(Value::as_str) != Some(id));
    if before != rows.len() {
        write_rows(journal_root, facet_dir, &rows)?;
    }
    Ok(before != rows.len())
}

fn activity_rows(journal_root: &Path, facet_dir: &str) -> Result<Vec<Value>, FacetWriteError> {
    Ok(
        read_activity_file(journal_root, facet_dir, "activities.jsonl")?
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect(),
    )
}

fn write_rows(journal_root: &Path, facet_dir: &str, rows: &[Value]) -> Result<(), FacetWriteError> {
    let text = rows
        .iter()
        .map(python_json_line)
        .collect::<Vec<_>>()
        .join("\n");
    write_activity_file(
        journal_root,
        facet_dir,
        "activities.jsonl",
        &(if text.is_empty() {
            text
        } else {
            format!("{text}\n")
        }),
    )
}

fn python_json_line(value: &Value) -> String {
    let compact = serde_json::to_string(value).expect("JSON row");
    let mut output = String::with_capacity(compact.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in compact.chars() {
        output.push(character);
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
        } else if matches!(character, ',' | ':') {
            output.push(' ');
        }
    }
    output
}

fn read_content_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
) -> Result<Option<String>, FacetStoreError> {
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::Activities,
        relative_path,
    )?;
    if !path_lexists(&path)? {
        return Ok(None);
    }
    read_text(&path, String::new())
        .map(Some)
        .map_err(Into::into)
}

fn write_content_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
    contents: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::Activities,
        relative_path,
    )?;
    write_text(&path, contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(FacetWriteError::ContentWrite)
}
