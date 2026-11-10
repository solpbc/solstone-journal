// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The screen-text record (`talents/screen_text.md`) of a terminal-only segment.
//!
//! Describe builds a segment's screen text from the frames of its screen
//! video. A terminal segment has no frames: tmux hands over the text itself.
//! This builds the same record from those captures, each line at its first
//! appearance on a pane, masked like recognized text, so search and an agent
//! holding the screen-text grant read a terminal the way they read a screen.
//! A segment with anything but tmux captures stays describe's.

use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_format::content::screen_text::{
    append_screen_text_line, safe_lines_with_state, screen_text_minute,
};

/// Largest capture file read; a larger one leaves the segment as it is.
const MAX_CAPTURE_BYTES: u64 = 32 * 1024 * 1024;

const PROVENANCE: &str = "text shown in your terminal. this says what was visible, not what you read, typed or did. masking passwords and other secrets is best effort; your redaction rules do not apply.\n";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TerminalScreenText {
    /// Not a terminal-only segment, or its captures could not be read whole:
    /// whatever record it has is left alone.
    NotTerminal,
    /// The record a terminal-only segment should hold; `None` when its panes
    /// showed nothing.
    Record(Option<String>),
}

fn is_screen_capture(name: &str) -> bool {
    name == "screen.jsonl" || name.ends_with("_screen.jsonl")
}

pub(crate) fn segment_terminal_screen_text(segment_dir: &Path) -> TerminalScreenText {
    let Ok(entries) = fs::read_dir(segment_dir) else {
        return TerminalScreenText::NotTerminal;
    };
    let mut captures = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return TerminalScreenText::NotTerminal;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Recognition state means describe owns this segment's record.
        if name.starts_with(".screen-ocr-") {
            return TerminalScreenText::NotTerminal;
        }
        if is_screen_capture(name) {
            captures.push(entry.path());
        }
    }
    if captures.is_empty() {
        return TerminalScreenText::NotTerminal;
    }
    captures.sort();
    let mut frames = Vec::new();
    for path in captures {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            return TerminalScreenText::NotTerminal;
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_CAPTURE_BYTES {
            return TerminalScreenText::NotTerminal;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            return TerminalScreenText::NotTerminal;
        };
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                return TerminalScreenText::NotTerminal;
            };
            if record.get("timestamp").is_some() {
                frames.push(record);
            }
        }
    }
    if frames.is_empty() {
        return TerminalScreenText::NotTerminal;
    }
    let Some(shown) = solstone_core_talent_runtime::terminal_shown_lines(&frames) else {
        return TerminalScreenText::NotTerminal;
    };
    TerminalScreenText::Record(record_text(&shown))
}

fn record_text(shown: &[solstone_core_talent_runtime::ShownLines]) -> Option<String> {
    let mut record = String::from(PROVENANCE);
    let mut previous = None;
    let mut any = false;
    for item in shown {
        let clock = screen_text_minute(item.offset_ms);
        if clock.is_empty() {
            continue;
        }
        let mut redact_following = false;
        let lines = item
            .lines
            .iter()
            .flat_map(|line| safe_lines_with_state(line, &mut redact_following))
            .collect::<Vec<_>>();
        if lines.is_empty() {
            continue;
        }
        let group = (item.offset_ms / 1_000, item.pane);
        if previous != Some(group) {
            record.push_str(&format!("\n## {clock} · terminal\n"));
            previous = Some(group);
        }
        for line in lines {
            append_screen_text_line(&mut record, &line);
        }
        any = true;
    }
    any.then_some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(time: f64, session: &str, content: &str) -> String {
        json!({"timestamp": time, "content": {"tmux": {
            "session": session,
            "window": {"id": "@1", "index": 0, "name": "work"},
            "panes": [{"id": "%0", "index": 0, "active": true, "content": content}],
        }}})
        .to_string()
    }

    fn segment(files: &[(&str, String)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            fs::write(dir.path().join(name), body).unwrap();
        }
        dir
    }

    #[test]
    fn terminal_text_is_published_once_per_line_and_masked() {
        let token = "ghp_q8XzT3vLm9Kp2RwN7sYbF4cHdJ6aUeG1oVi5";
        let captures = [
            frame(4.0, "main", "$ make ci\nstatus-left-length 40\n"),
            frame(
                9.0,
                "main",
                &format!("$ make ci\nstatus-left-length 40\n$ export GH_TOKEN={token}\n"),
            ),
        ]
        .join("\n");
        let dir = segment(&[("tmux_0_screen.jsonl", captures)]);
        let TerminalScreenText::Record(Some(record)) = segment_terminal_screen_text(dir.path())
        else {
            panic!("expected a record");
        };
        let listed = |lines: &[&str]| {
            let mut text = String::new();
            for line in lines {
                append_screen_text_line(&mut text, line);
            }
            text
        };
        assert!(record.starts_with(PROVENANCE));
        let first = format!(
            "\n## 00:04 · terminal\n{}",
            listed(&["$ make ci", "status-left-length 40"])
        );
        assert!(record.contains(&first), "{record}");
        assert_eq!(
            record.matches(&listed(&["status-left-length 40"])).count(),
            1
        );
        let second = format!(
            "\n## 00:09 · terminal\n{}",
            listed(&["$ export GH_TOKEN=[REDACTED]"])
        );
        assert!(record.contains(&second), "{record}");
        assert!(!record.contains(token), "{record}");
    }

    #[test]
    fn a_captured_producer_envelope_becomes_plain_screen_text() {
        let fixture = include_str!(
            "../../solstone-core-format/tests/data/golden/tmux-observer-envelope-main.jsonl"
        );
        let dir = segment(&[("tmux_0_screen.jsonl", fixture.to_owned())]);
        let TerminalScreenText::Record(Some(record)) = segment_terminal_screen_text(dir.path())
        else {
            panic!("expected a record");
        };
        let mut expected = String::new();
        append_screen_text_line(&mut expected, "RED café");
        assert!(
            record.contains(&format!("\n## 00:00 · terminal\n{expected}")),
            "{record}"
        );
        assert!(!record.contains('\u{1b}'), "{record}");
    }

    #[test]
    fn a_segment_with_screen_frames_or_recognition_state_stays_describes() {
        let video_frames =
            json!({"timestamp": 1.0, "frame_id": 1, "analysis": {"primary": "code"}}).to_string();
        let dir = segment(&[("center_screen.jsonl", video_frames)]);
        assert_eq!(
            segment_terminal_screen_text(dir.path()),
            TerminalScreenText::NotTerminal
        );

        let dir = segment(&[
            ("tmux_0_screen.jsonl", frame(1.0, "main", "hello world\n")),
            (".screen-ocr-v3-folded.json", "{}".to_owned()),
        ]);
        assert_eq!(
            segment_terminal_screen_text(dir.path()),
            TerminalScreenText::NotTerminal
        );

        let dir = segment(&[("audio.jsonl", "{}".to_owned())]);
        assert_eq!(
            segment_terminal_screen_text(dir.path()),
            TerminalScreenText::NotTerminal
        );
    }

    #[test]
    fn panes_that_showed_nothing_hold_no_record() {
        let dir = segment(&[("tmux_0_screen.jsonl", frame(1.0, "main", "\n\n"))]);
        assert_eq!(
            segment_terminal_screen_text(dir.path()),
            TerminalScreenText::Record(None)
        );
    }
}
