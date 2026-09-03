// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use solstone_core_processing_record::{MediaKind, media_kind};

use crate::{DayScan, JournalStatsError, SCHEMA_VERSION};

const DAY_FIELDS: &[&str] = &[
    "transcript_sessions",
    "transcript_segments",
    "transcript_duration",
    "transcript_ranges",
    "percept_sessions",
    "percept_frames",
    "percept_duration",
    "percept_ranges",
    "browser_segments",
    "pending_segments",
    "segments_pending_think",
    "outputs_processed",
    "outputs_pending",
    "day_bytes",
];

/// Publishes a complete per-day cache payload.
pub trait DayCacheWriter {
    fn write_day_cache(&self, path: &Path, payload: &DayScan) -> Result<(), JournalStatsError>;
}

/// Production cache writer using journal-io's atomic JSON publication.
#[derive(Debug, Default, Clone, Copy)]
pub struct FilesystemDayCacheWriter;

impl DayCacheWriter for FilesystemDayCacheWriter {
    fn write_day_cache(&self, path: &Path, payload: &DayScan) -> Result<(), JournalStatsError> {
        solstone_core_journal_io::write_json(
            path,
            payload,
            solstone_core_journal_io::JsonWriteOptions::default(),
        )?;
        Ok(())
    }
}

pub(crate) fn save_day_cache<W: DayCacheWriter>(
    writer: &W,
    path: &Path,
    payload: &DayScan,
) -> Result<(), JournalStatsError> {
    writer.write_day_cache(path, payload)
}

/// Return a cache payload only when it is schema-valid and strictly newer than inputs.
pub fn load_fresh_day_cache(day_dir: &Path) -> Result<Option<DayScan>, JournalStatsError> {
    let path = day_dir.join("stats.json");
    if !path.exists() {
        return Ok(None);
    }
    let Ok(cache_mtime) = fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
        return Ok(None);
    };
    let input_mtime = bounded_input_mtime(day_dir)?;
    if input_mtime.is_some_and(|input| cache_mtime <= input) {
        return Ok(None);
    }
    let Ok(text) = solstone_core_journal_io::read_text(&path, String::new()) else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(None);
    };
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(SCHEMA_VERSION))
    {
        return Ok(None);
    }
    let Some(stats) = value.get("stats").and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };
    if DAY_FIELDS.iter().any(|field| !stats.contains_key(*field)) {
        return Ok(None);
    }
    match serde_json::from_value(value) {
        Ok(payload) => Ok(Some(payload)),
        Err(_) => Ok(None),
    }
}

/// Latest mtime among precisely the Python day-cache bounded input set.
pub fn bounded_input_mtime(day_dir: &Path) -> Result<Option<SystemTime>, JournalStatsError> {
    if !day_dir.is_dir() {
        return Ok(None);
    }
    let mut latest = None;
    for path in two_level_entries(day_dir)? {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let matched = name.ends_with("audio.jsonl")
            || name.ends_with("_transcript.jsonl")
            || name.ends_with("_transcript.md")
            || name.ends_with("screen.jsonl");
        if matched && path.is_file() {
            update_latest(&mut latest, &path)?;
        }
    }
    for entry in read_entries(day_dir)? {
        if !entry.is_file() {
            continue;
        }
        let extension = extension_lower(&entry);
        if matches!(extension.as_deref(), Some("flac" | "m4a"))
            || extension
                .as_deref()
                .and_then(media_kind)
                .is_some_and(|kind| kind == MediaKind::Video)
        {
            update_latest(&mut latest, &entry)?;
        }
    }
    let talents = day_dir.join("talents");
    if talents.is_dir() {
        for entry in read_entries(&talents)? {
            if entry.is_file() && is_talent_output(&entry) {
                update_latest(&mut latest, &entry)?;
            }
            if entry.is_dir() {
                for nested in read_entries(&entry)? {
                    if nested.is_file() && is_talent_output(&nested) {
                        update_latest(&mut latest, &nested)?;
                    }
                }
            }
        }
    }
    let health = day_dir.join("health");
    if health.is_dir() {
        for entry in read_entries(&health)? {
            let Some(name) = entry.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if entry.is_file() && (name.ends_with(".jsonl") || name.ends_with(".updated")) {
                update_latest(&mut latest, &entry)?;
            }
        }
    }
    Ok(latest)
}

fn two_level_entries(day_dir: &Path) -> Result<Vec<std::path::PathBuf>, JournalStatsError> {
    let mut result = Vec::new();
    for first in read_entries(day_dir)? {
        if !first.is_dir() {
            continue;
        }
        for second in read_entries(&first)? {
            if !second.is_dir() {
                continue;
            }
            result.extend(read_entries(&second)?);
        }
    }
    Ok(result)
}

fn read_entries(path: &Path) -> Result<Vec<std::path::PathBuf>, JournalStatsError> {
    let entries = fs::read_dir(path).map_err(|error| JournalStatsError::io(path, error))?;
    let mut result = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| JournalStatsError::io(path, error))?;
    result.sort();
    Ok(result)
}

fn update_latest(latest: &mut Option<SystemTime>, path: &Path) -> Result<(), JournalStatsError> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| JournalStatsError::io(path, error))?;
    if latest.is_none_or(|current| modified > current) {
        *latest = Some(modified);
    }
    Ok(())
}

fn extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

fn is_talent_output(path: &Path) -> bool {
    matches!(extension_lower(path).as_deref(), Some("json" | "md"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use chrono::{FixedOffset, TimeZone};
    use solstone_core_journal_io::{
        JournalRoot,
        operational_log::{OplogFormat, create_oplog_at},
    };

    use super::bounded_input_mtime;

    #[test]
    fn canonical_jsonl_oplog_is_a_bounded_day_cache_input() {
        let root = tempfile::tempdir().unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 6, 2, 12, 0, 0)
            .unwrap();
        let mut writer = create_oplog_at(
            JournalRoot::open(root.path()).unwrap(),
            "think",
            "daily",
            OplogFormat::Jsonl,
            opened,
        )
        .unwrap();
        let leaf = writer.leaf_name().to_owned();
        writer.write_all(b"{\"event\":\"run.summary\"}\n").unwrap();
        drop(writer);

        let day = root.path().join("chronicle/20260602");
        let expected = fs::metadata(day.join("health").join(leaf))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(bounded_input_mtime(&day).unwrap(), Some(expected));
    }
}
