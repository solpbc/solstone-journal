// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A computer nobody touched is idle, even when Sense calls its screen active.
//!
//! Sense's own idle definition is fewer than 3 transcript lines and fewer
//! than 3 distinct screen frames, but its prompt also calls active-as-idle
//! the worst error, and it reads a static screen nobody touched as browsing.
//! This rule needs two independent signals before it overrides Sense, and it
//! is applied after generation, whatever generated the answer:
//!
//! 1. The segment is near-empty by Sense's own definition, counted from its
//!    stored files: under 3 transcript lines and under 3 screen frames, and
//!    no browser page or protected-video note.
//! 2. The segment's own desktop activity rows report the computer idle or
//!    locked for at least [`IDLE_SHARE`] of the segment's length.
//!
//! Then density and content type become `idle`, and no meeting is detected.
//! A segment with content, or without its own activity rows, keeps Sense's
//! answer. Rows written as a segment of their own describe no screen, so the
//! rule reaches a screen only where activity sits in its capture segment.

use crate::PreparedTalent;
use serde_json::{Map, Value};
use std::path::Path;

/// The share of a segment its activity rows must report idle or locked.
/// Sampling cadence and the segment's edges leave a few seconds unreported.
const IDLE_SHARE: f64 = 0.9;
/// Sense's near-empty bound, for transcript lines and for screen frames.
const NEAR_EMPTY: usize = 3;

/// The response typed idle, or `None` when the rule does not apply or changes nothing.
pub(crate) fn enforce(prepared: &PreparedTalent, journal: &Path, response: &str) -> Option<String> {
    if prepared.name != "sense" || prepared.config.contains_key("span") {
        return None;
    }
    let text = |key: &str| prepared.config.get(key).and_then(Value::as_str);
    let segment = text("segment")?;
    let seconds = segment
        .rsplit_once('_')?
        .1
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)?;
    let mut directory = solstone_core_journal_io::day_path(journal, text("day")?, false).ok()?;
    if let Some(stream) = text("stream").filter(|stream| !stream.is_empty()) {
        directory.push(stream);
    }
    directory.push(segment);
    if !untouched(&directory, seconds * 1000)? {
        return None;
    }
    let mut output = serde_json::from_str::<Value>(response).ok()?;
    let object = output.as_object_mut()?;
    let already = object.get("density").and_then(Value::as_str) == Some("idle")
        && object.get("content_type").and_then(Value::as_str) == Some("idle")
        && object.get("meeting_detected").and_then(Value::as_bool) != Some(true);
    if already {
        return None;
    }
    object.insert("density".to_owned(), Value::String("idle".to_owned()));
    object.insert("content_type".to_owned(), Value::String("idle".to_owned()));
    object.insert("meeting_detected".to_owned(), Value::Bool(false));
    Some(output.to_string())
}

/// Whether the segment is near-empty and its own activity reports it idle or
/// locked for nearly its whole length. `None` when the segment cannot be read.
fn untouched(directory: &Path, span_ms: u64) -> Option<bool> {
    let (mut lines, mut frames) = (0, 0);
    let mut idle = Vec::new();
    for entry in std::fs::read_dir(directory).ok()? {
        let path = entry.ok()?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        if name.ends_with("_desktop_activity.jsonl") {
            idle.extend(records(&path).iter().filter_map(idle_interval));
        } else if name.ends_with("screen.jsonl") {
            frames += records(&path)
                .iter()
                .filter(|record| record.contains_key("frame_id"))
                .count();
        } else if (name.starts_with("browser_") && name.ends_with(".jsonl"))
            || name.ends_with("_protected_video.jsonl")
        {
            if !records(&path).is_empty() {
                return Some(false);
            }
        } else if solstone_core_transcripts::is_speech_transcript_file(&path) {
            lines += records(&path)
                .iter()
                .filter(|record| {
                    record
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
                })
                .count();
        }
        if lines >= NEAR_EMPTY || frames >= NEAR_EMPTY {
            return Some(false);
        }
    }
    idle.sort_unstable();
    let (mut covered, mut reach) = (0u64, 0u64);
    for (start, end) in idle {
        let start = start.max(reach);
        if end > start {
            covered += end - start;
            reach = end;
        }
    }
    Some(covered as f64 >= IDLE_SHARE * span_ms as f64)
}

fn records(path: &Path) -> Vec<Map<String, Value>> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| match value {
            Value::Object(record) => Some(record),
            _ => None,
        })
        .collect()
}

