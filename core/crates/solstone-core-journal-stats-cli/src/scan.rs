// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Filesystem portions of a per-day scan.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use solstone_core_journal_io::{PathOrDay, iter_segments};
use solstone_core_processing_record::{MediaKind, media_kind};

use crate::{
    DayScanRequest, activities::accumulate_activities, error::JournalStatsError,
    fold::fold_segments, model::DayScan, talents::daily_output_counts,
};

pub(crate) fn compute_day<S, H, W>(
    request: &DayScanRequest<'_, S, H, W>,
    day_dir: &Path,
) -> Result<DayScan, JournalStatsError>
where
    S: solstone_core_system_health::SegmentSource,
    H: solstone_core_system_health::HealthLogSource,
{
    let mut scan = DayScan::default();
    let transcript = count_transcripts(day_dir)?;
    scan.stats.transcript_sessions = transcript.sessions;
    scan.stats.transcript_segments = transcript.segments;
    scan.stats.transcript_duration = transcript.duration;

    let percept = count_screens(day_dir)?;
    scan.stats.percept_sessions = percept.sessions;
    scan.stats.percept_frames = percept.frames;
    scan.stats.percept_duration = percept.duration;

    scan.stats.pending_segments = pending_segments(request.journal_root, request.day)?;
    let outputs = daily_output_counts(
        day_dir,
        request.system_talent_root,
        request.apps_root,
        request.talent_overrides,
    )?;
    scan.stats.outputs_processed = outputs.processed;
    scan.stats.outputs_pending = outputs.pending;

    let fold = fold_segments(
        request.segment_source,
        request.health_source,
        request.journal_root,
        request.day,
        request.now,
    );
    scan.stats.transcript_ranges = fold.transcript_ranges;
    scan.stats.percept_ranges = fold.percept_ranges;
    scan.stats.browser_segments = fold.browser_segments;
    scan.stats.segments_pending_think = fold.segments_pending_think;
    scan.stats.segment_fold_failed = fold.segment_fold_failed;

    let activities = accumulate_activities(request.journal_root, request.day)?;
    scan.agent_data = activities.agent_data;
    scan.facet_data = activities.facet_data;
    scan.heatmap_data = activities.heatmap_data;
    scan.stats.day_bytes = day_bytes(day_dir)?;
    Ok(scan)
}

#[derive(Default)]
struct TranscriptCounts {
    sessions: u64,
    segments: u64,
    duration: f64,
}

fn count_transcripts(day_dir: &Path) -> Result<TranscriptCounts, JournalStatsError> {
    let paths: BTreeSet<PathBuf> = two_level_paths(day_dir)?
        .into_iter()
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            name == "audio.jsonl"
                || has_nonempty_prefix(name, "_audio.jsonl")
                || has_nonempty_prefix(name, "_transcript.jsonl")
        })
        .collect();

    let mut counts = TranscriptCounts::default();
    for path in paths {
        counts.sessions += 1;
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let mut timestamps = Vec::new();
        for line in lines.into_iter().skip(1) {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(record) = record.as_object() else {
                continue;
            };
            counts.segments += 1;
            if let Some(timestamp) = record.get("start").and_then(Value::as_str) {
                timestamps.push(parse_timestamp(timestamp));
            }
        }
        if let (Some(minimum), Some(maximum)) = (
            timestamps.iter().copied().reduce(f64::min),
            timestamps.iter().copied().reduce(f64::max),
        ) {
            counts.duration += maximum - minimum;
        }
    }
    Ok(counts)
}

#[derive(Default)]
struct ScreenCounts {
    sessions: u64,
    frames: u64,
    duration: f64,
}

