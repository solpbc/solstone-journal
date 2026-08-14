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
        json!({
            "stream": segment.stream,
            "segment": segment.key,
            "outputs": records(&segment.path.join("talents")),
        })
    })
    .collect::<Vec<_>>();
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
    let mut records = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_str()?;
            if !matches!(extension, "md" | "json" | "jsonl") || !path.is_file() {
                return None;
            }
            Some(json!({
                "name": path.file_name()?.to_string_lossy(),
                "bytes": path.metadata().ok()?.len(),
            }))
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    records
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
