// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, day_dirs, list_dir_entries, segment_path};

use crate::error::GrabFailure;
use crate::request::GrabDiagnostics;
use crate::time::{SegmentWindow, segment_window};

/// Deliberately deterministic; Python's source tuple was built from a frozenset.
pub(crate) const VIDEO_EXTENSIONS: [&str; 3] = [".webm", ".mp4", ".mov"];

#[derive(Clone, Debug)]
pub(crate) struct ScreenBundle {
    pub video_path: Option<PathBuf>,
    pub jsonl_rel: Option<String>,
    pub video_rel: Option<String>,
    pub frame_records: Vec<Value>,
    pub frame_index: HashMap<i64, Value>,
    pub legacy_schema: bool,
    pub header_only: bool,
    pub status: &'static str,
    pub window: SegmentWindow,
}

pub(crate) fn is_screen_token(stem: &str) -> bool {
    stem == "screen" || stem.ends_with("_screen")
}

pub(crate) fn normalize_screen_token(stem: &str) -> String {
    if stem == "screen" {
        stem.to_owned()
    } else {
        stem.strip_suffix("_screen").unwrap_or(stem).to_owned()
    }
}

pub(crate) fn screen_stem(token: &str) -> String {
    if is_screen_token(token) {
        token.to_owned()
    } else {
        format!("{token}_screen")
    }
}

pub(crate) fn available_days(journal: &Path) -> Result<Vec<String>, GrabFailure> {
    let mut days: Vec<_> = day_dirs(journal).map_err(path_error)?.into_keys().collect();
    days.sort();
    Ok(days)
}

pub(crate) fn closest_days(day: &str, days: &[String]) -> Vec<String> {
    if !day.bytes().all(|byte| byte.is_ascii_digit()) {
        return days.iter().take(5).cloned().collect();
    }
    let Ok(target) = day.parse::<i128>() else {
        return days.iter().take(5).cloned().collect();
    };
    let mut values = days.to_vec();
    values.sort_by_key(|value| {
        let number = value.parse::<i128>().unwrap_or_default();
        ((number - target).abs(), number)
    });
    values.truncate(5);
    values.sort();
    values
}

pub(crate) fn require_day(journal: &Path, day: &str) -> Result<PathBuf, GrabFailure> {
    let path = journal.join("chronicle").join(day);
    if path.is_dir() {
        return Ok(path);
    }
    Err(GrabFailure::runtime(format!(
        "day {day} not found{}",
        format_alternatives(
            "Available days (closest 5):",
            &closest_days(day, &available_days(journal)?)
        ),
    )))
}

pub(crate) fn require_stream(
    journal: &Path,
    day: &str,
    stream: &str,
) -> Result<PathBuf, GrabFailure> {
    let day_path = require_day(journal, day)?;
    let path = day_path.join(stream);
    if path.is_dir() {
        return Ok(path);
    }
    let streams = streams_except_health(&day_path)?;
    Err(GrabFailure::runtime(format!(
        "stream {stream} not found in {day}{}",
        format_alternatives(&format!("Available streams in {day}:"), &streams),
    )))
}

pub(crate) fn require_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<PathBuf, GrabFailure> {
    let stream_path = require_stream(journal, day, stream)?;
    let path = segment_path(journal, day, segment, stream, false).map_err(path_error)?;
    if path.is_dir() {
        return Ok(path);
    }
    let segments = truncate_segments(&available_segments(&stream_path)?);
    Err(GrabFailure::runtime(format!(
        "segment {segment} not found in {day}/{stream}{}",
        format_alternatives(&format!("Available segments in {day}/{stream}:"), &segments),
    )))
}

pub(crate) fn streams_except_health(day_path: &Path) -> Result<Vec<String>, GrabFailure> {
    Ok(list_dir_entries(day_path)
        .map_err(path_error)?
        .into_iter()
        .filter_map(|entry| {
            (entry.kind == DirEntryKind::Directory && entry.name != "health")
                .then(|| entry.name.to_string_lossy().into_owned())
        })
        .collect())
}

