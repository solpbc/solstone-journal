// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! First copresence-qualifying detected-store day, cached for GET surfaces.

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    JsonWriteOptions, MalformedPolicy, contained_path, day_dirs, read_json, read_text, write_json,
};

use super::detected_entity_activity::detected_days;
use super::map::list_facet_directories;

const CACHE_REL: &str = "facets/.connections-horizon-cache.json";
const CACHE_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionsHorizon {
    pub day: String,
    pub earlier_days: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    path: String,
    len: u64,
    mtime_secs: i64,
    mtime_nanos: i64,
    ino: u64,
}

struct ScanOutcome {
    horizon_day: Option<String>,
    fingerprints: Option<Vec<FileFingerprint>>,
}

pub fn refresh_connections_horizon(journal_root: &Path) -> Option<ConnectionsHorizon> {
    if let Some(cached) = read_hit(journal_root) {
        return publish(journal_root, cached);
    }
    let outcome = scan(journal_root)?;
    if let Some(files) = outcome.fingerprints {
        write_cache(journal_root, outcome.horizon_day.as_deref(), &files);
    }
    publish(journal_root, outcome.horizon_day)
}

fn publish(journal_root: &Path, day: Option<String>) -> Option<ConnectionsHorizon> {
    let day = day?;
    let earlier_days = count_earlier_days(journal_root, &day);
    (earlier_days >= 1).then_some(ConnectionsHorizon { day, earlier_days })
}

fn count_earlier_days(journal_root: &Path, day: &str) -> usize {
    match day_dirs(journal_root) {
        Ok(dirs) => dirs
            .keys()
            .filter(|candidate| candidate.as_str() < day)
            .count(),
        Err(_) => 0,
    }
}

fn scan(journal_root: &Path) -> Option<ScanOutcome> {
    let listed = listed_store_days(journal_root)?;
    let mut days: Vec<String> = listed.iter().map(|(_, day)| day.clone()).collect();
    days.sort();
    days.dedup();

    let mut horizon_day = None;
    for day in &days {
        let mut unreadable = false;
        let qualifies = listed
            .iter()
            .filter(|(_, listed_day)| listed_day == day)
            .any(
                |(facet, _)| match day_has_qualifying_row(journal_root, facet, day) {
                    Some(true) => true,
                    Some(false) => false,
                    None => {
                        unreadable = true;
                        false
                    }
                },
            );
        if unreadable {
            return None;
        }
        if qualifies {
            horizon_day = Some(day.clone());
            break;
        }
    }

    let fingerprints = fingerprint_set(journal_root, &listed, horizon_day.as_deref());
    Some(ScanOutcome {
        horizon_day,
        fingerprints,
    })
}

fn listed_store_days(journal_root: &Path) -> Option<Vec<(String, String)>> {
    let facets = list_facet_directories(journal_root).ok()?;
    let mut listed = Vec::new();
    for facet in facets {
        let days = detected_days(journal_root, &facet).ok()?;
        for day in days {
            listed.push((facet.clone(), day));
        }
    }
    Some(listed)
}

fn store_rel(facet: &str, day: &str) -> String {
    format!("facets/{facet}/entities/{day}.jsonl")
}

fn is_day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn row_qualifies(entry: &Map<String, Value>) -> bool {
    let Some(Value::String(name)) = entry.get("name") else {
        return false;
    };
    if name.trim().is_empty() {
        return false;
    }
    let Some(Value::Array(segments)) = entry.get("segments") else {
        return false;
    };
    segments
        .iter()
        .any(|segment| matches!(segment, Value::String(value) if !value.trim().is_empty()))
}

fn file_has_qualifying_row(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(Value::Object(record)) => row_qualifies(&record),
            Ok(_) | Err(_) => false,
        }
    })
}

fn day_has_qualifying_row(journal_root: &Path, facet: &str, day: &str) -> Option<bool> {
    let path = contained_path(journal_root, &store_rel(facet, day)).ok()?;
    let text = read_text(&path, String::new()).ok()?;
    Some(file_has_qualifying_row(&text))
}

fn fingerprint_path(journal_root: &Path, rel: &str) -> Option<FileFingerprint> {
    let path = contained_path(journal_root, rel).ok()?;
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileFingerprint {
            path: rel.to_owned(),
            len: metadata.len(),
            mtime_secs: metadata.mtime(),
            mtime_nanos: metadata.mtime_nsec(),
            ino: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

fn fingerprint_set(
    journal_root: &Path,
    listed: &[(String, String)],
    horizon_day: Option<&str>,
) -> Option<Vec<FileFingerprint>> {
    let mut files = Vec::new();
    for (facet, day) in listed {
        if horizon_day.is_some_and(|bound| day.as_str() > bound) {
            continue;
        }
        files.push(fingerprint_path(journal_root, &store_rel(facet, day))?);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Some(files)
}

fn read_hit(journal_root: &Path) -> Option<Option<String>> {
    let path = contained_path(journal_root, CACHE_REL).ok()?;
    let value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise).ok()?;
    let object = value.as_object()?;
    if object.get("schema_version").and_then(Value::as_u64) != Some(CACHE_SCHEMA_VERSION) {
        return None;
    }
    let horizon_day = match object.get("horizon_day") {
        Some(Value::Null) => None,
        Some(Value::String(day)) if is_day_key(day) => Some(day.clone()),
        _ => return None,
    };
    let cached_files = parse_files(object.get("files")?)?;
    let listed = listed_store_days(journal_root)?;
    let current = fingerprint_set(journal_root, &listed, horizon_day.as_deref())?;
    (current == cached_files).then_some(horizon_day)
}

fn parse_files(value: &Value) -> Option<Vec<FileFingerprint>> {
    let mut files = Vec::new();
    for entry in value.as_array()? {
        let object = entry.as_object()?;
        files.push(FileFingerprint {
            path: object.get("path")?.as_str()?.to_owned(),
            len: object.get("len")?.as_u64()?,
            mtime_secs: object.get("mtime_secs")?.as_i64()?,
            mtime_nanos: object.get("mtime_nanos")?.as_i64()?,
            ino: object.get("ino")?.as_u64()?,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Some(files)
}

fn write_cache(journal_root: &Path, horizon_day: Option<&str>, files: &[FileFingerprint]) {
    let Ok(path) = contained_path(journal_root, CACHE_REL) else {
        return;
    };
    if !cache_parent_is_dir(&path) {
        return;
    }
    let files_json = files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "len": file.len,
                "mtime_secs": file.mtime_secs,
                "mtime_nanos": file.mtime_nanos,
                "ino": file.ino,
            })
        })
        .collect::<Vec<_>>();
    let value = json!({
        "schema_version": CACHE_SCHEMA_VERSION,
        "horizon_day": horizon_day,
        "files": files_json,
    });
    let _ = write_json(
        &path,
        &value,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: true,
        },
    );
}

fn cache_parent_is_dir(path: &Path) -> bool {
    path.parent().is_some_and(Path::is_dir)
}