fn count_screens(day_dir: &Path) -> Result<ScreenCounts, JournalStatsError> {
    let mut paths: Vec<PathBuf> = two_level_paths(day_dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == "screen.jsonl" || has_nonempty_prefix(name, "_screen.jsonl")
                })
        })
        .collect();
    paths.sort();

    let mut counts = ScreenCounts::default();
    for path in paths {
        counts.sessions += 1;
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let mut timestamps = Vec::new();
        for line in contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(record) = record.as_object() else {
                continue;
            };
            if record.contains_key("error") {
                continue;
            }
            if !record.contains_key("frame_id") {
                continue;
            }
            counts.frames += 1;
            if let Some(timestamp) = record.get("timestamp").and_then(Value::as_f64) {
                timestamps.push(timestamp);
            }
        }
        if let (Some(minimum), Some(maximum)) = (
            timestamps.iter().copied().reduce(f64::min),
            timestamps.iter().copied().reduce(f64::max),
        ) {
            counts.duration += maximum - minimum;
        }
    }
    Ok(counts)
}

fn pending_segments(journal_root: &Path, day: &str) -> Result<u64, JournalStatsError> {
    let mut pending = BTreeSet::new();
    for segment in iter_segments(journal_root, PathOrDay::Day(day))? {
        for entry in read_dir(&segment.path)? {
            if !entry
                .file_type()
                .map_err(|source| JournalStatsError::io(entry.path(), source))?
                .is_file()
            {
                continue;
            }
            let extension = entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase);
            let is_media = matches!(
                extension.as_deref().and_then(media_kind),
                Some(MediaKind::Audio | MediaKind::Video)
            );
            if is_media && !entry.path().with_extension("jsonl").exists() {
                pending.insert(segment.key.clone());
            }
        }
    }
    Ok(pending.len().try_into().unwrap_or(u64::MAX))
}

fn day_bytes(path: &Path) -> Result<u64, JournalStatsError> {
    let mut bytes = 0;
    for entry in read_dir(path)? {
        let file_type = entry
            .file_type()
            .map_err(|source| JournalStatsError::io(entry.path(), source))?;
        if file_type.is_dir() {
            bytes += day_bytes(&entry.path())?;
        } else if file_type.is_file() {
            bytes += entry
                .metadata()
                .map_err(|source| JournalStatsError::io(entry.path(), source))?
                .len();
        }
    }
    Ok(bytes)
}

fn two_level_paths(day_dir: &Path) -> Result<Vec<PathBuf>, JournalStatsError> {
    let mut paths = Vec::new();
    for first in read_dir(day_dir)? {
        if !first
            .file_type()
            .map_err(|source| JournalStatsError::io(first.path(), source))?
            .is_dir()
        {
            continue;
        }
        for second in read_dir(&first.path())? {
            if !second
                .file_type()
                .map_err(|source| JournalStatsError::io(second.path(), source))?
                .is_dir()
            {
                continue;
            }
            for entry in read_dir(&second.path())? {
                paths.push(entry.path());
            }
        }
    }
    Ok(paths)
}

fn has_nonempty_prefix(name: &str, suffix: &str) -> bool {
    name.strip_suffix(suffix)
        .is_some_and(|prefix| !prefix.is_empty())
}

fn parse_timestamp(timestamp: &str) -> f64 {
    let mut pieces = timestamp.split(':');
    let Some(hours) = pieces.next().and_then(|piece| piece.parse::<f64>().ok()) else {
        return 0.0;
    };
    let Some(minutes) = pieces.next().and_then(|piece| piece.parse::<f64>().ok()) else {
        return 0.0;
    };
    let Some(seconds) = pieces.next().and_then(|piece| piece.parse::<f64>().ok()) else {
        return 0.0;
    };
    if pieces.next().is_some() {
        return 0.0;
    }
    hours * 3_600.0 + minutes * 60.0 + seconds
}

fn read_dir(path: &Path) -> Result<Vec<fs::DirEntry>, JournalStatsError> {
    fs::read_dir(path)
        .map_err(|source| JournalStatsError::io(path, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| JournalStatsError::io(path, source))
}