/// An `activity` row reporting idle or locked, as `[ts, end_ts)` in milliseconds.
fn idle_interval(row: &Map<String, Value>) -> Option<(u64, u64)> {
    let field = |key: &str| row.get(key).and_then(Value::as_str);
    if field("t") != Some("desktop_activity")
        || field("kind") != Some("activity")
        || !matches!(field("state"), Some("idle" | "locked"))
    {
        return None;
    }
    let start = row.get("ts")?.as_u64()?;
    let end = row.get("end_ts")?.as_u64()?;
    (end > start).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    const START: u64 = 1_793_437_500_000;

    fn sense() -> PreparedTalent {
        PreparedTalent::new(
            "sense".into(),
            json!({"day": "20261031", "stream": "device", "segment": "030500_300"})
                .as_object()
                .unwrap()
                .clone(),
        )
    }

    fn answer() -> String {
        json!({"density": "active", "content_type": "browsing", "meeting_detected": false,
               "activity_summary": "Viewed the notes page."})
        .to_string()
    }

    fn segment(journal: &Path) -> PathBuf {
        let directory = journal.join("chronicle/20261031/device/030500_300");
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write(directory: &Path, name: &str, rows: &[Value]) {
        let text = rows
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        std::fs::write(directory.join(name), text).unwrap();
    }

    fn activity(state: &str, from_s: u64, to_s: u64) -> Value {
        json!({"v": 1, "t": "desktop_activity", "kind": "activity", "method": "quartz", "state": state,
               "ts": START + from_s * 1000, "end_ts": START + to_s * 1000})
    }

    /// A one-frame screen and an audio file with only its header, as an untouched Mac records.
    fn untouched_screen(directory: &Path) {
        write(
            directory,
            "030500_300_display_3_screen.jsonl",
            &[
                json!({"raw": "030500_300_display_3_screen.mp4"}),
                json!({"frame_id": 1, "content": {}}),
            ],
        );
        write(
            directory,
            "030500_300_audio.jsonl",
            &[json!({"raw": "030500_300_audio.m4a"})],
        );
    }

    fn scratch() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        // day_path resolves under the journal's chronicle.
        std::fs::create_dir_all(journal.path().join("chronicle")).unwrap();
        journal
    }

    fn typed(journal: &Path) -> Option<Value> {
        enforce(&sense(), journal, &answer()).map(|text| serde_json::from_str(&text).unwrap())
    }

    #[test]
    fn an_untouched_segment_whose_activity_reports_idle_becomes_idle() {
        let journal = scratch();
        let directory = segment(journal.path());
        untouched_screen(&directory);
        // Two files, overlapping, and a locked stretch: their union covers the segment.
        write(
            &directory,
            "030500_a_desktop_activity.jsonl",
            &[activity("idle", 0, 200)],
        );
        write(
            &directory,
            "030500_b_desktop_activity.jsonl",
            &[activity("idle", 150, 250), activity("locked", 250, 296)],
        );
        let output = typed(journal.path()).expect("the rule fires");
        assert_eq!(output["density"], "idle");
        assert_eq!(output["content_type"], "idle");
        assert_eq!(output["meeting_detected"], false);
        assert_eq!(output["activity_summary"], "Viewed the notes page.");
    }

    #[test]
    fn sense_keeps_its_answer_unless_both_signals_hold() {
        // Idle for most but not nearly all of the segment: the owner came back.
        let journal = scratch();
        let directory = segment(journal.path());
        untouched_screen(&directory);
        write(
            &directory,
            "030500_a_desktop_activity.jsonl",
            &[activity("idle", 0, 260), activity("active", 260, 300)],
        );
        assert!(typed(journal.path()).is_none());

        // Idle all the way, but there is something on screen, heard, or shown in a browser.
        let cases: [(&str, Vec<Value>); 3] = [
            (
                "030500_300_display_3_screen.jsonl",
                vec![
                    json!({"raw": "x"}),
                    json!({"frame_id": 1}),
                    json!({"frame_id": 2}),
                    json!({"frame_id": 3}),
                ],
            ),
            (
                "030500_300_audio.jsonl",
                vec![
                    json!({"raw": "x"}),
                    json!({"text": "one"}),
                    json!({"text": "two"}),
                    json!({"text": "three"}),
                ],
            ),
            (
                "browser_pages.jsonl",
                vec![json!({"url": "https://example.com/"})],
            ),
        ];
        for (name, rows) in cases {
            let journal = scratch();
            let directory = segment(journal.path());
            untouched_screen(&directory);
            write(
                &directory,
                "030500_a_desktop_activity.jsonl",
                &[activity("idle", 0, 300)],
            );
            write(&directory, name, &rows);
            assert!(typed(journal.path()).is_none(), "{name}");
        }

        // Near-empty with no activity rows of its own.
        let journal = scratch();
        untouched_screen(&segment(journal.path()));
        assert!(typed(journal.path()).is_none());
    }
}