pub(crate) fn available_segments(stream_path: &Path) -> Result<Vec<String>, GrabFailure> {
    Ok(list_dir_entries(stream_path)
        .map_err(path_error)?
        .into_iter()
        .filter_map(|entry| {
            let name = entry.name.to_string_lossy().into_owned();
            (entry.kind == DirEntryKind::Directory && segment_window("20000101", &name).is_ok())
                .then_some(name)
        })
        .collect())
}

pub(crate) fn truncate_segments(segments: &[String]) -> Vec<String> {
    if segments.len() <= 20 {
        return segments.to_vec();
    }
    let mut result = segments[..10].to_vec();
    result.push("...".to_owned());
    result.extend_from_slice(&segments[segments.len() - 10..]);
    result
}

pub(crate) fn available_screen_tokens(segment_path: &Path) -> Result<Vec<String>, GrabFailure> {
    let mut tokens = BTreeSet::new();
    for entry in list_dir_entries(segment_path).map_err(path_error)? {
        if entry.kind != DirEntryKind::File {
            continue;
        }
        let path = entry.path;
        let suffix = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if (suffix.as_deref() == Some("jsonl")
            || suffix
                .as_deref()
                .is_some_and(|value| VIDEO_EXTENSIONS.contains(&format!(".{value}").as_str())))
            && is_screen_token(stem)
        {
            tokens.insert(normalize_screen_token(stem));
        }
    }
    Ok(tokens.into_iter().collect())
}

pub(crate) fn load_bundle(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    token: &str,
    keep_errors: bool,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<ScreenBundle, GrabFailure> {
    let segment_path = require_segment(journal, day, stream, segment)?;
    let stem = screen_stem(token);
    let candidate_jsonl = segment_path.join(format!("{stem}.jsonl"));
    let (jsonl_path, records) = if candidate_jsonl.is_file() {
        (
            Some(candidate_jsonl.clone()),
            load_analysis_frames(&candidate_jsonl, keep_errors, diagnostics),
        )
    } else {
        (None, Vec::new())
    };
    let header = records
        .first()
        .filter(|record| has_key(record, "raw") && !has_key(record, "frame_id"));
    let mut video_path = header
        .and_then(|record| record.get("raw"))
        .and_then(Value::as_str)
        .filter(|raw| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|extension| raw.ends_with(extension))
        })
        .map(|raw| segment_path.join(raw))
        .filter(|path| path.is_file());
    if video_path.is_none() {
        video_path = VIDEO_EXTENSIONS
            .iter()
            .map(|extension| segment_path.join(format!("{stem}{extension}")))
            .find(|path| path.is_file());
    }
    let mut frame_records: Vec<_> = records
        .iter()
        .filter(|record| has_key(record, "frame_id"))
        .cloned()
        .collect();
    frame_records.sort_by_key(frame_id);
    let frame_index = frame_records
        .iter()
        .filter_map(|record| frame_id(record).map(|id| (id, record.clone())))
        .collect();
    let non_header = if header.is_some() {
        &records[1..]
    } else {
        records.as_slice()
    };
    let header_only = jsonl_path.is_some() && frame_records.is_empty() && non_header.is_empty();
    let legacy_schema = jsonl_path.is_some() && frame_records.is_empty() && !non_header.is_empty();
    let status = match (jsonl_path.is_some(), video_path.is_some()) {
        (false, true) => "captured but not analyzed",
        (true, false) => "analyzed; raw media purged by retention",
        (true, true) => "analyzed",
        (false, false) => {
            return Err(GrabFailure::runtime(format!(
                "screen {token} not found in {day}/{stream}/{segment}{}",
                format_alternatives(
                    &format!("Available screens in {day}/{stream}/{segment}:"),
                    &available_screen_tokens(&segment_path)?
                ),
            )));
        }
    };
    let window = segment_window(day, segment)?;
    Ok(ScreenBundle {
        video_path: video_path.as_ref().map(PathBuf::from),
        jsonl_rel: jsonl_path
            .as_deref()
            .map(|path| journal_relative(journal, path)),
        video_rel: video_path
            .as_deref()
            .map(|path| journal_relative(journal, path)),
        frame_records,
        frame_index,
        legacy_schema,
        header_only,
        status,
        window,
    })
}

