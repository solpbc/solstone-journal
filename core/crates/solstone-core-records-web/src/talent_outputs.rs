// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

pub fn list(journal_root: &Path, day: &str, segment: Option<&str>) -> Result<Value, String> {
    validate_day(day)?;
    let day_dir = journal_root.join("chronicle").join(day);
    if let Some(segment) = segment {
        validate_segment(segment)?;
        return Ok(json!({
            "day": day,
            "segment": segment,
            "outputs": records(&day_dir.join(segment).join("talents")),
        }));
    }
    let segments = solstone_core_journal_io::iter_segments(
        journal_root,
        solstone_core_journal_io::PathOrDay::Day(day),
    )
    .map_err(|error| error.to_string())?
    .into_iter()
    .map(|segment| {
        let identity = segment.record_identity().ok_or_else(|| {
            format!(
                "segment path is not UTF-8 representable: {}",
                segment.path().display()
            )
        })?;
        Ok(json!({
            "stream": identity.stream,
            "segment": identity.key,
            "outputs": records(&segment.path().join("talents")),
        }))
    })
    .collect::<Result<Vec<_>, String>>()?;
    Ok(json!({
        "day": day,
        "daily": records(&day_dir.join("talents")),
        "segments": segments,
    }))
}

pub fn find(
    journal_root: &Path,
    agent: &str,
    day: &str,
    segment: Option<&str>,
) -> Result<String, FindError> {
    validate_day(day).map_err(FindError::Invalid)?;
    validate_agent(agent).map_err(FindError::Invalid)?;
    let talents = if let Some(segment) = segment {
        validate_segment(segment).map_err(FindError::Invalid)?;
        journal_root
            .join("chronicle")
            .join(day)
            .join(segment)
            .join("talents")
    } else {
        journal_root.join("chronicle").join(day).join("talents")
    };
    for extension in [".md", ".json", ".jsonl"] {
        let candidate = talents.join(format!("{agent}{extension}"));
        if candidate.is_file() {
            return Ok(candidate
                .strip_prefix(journal_root.join("chronicle"))
                .expect("talent output is beneath chronicle")
                .to_string_lossy()
                .replace('\\', "/"));
        }
    }
    Err(FindError::NotFound)
}

#[derive(Debug, PartialEq, Eq)]
pub enum FindError {
    Invalid(String),
    NotFound,
}

fn records(directory: &Path) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    let mut records = Vec::new();
    for entry in entries {
        let entry_name = entry.file_name();
        let path = entry.path();
        if path.is_file() {
            if let Some(record) = record(&path, entry_name.to_string_lossy().into_owned()) {
                records.push(record);
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        let Ok(children) = fs::read_dir(&path) else {
            continue;
        };
        let mut children = children.filter_map(Result::ok).collect::<Vec<_>>();
        children.sort_by_key(|child| child.file_name());
        for child in children {
            let child_path = child.path();
            let name = format!(
                "{}/{}",
                entry_name.to_string_lossy(),
                child.file_name().to_string_lossy()
            );
            if let Some(record) = record(&child_path, name) {
                records.push(record);
            }
        }
    }
    records
}

fn record(path: &Path, name: impl Into<String>) -> Option<Value> {
    let extension = path.extension()?.to_str()?;
    if !path.is_file() || !matches!(extension, "md" | "json" | "jsonl") {
        return None;
    }
    Some(json!({
        "name": name.into(),
        "bytes": path.metadata().ok()?.len(),
    }))
}

fn validate_day(day: &str) -> Result<(), String> {
    if day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err("day must be YYYYMMDD".into())
    }
}

fn validate_segment(segment: &str) -> Result<(), String> {
    let (start, length) = segment
        .split_once('_')
        .ok_or_else(|| "invalid segment key".to_owned())?;
    if start.len() == 6
        && !length.is_empty()
        && start.bytes().all(|byte| byte.is_ascii_digit())
        && length.bytes().all(|byte| byte.is_ascii_digit())
    {
        Ok(())
    } else {
        Err("invalid segment key".into())
    }
}

fn validate_agent(agent: &str) -> Result<(), String> {
    if agent.is_empty()
        || agent.contains(['/', '\\'])
        || std::path::Path::new(agent)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        Err("agent must be a bare talent name".into())
    } else {
        Ok(())
    }
}
