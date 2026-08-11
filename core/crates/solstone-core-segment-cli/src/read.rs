// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use chrono::{Duration, NaiveDate, NaiveTime};
use serde_json::{Value, json};
use solstone_core_format::segment::segment_parse;
use solstone_core_segment::{
    DirEntryKind, list_days, list_dir_entries, list_segments, list_segments_in, read_stream_record,
    read_text,
};

use crate::index::{SegmentIndexStatus, read_segment_index};
use crate::location::SegmentLocation;

#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub(crate) name: &'static str,
    pub(crate) passed: bool,
    pub(crate) detail: String,
}

pub(crate) fn split_path(value: &str) -> Result<(&str, &str, &str), &'static str> {
    let mut parts = value.split('/');
    let (Some(day), Some(stream), Some(segment), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("Segment path must be day/stream/segment (e.g. 20260304/default/090000_300)");
    };
    Ok((day, stream, segment))
}

pub(crate) fn read_marker(path: &Path) -> Result<Option<Value>, String> {
    let marker = path.join("stream.json");
    let text = read_text(&marker, String::new()).map_err(|error| error.to_string())?;
    if text.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    Ok(value.is_object().then_some(value))
}

fn marker_stream(marker: &Value) -> Option<&str> {
    marker
        .get("stream")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

pub(crate) fn format_size(size: u64) -> String {
    if size >= 1_000_000 {
        format!("{:.1}M", size as f64 / 1_000_000.0)
    } else if size >= 1_000 {
        format!("{:.1}K", size as f64 / 1_000.0)
    } else {
        format!("{size}B")
    }
}

fn segment_duration(segment: &str) -> u64 {
    segment
        .split_once('_')
        .and_then(|(_, duration)| duration.parse().ok())
        .unwrap_or(0)
}

fn times(segment: &str) -> (Option<String>, Option<String>) {
    let Some(time) = segment_parse(segment) else {
        return (None, None);
    };
    let seconds = time.hour as u64 * 3600 + time.minute as u64 * 60 + time.second as u64;
    let end = (seconds + segment_duration(segment)) % 86_400;
    let start = NaiveTime::from_num_seconds_from_midnight_opt(seconds as u32, 0)
        .expect("parsed time is valid");
    let end = NaiveTime::from_num_seconds_from_midnight_opt(end as u32, 0)
        .expect("bounded time is valid");
    (
        Some(start.format("%H:%M:%S").to_string()),
        Some(end.format("%H:%M:%S").to_string()),
    )
}

#[derive(Default)]
struct Stats {
    files: u64,
    talents: u64,
    size: u64,
}

fn stats(path: &Path) -> Stats {
    let mut result = Stats::default();
    collect_stats(path, false, &mut result);
    result
}

fn collect_stats(path: &Path, in_talents: bool, result: &mut Stats) {
    let Ok(entries) = list_dir_entries(path) else {
        return;
    };
    for entry in entries {
        match entry.kind {
            DirEntryKind::File => {
                result.files += 1;
                result.size += std::fs::metadata(&entry.path)
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                if in_talents {
                    result.talents += 1;
                }
            }
            DirEntryKind::Directory => collect_stats(
                &entry.path,
                in_talents || entry.name.to_string_lossy() == "talents",
                result,
            ),
            DirEntryKind::Other => {}
        }
    }
}

fn top_level_files(path: &Path) -> Vec<String> {
    let mut files = list_dir_entries(path)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn talent_files(path: &Path) -> Vec<String> {
    let mut files = list_dir_entries(&path.join("talents"))
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn events_summary(path: &Path) -> Value {
    let text = read_text(path.join("events.jsonl"), String::new()).unwrap_or_default();
    let mut entries = 0_u64;
    let mut tracts = std::collections::BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        entries += 1;
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(tract) = value.get("tract").and_then(Value::as_str)
        {
            tracts.insert(tract.to_owned());
        }
    }
    json!({"entries": entries, "tracts": tracts.into_iter().collect::<Vec<_>>()})
}

fn next_day(day: &str) -> Option<String> {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()
        .map(|date| (date + Duration::days(1)).format("%Y%m%d").to_string())
}

pub(crate) fn successors(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Vec<SegmentLocation> {
    let mut result = Vec::new();
    for (scan_day, _) in list_days(journal).unwrap_or_default() {
        for candidate in list_segments(journal, &scan_day).unwrap_or_default() {
            let Ok(Some(marker)) = read_marker(&candidate.path) else {
                continue;
            };
            if marker_stream(&marker) != Some(stream)
                || marker.get("prev_day").and_then(Value::as_str) != Some(day)
                || marker.get("prev_segment").and_then(Value::as_str) != Some(segment)
            {
                continue;
            }
            if let Ok(location) =
                SegmentLocation::resolve(journal, &scan_day, &candidate.stream, &candidate.key)
            {
                result.push(location);
            }
        }
    }
    result.sort_by_key(|location| location.token());
    result
}

fn next_segment(journal: &Path, day: &str, stream: &str, segment: &str) -> Option<SegmentLocation> {
    let days = std::iter::once(day.to_owned()).chain(next_day(day));
    for scan_day in days {
        for candidate in list_segments(journal, &scan_day).unwrap_or_default() {
            let Ok(Some(marker)) = read_marker(&candidate.path) else {
                continue;
            };
            if marker_stream(&marker) == Some(stream)
                && marker.get("prev_day").and_then(Value::as_str) == Some(day)
                && marker.get("prev_segment").and_then(Value::as_str) == Some(segment)
            {
                return SegmentLocation::resolve(
                    journal,
                    &scan_day,
                    &candidate.stream,
                    &candidate.key,
                )
                .ok();
            }
        }
    }
    None
}

fn is_tail(journal: &Path, day: &str, stream: &str, segment: &str) -> bool {
    read_stream_record(journal, stream)
        .ok()
        .flatten()
        .is_some_and(|state| {
            state.get("last_day").and_then(Value::as_str) == Some(day)
                && state.get("last_segment").and_then(Value::as_str) == Some(segment)
        })
}

/// Resolves a chain pointer through its marker stream instead of assuming that
/// the owner-facing stream token is a directory component.
///
/// The default-stream token names a direct-under-day layout while markers keep
/// the real stream identity.  The physical layout therefore cannot be derived
/// from a chain marker and must be selected explicitly from discovered segments.
fn chain_location(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Option<SegmentLocation> {
    list_segments(journal, day)
        .ok()?
        .into_iter()
        .find_map(|candidate| {
            if candidate.key != segment {
                return None;
            }
            let Ok(Some(marker)) = read_marker(&candidate.path) else {
                return None;
            };
            (marker_stream(&marker) == Some(stream))
                .then(|| SegmentLocation::resolve(journal, day, &candidate.stream, segment).ok())
                .flatten()
        })
}

fn describe_prev(
    journal: &Path,
    location: &SegmentLocation,
    marker: &Value,
    stream: &str,
) -> String {
    let Some(previous) = marker.get("prev_segment").and_then(Value::as_str) else {
        return "(none)".to_owned();
    };
    let day = marker
        .get("prev_day")
        .and_then(Value::as_str)
        .unwrap_or(&location.day);
    let token = format!("{day}/{stream}/{previous}");
    match chain_location(journal, day, stream, previous) {
        Some(previous_location) if previous_location.path.is_dir() => token,
        _ => format!("{token} [MISSING]"),
    }
}

fn describe_next(journal: &Path, location: &SegmentLocation, stream: &str) -> String {
    if let Some(next) = next_segment(journal, &location.day, stream, &location.segment) {
        return format!("{}/{}/{}", next.day, stream, next.segment);
    }
    if is_tail(journal, &location.day, stream, &location.segment) {
        "(tail)".to_owned()
    } else {
        "(none)".to_owned()
    }
}

pub(crate) fn checks(journal: &Path, location: &SegmentLocation) -> Vec<Check> {
    let exists = location.path.is_dir();
    let marker = if exists {
        read_marker(&location.path).ok().flatten()
    } else {
        None
    };
    let stream = marker
        .as_ref()
        .and_then(marker_stream)
        .unwrap_or(location.stream.as_str());
    let stream_json = location.path.join("stream.json");
    let marker_valid = match &marker {
        Some(marker) if marker_stream(marker).is_some() => (true, "stream.json valid".to_owned()),
        Some(_) => (false, "stream.json missing stream field".to_owned()),
        None if stream_json.exists() => (false, "stream.json invalid JSON".to_owned()),
        None => (false, "stream.json missing".to_owned()),
    };
    let backward = match marker
        .as_ref()
        .and_then(|value| value.get("prev_segment"))
        .and_then(Value::as_str)
    {
        None => (true, "no previous segment".to_owned()),
        Some(_) => match (
            marker
                .as_ref()
                .and_then(|value| value.get("prev_day"))
                .and_then(Value::as_str),
            marker
                .as_ref()
                .and_then(|value| value.get("prev_segment"))
                .and_then(Value::as_str),
        ) {
            (Some(day), Some(segment)) => match chain_location(journal, day, stream, segment) {
                Some(previous) if previous.path.is_dir() => {
                    (true, "previous segment found".to_owned())
                }
                _ => (
                    false,
                    format!("missing previous segment {day}/{stream}/{segment}"),
                ),
            },
            _ => (false, "prev_segment set without prev_day".to_owned()),
        },
    };
    let forward =
        if let Some(next) = next_segment(journal, &location.day, stream, &location.segment) {
            (
                true,
                format!("next segment {}/{}/{}", next.day, stream, next.segment),
            )
        } else if is_tail(journal, &location.day, stream, &location.segment) {
            (true, "stream tail".to_owned())
        } else {
            (false, "next segment not found, not stream tail".to_owned())
        };
    let index = read_segment_index(journal, &location.index_rel);
    let index_check = match &index {
        SegmentIndexStatus::Absent => (true, "journal index not available".to_owned()),
        SegmentIndexStatus::Ready { indexed: true, .. } => (true, "segment indexed".to_owned()),
        SegmentIndexStatus::Ready { indexed: false, .. } => {
            (false, "segment not indexed".to_owned())
        }
        SegmentIndexStatus::Unreadable { error } => (
            false,
            format!("journal index error: {error} (run: journal indexer --rescan)"),
        ),
    };
    let content = exists
        && (location.path.join("audio.jsonl").exists()
            || location.path.join("screen.jsonl").exists()
            || list_dir_entries(&location.path)
                .unwrap_or_default()
                .into_iter()
                .any(|entry| {
                    entry.kind == DirEntryKind::File
                        && entry.name.to_string_lossy().starts_with("browser_")
                        && entry.name.to_string_lossy().ends_with(".jsonl")
                }));
    vec![
        Check {
            name: "directory exists",
            passed: exists,
            detail: if exists {
                "directory exists"
            } else {
                "directory missing"
            }
            .to_owned(),
        },
        Check {
            name: "stream.json exists",
            passed: stream_json.is_file(),
            detail: if stream_json.is_file() {
                "stream.json exists"
            } else {
                "stream.json missing"
            }
            .to_owned(),
        },
        Check {
            name: "stream.json valid",
            passed: marker_valid.0,
            detail: marker_valid.1,
        },
        Check {
            name: "content files present",
            passed: content,
            detail: if content {
                "content files present"
            } else if exists {
                "no audio.jsonl, screen.jsonl, or browser_*.jsonl"
            } else {
                "segment directory missing"
            }
            .to_owned(),
        },
        Check {
            name: "backward chain",
            passed: backward.0,
            detail: backward.1,
        },
        Check {
            name: "forward chain",
            passed: forward.0,
            detail: forward.1,
        },
        Check {
            name: "index presence",
            passed: index_check.0,
            detail: index_check.1,
        },
    ]
}

pub(crate) fn render_checks(checks: &[Check]) -> String {
    let mut output = String::new();
    for check in checks {
        if check.passed {
            output.push_str(&format!("{:<5} {}\n", "PASS", check.name));
        } else {
            output.push_str(&format!("{:<5} {}: {}\n", "FAIL", check.name, check.detail));
        }
    }
    output
}

pub(crate) fn list_output(
    journal: &Path,
    day: &str,
    stream_filter: Option<&str>,
    json_output: bool,
) -> String {
    let mut segments = list_segments(journal, day).unwrap_or_default();
    segments.retain(|segment| stream_filter.is_none_or(|stream| stream == segment.stream));
    // The reference leaves equal segment keys in filesystem order.  Make that
    // unspecified cross-stream case deterministic with the stream tie-breaker.
    segments.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then(left.stream.cmp(&right.stream))
    });
    if segments.is_empty() {
        return format!("No segments found for {day}\n");
    }
    let rows = segments.into_iter().map(|segment| {
        let (start, end) = times(&segment.key);
        let stats = stats(&segment.path);
        json!({"stream": segment.stream, "segment": segment.key, "start": start, "end": end,
            "duration": segment_duration(&segment.key), "files": stats.files, "talents": stats.talents, "size": stats.size})
    }).collect::<Vec<_>>();
    if json_output {
        return serde_json::to_string_pretty(&rows).expect("rows serialize") + "\n";
    }
    let mut output = format!(
        "{:<20} {:<14} {:<15} {:>5} {:>5} {:>7} {:>8}\n{}\n",
        "STREAM",
        "SEGMENT",
        "TIME",
        "DUR",
        "FILES",
        "TALENTS",
        "SIZE",
        "-".repeat(78)
    );
    for row in rows {
        let start = row["start"].as_str();
        let end = row["end"].as_str();
        let time = start
            .zip(end)
            .map(|(start, end)| format!("{start}-{end}"))
            .unwrap_or_else(|| "?".to_owned());
        output.push_str(&format!(
            "{:<20} {:<14} {:<15} {:>5} {:>5} {:>7} {:>8}\n",
            row["stream"].as_str().unwrap_or(""),
            row["segment"].as_str().unwrap_or(""),
            time,
            format!("{}s", row["duration"].as_u64().unwrap_or(0)),
            row["files"].as_u64().unwrap_or(0),
            row["talents"].as_u64().unwrap_or(0),
            format_size(row["size"].as_u64().unwrap_or(0))
        ));
    }
    output
}

pub(crate) fn inspect_output(
    journal: &Path,
    location: &SegmentLocation,
    json_output: bool,
) -> String {
    let marker = read_marker(&location.path)
        .ok()
        .flatten()
        .unwrap_or_else(|| json!({}));
    let stream = marker_stream(&marker).unwrap_or(&location.stream);
    let (start, end) = times(&location.segment);
    let files = top_level_files(&location.path);
    let talents = talent_files(&location.path);
    let stats = stats(&location.path);
    let events = events_summary(&location.path);
    let index = read_segment_index(journal, &location.index_rel);
    let (available, indexed, chunks, error) = index.fields();
    let previous = describe_prev(journal, location, &marker, stream);
    let next = describe_next(journal, location, stream);
    let payload = json!({
        "path": location.token(), "stream": stream, "segment": location.segment,
        "seq": marker.get("seq"), "prev_day": marker.get("prev_day"), "prev_segment": marker.get("prev_segment"),
        "start": start, "end": end, "duration": segment_duration(&location.segment),
        "chain": {"prev": previous, "next": next}, "files": files, "talents": talents,
        "stats": {"files": stats.files, "talents": stats.talents, "size": stats.size}, "events": events,
        "index": {"available": available, "indexed": indexed, "chunks": chunks, "error": error},
    });
    if json_output {
        return serde_json::to_string_pretty(&payload).expect("payload serializes") + "\n";
    }
    let range = start
        .zip(end)
        .map(|(start, end)| format!("{start} - {end}"))
        .unwrap_or_else(|| "?".to_owned());
    let mut output = format!("Segment: {}\n", location.token());
    if let Some(seq) = marker.get("seq") {
        output.push_str(&format!("Stream:  {stream} (seq {seq})\n"));
    } else {
        output.push_str(&format!("Stream:  {stream}\n"));
    }
    output.push_str(&format!(
        "Time:    {range} ({}s)\n\nChain:\n  prev: {previous}\n  next: {next}\n\nFiles ({}):\n",
        segment_duration(&location.segment),
        files.len()
    ));
    if !files.is_empty() {
        output.push_str(&format!("  {}\n", files.join(", ")));
    }
    output.push_str(&format!("\nTalents ({}):\n", talents.len()));
    if !talents.is_empty() {
        output.push_str(&format!("  {}\n", talents.join(", ")));
    }
    output.push_str(&format!("\nSize: {}\n", format_size(stats.size)));
    match index {
        SegmentIndexStatus::Unreadable { error } => output.push_str(&format!(
            "Index: error ({error}) - run: journal indexer --rescan\n"
        )),
        SegmentIndexStatus::Ready {
            indexed: true,
            chunks,
        } => output.push_str(&format!("Index: indexed ({chunks} chunks)\n")),
        SegmentIndexStatus::Ready { indexed: false, .. } => output.push_str("Index: not-indexed\n"),
        SegmentIndexStatus::Absent => output.push_str("Index: unavailable\n"),
    }
    let tracts = payload["events"]["tracts"]
        .as_array()
        .expect("events tracts")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let entries = payload["events"]["entries"].as_u64().unwrap_or(0);
    if tracts.is_empty() {
        output.push_str(&format!("Events: {entries} entries\n"));
    } else {
        output.push_str(&format!(
            "Events: {entries} entries ({})\n",
            tracts.join(", ")
        ));
    }
    output
}

pub(crate) fn day_segments(journal: &Path, day: &str) -> Vec<SegmentLocation> {
    let day_path = solstone_core_segment::day_path(journal, Some(day), false).ok();
    let Some(day_path) = day_path else {
        return Vec::new();
    };
    let mut segments = list_segments_in(journal, &day_path)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|segment| {
            SegmentLocation::resolve(journal, day, &segment.stream, &segment.key).ok()
        })
        .collect::<Vec<_>>();
    segments.sort_by(|left, right| {
        left.segment
            .cmp(&right.segment)
            .then(left.stream.cmp(&right.stream))
    });
    segments
}