pub(crate) fn frame_id(record: &Value) -> Option<i64> {
    record.get("frame_id").and_then(coerce_frame_id)
}

pub(crate) fn coerce_frame_id(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|number| number as i64)),
        Value::String(value) => value
            .parse::<i64>()
            .ok()
            .or_else(|| value.parse::<f64>().ok().map(|number| number as i64)),
        Value::Bool(value) => Some(i64::from(*value)),
        _ => None,
    }
}

fn load_analysis_frames(
    path: &Path,
    keep_errors: bool,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Vec<Value> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            diagnostics.read_error(path, &error);
            return Vec::new();
        }
    };
    let mut header = None;
    let mut frames = Vec::new();
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(record) => {
                if has_key(&record, "error") {
                    if keep_errors && has_key(&record, "frame_id") {
                        frames.push(record);
                    }
                } else if !has_key(&record, "frame_id") && header.is_none() {
                    header = Some(record);
                } else {
                    frames.push(record);
                }
            }
            Err(error) => diagnostics.malformed_jsonl(path, index + 1, &error.to_string()),
        }
    }
    frames.sort_by_key(frame_id);
    header.into_iter().chain(frames).collect()
}

fn has_key(value: &Value, key: &str) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key(key))
}

fn journal_relative(journal: &Path, path: &Path) -> String {
    path.strip_prefix(journal.join("chronicle"))
        .or_else(|_| path.strip_prefix(journal))
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn format_alternatives(header: &str, values: &[String]) -> String {
    let mut result = format!("\n\n{header}");
    for value in values {
        result.push_str(&format!("\n  {value}"));
    }
    result
}

fn path_error(error: solstone_core_journal_io::PathError) -> GrabFailure {
    GrabFailure::runtime(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        available_screen_tokens, closest_days, load_analysis_frames, load_bundle,
        normalize_screen_token, screen_stem, streams_except_health, truncate_segments,
    };
    use crate::RecordingDiagnostics;

    fn segment(root: &std::path::Path) -> std::path::PathBuf {
        let path = root.join("chronicle/20260809/work/120000_300");
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn screen_tokens_and_alternatives_match_reference_rules() {
        assert_eq!(normalize_screen_token("center_DP-3_screen"), "center_DP-3");
        assert_eq!(
            normalize_screen_token("center_DP-3_screen_screen"),
            "center_DP-3_screen"
        );
        assert_eq!(screen_stem("screen"), "screen");
        assert_eq!(screen_stem("center_DP-3"), "center_DP-3_screen");
        assert_eq!(
            closest_days(
                "20260812",
                &[
                    "20260801".into(),
                    "20260810".into(),
                    "20260811".into(),
                    "20260813".into(),
                    "20260814".into(),
                    "20260815".into()
                ]
            ),
            vec!["20260810", "20260811", "20260813", "20260814", "20260815"]
        );
        assert_eq!(
            closest_days("today", &["b".into(), "a".into()]),
            vec!["b", "a"]
        );
        let values: Vec<_> = (0..22).map(|value| format!("{value:02}")).collect();
        assert_eq!(truncate_segments(&values).len(), 21);
    }

    #[test]
    fn header_raw_wins_and_schema_statuses_are_classified() {
        let temp = tempdir().unwrap();
        let path = segment(temp.path());
        fs::write(path.join("screen.webm"), b"probe").unwrap();
        fs::write(path.join("other.mov"), b"header").unwrap();
        fs::write(
            path.join("screen.jsonl"),
            "{\"raw\": \"other.mov\"}\n{\"frame_id\": 7, \"timestamp\": 1}\n",
        )
        .unwrap();
        let mut diagnostics = RecordingDiagnostics::default();
        let bundle = load_bundle(
            temp.path(),
            "20260809",
            "work",
            "120000_300",
            "screen",
            true,
            &mut diagnostics,
        )
        .unwrap();
        assert!(bundle.video_path.unwrap().ends_with("other.mov"));
        assert_eq!(bundle.status, "analyzed");
        fs::write(path.join("legacy_screen.jsonl"), "{\"old\": true}\n").unwrap();
        let legacy = load_bundle(
            temp.path(),
            "20260809",
            "work",
            "120000_300",
            "legacy",
            true,
            &mut diagnostics,
        )
        .unwrap();
        assert!(legacy.legacy_schema);
        fs::write(
            path.join("empty_screen.jsonl"),
            "{\"raw\": \"missing.webm\"}\n",
        )
        .unwrap();
        let empty = load_bundle(
            temp.path(),
            "20260809",
            "work",
            "120000_300",
            "empty",
            true,
            &mut diagnostics,
        )
        .unwrap();
        assert!(empty.header_only);
        fs::write(path.join("captured_screen.webm"), b"raw").unwrap();
        let captured = load_bundle(
            temp.path(),
            "20260809",
            "work",
            "120000_300",
            "captured",
            true,
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(captured.status, "captured but not analyzed");
        fs::write(path.join("purged_screen.jsonl"), "{\"frame_id\": 1}\n").unwrap();
        let purged = load_bundle(
            temp.path(),
            "20260809",
            "work",
            "120000_300",
            "purged",
            true,
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(purged.status, "analyzed; raw media purged by retention");
    }

    #[test]
    fn corrupt_lines_are_skipped_and_reported() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("screen.jsonl");
        fs::write(
            &file,
            "{\"raw\": \"screen.webm\"}\nnot json\n{\"frame_id\": 2}\n",
        )
        .unwrap();
        let mut diagnostics = RecordingDiagnostics::default();
        let records = load_analysis_frames(&file, true, &mut diagnostics);
        assert_eq!(
            records,
            vec![json!({"raw":"screen.webm"}), json!({"frame_id":2})]
        );
        assert_eq!(diagnostics.malformed.len(), 1);
        assert_eq!(diagnostics.malformed[0].1, 2);
        let directory = temp.path().join("unreadable");
        fs::create_dir(&directory).unwrap();
        assert!(load_analysis_frames(&directory, true, &mut diagnostics).is_empty());
        assert_eq!(diagnostics.read_errors.len(), 1);
    }

    #[test]
    fn screen_inventory_and_stream_alternatives_are_read_only() {
        let temp = tempdir().unwrap();
        let path = segment(temp.path());
        fs::write(path.join("center_DP-3_screen.jsonl"), "").unwrap();
        fs::write(path.join("screen.MP4"), "").unwrap();
        assert_eq!(
            available_screen_tokens(&path).unwrap(),
            vec!["center_DP-3", "screen"]
        );
        fs::create_dir_all(temp.path().join("chronicle/20260809/empty")).unwrap();
        fs::create_dir_all(temp.path().join("chronicle/20260809/health")).unwrap();
        assert_eq!(
            streams_except_health(&temp.path().join("chronicle/20260809")).unwrap(),
            vec!["empty", "work"]
        );
    }

    #[test]
    fn frame_id_coercion_matches_python_int_for_common_json_values() {
        assert_eq!(super::coerce_frame_id(&json!(7.0)), Some(7));
        assert_eq!(super::coerce_frame_id(&json!("7")), Some(7));
        assert_eq!(super::coerce_frame_id(&json!(true)), Some(1));
    }
}
