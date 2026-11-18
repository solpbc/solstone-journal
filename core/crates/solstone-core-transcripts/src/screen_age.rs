// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Screen text marked by how long it has been showing, for an activity's story.
//!
//! Every frame reads as a complete screen under its own time, so a line that
//! stays put (an old command in a background terminal, an agent's earlier
//! edit) reads as if it happened at each frame's time. Here a line already
//! shown in an earlier frame of the same capture is marked: "already on screen
//! before this activity began" when it showed in the stream's segment before
//! the activity, otherwise "unchanged since" the time it first showed. Nothing
//! is removed, and new lines read as they always have.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::{Duration, NaiveDateTime, Timelike};
use serde_json::Value;

use super::{Segment, segment_times};

/// The mark on a line that showed before the activity's first segment.
pub(crate) const BEFORE_ACTIVITY: &str = "(already on screen before this activity began) ";
/// The mark on a line that showed before the segment a segment talent reads.
pub(crate) const BEFORE_SEGMENT: &str = "(already on screen before this segment began) ";

/// A line this short (a border, a lone bracket) carries no event worth marking.
const MIN_MARKED_CHARS: usize = 3;

/// The marked text of every screen capture file in `span`, by path. `prior` is
/// the stream's segment just before the span: its frames only say what was
/// already showing, and a line from it reads `before`. With `terminals` off, a
/// terminal capture's panes are left exactly as written.
pub(crate) fn mark_span(
    day: &str,
    prior: Option<&Segment>,
    span: &[Segment],
    is_screen: impl Fn(&str) -> bool,
    before: &'static str,
    terminals: bool,
) -> BTreeMap<PathBuf, String> {
    let mut marked = BTreeMap::new();
    let Some(activity_start) = span
        .first()
        .and_then(|segment| segment_times(day, &segment.key))
        .map(|(start, _)| start)
    else {
        return marked;
    };
    // First time each line showed, per capture file name.
    let mut first_seen = BTreeMap::<(String, String), NaiveDateTime>::new();
    for (index, segment) in prior.into_iter().chain(span).enumerate() {
        let in_span = prior.is_none() || index > 0;
        let Some((start, _)) = segment_times(day, &segment.key) else {
            continue;
        };
        for path in super::sorted_files(&segment.path) {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_screen(name) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let mut ages = Ages {
                file: name.to_owned(),
                first_seen: &mut first_seen,
                activity_start,
                at: start,
                mark: in_span,
                before,
                terminals,
            };
            let mut out = String::with_capacity(text.len());
            for line in text.lines() {
                out.push_str(&ages.record(start, line));
                out.push('\n');
            }
            if in_span {
                marked.insert(path, out);
            }
        }
    }
    marked
}

struct Ages<'a> {
    file: String,
    first_seen: &'a mut BTreeMap<(String, String), NaiveDateTime>,
    activity_start: NaiveDateTime,
    at: NaiveDateTime,
    mark: bool,
    before: &'static str,
    terminals: bool,
}

impl Ages<'_> {
    /// One JSONL row, marked when it is a frame; any other row as it was.
    fn record(&mut self, segment_start: NaiveDateTime, line: &str) -> String {
        let Ok(Value::Object(mut frame)) = serde_json::from_str::<Value>(line) else {
            return line.to_owned();
        };
        let Some(seconds) = frame.get("timestamp").and_then(Value::as_f64) else {
            return line.to_owned();
        };
        self.at = segment_start + Duration::milliseconds((seconds * 1000.0) as i64);
        let Some(Value::Object(content)) = frame.get_mut("content") else {
            return line.to_owned();
        };
        let mut seen = Vec::new();
        let mut changed = false;
        for (category, value) in content.iter_mut() {
            match value {
                Value::String(text) => {
                    let marked = self.text(text, &mut seen);
                    changed |= marked != *text;
                    *text = marked;
                }
                Value::Object(tmux) if category == "tmux" && self.terminals => {
                    if let Some(Value::Array(panes)) = tmux.get_mut("panes") {
                        for pane in panes.iter_mut().filter_map(Value::as_object_mut) {
                            if let Some(Value::String(text)) = pane.get_mut("content") {
                                let marked = self.text(text, &mut seen);
                                changed |= marked != *text;
                                *text = marked;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // A line counts as shown once its whole frame is read, so repeats
        // within one frame stay unmarked.
        for line in seen {
            self.first_seen
                .entry((self.file.clone(), line))
                .or_insert(self.at);
        }
        if !changed {
            return line.to_owned();
        }
        serde_json::to_string(&Value::Object(frame)).unwrap_or_else(|_| line.to_owned())
    }

    fn text(&self, text: &str, seen: &mut Vec<String>) -> String {
        let ends_with_newline = text.ends_with('\n');
        let mut out = text
            .lines()
            .map(|line| {
                // A terminal redraws the same words with different escape
                // codes, so a line is known by the text it shows.
                let shown = visible(line);
                let marked = self.mark && shown.chars().count() >= MIN_MARKED_CHARS;
                let first = self.first_seen.get(&(self.file.clone(), shown.clone()));
                seen.push(shown);
                if !marked {
                    return line.to_owned();
                }
                match first {
                    Some(first) if *first < self.activity_start => {
                        format!("{}{line}", self.before)
                    }
                    Some(first) if *first < self.at => {
                        format!("(unchanged since {}) {line}", clock(*first))
                    }
                    _ => line.to_owned(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if ends_with_newline {
            out.push('\n');
        }
        out
    }
}

/// The text a line shows: terminal escape sequences removed, outer spaces trimmed.
fn visible(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters and intermediates, then one final byte.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            // A charset designation carries one more byte.
            Some('(' | ')') => {
                chars.next();
            }
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// "10:03 PM", as the story's own times read.
fn clock(time: NaiveDateTime) -> String {
    let (pm, hour) = time.hour12();
    format!(
        "{hour}:{:02} {}",
        time.minute(),
        if pm { "PM" } else { "AM" }
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::{
        Sources, TalentSource, cluster_period_for_screen_talent, cluster_period_with_memory,
        cluster_span_with_memory, cluster_span_with_screen_age_with_memory,
    };

    const DAY: &str = "20260101";

    fn frames(root: &TempDir, segment: &str, file: &str, rows: &[serde_json::Value]) {
        let dir = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("laptop")
            .join(segment);
        fs::create_dir_all(&dir).unwrap();
        let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
        fs::write(dir.join(file), text).unwrap();
    }

    fn screen(seconds: f64, content: serde_json::Value) -> serde_json::Value {
        json!({"frame_id": 1, "timestamp": seconds, "requests": [],
               "analysis": {"visual_description": "A code editor", "primary": "code", "secondary": "terminal", "overlap": true},
               "content": content})
    }

    fn tmux(seconds: f64, pane: &str) -> serde_json::Value {
        json!({"frame_id": 1, "timestamp": seconds, "requests": [],
               "analysis": {"visual_description": "Terminal session 'kit'", "primary": "tmux", "secondary": "none", "overlap": false},
               "content": {"tmux": {"session": "kit", "window": {"id": "@1", "index": 0, "name": "bash"},
                                    "panes": [{"id": "%1", "index": 0, "active": true, "content": pane}]}}})
    }

    #[test]
    fn a_story_reads_lines_that_stay_on_screen_marked_and_new_lines_as_they_are() {
        let root = TempDir::new().unwrap();
        let file = "center_DP-1_screen.jsonl";
        frames(
            &root,
            "095500_300",
            file,
            &[screen(30.0, json!({"terminal": "$ chmod 000 notes"}))],
        );
        frames(
            &root,
            "095500_300",
            "tmux_kit_screen.jsonl",
            &[tmux(40.0, "$ mv a b")],
        );
        frames(
            &root,
            "100000_300",
            file,
            &[
                screen(
                    40.0,
                    json!({"code": "fn load() {}", "terminal": "$ chmod 000 notes"}),
                ),
                screen(
                    200.0,
                    json!({"code": "fn load() {}\n    validate()?;", "terminal": "$ chmod 000 notes"}),
                ),
            ],
        );
        frames(
            &root,
            "100000_300",
            "tmux_kit_screen.jsonl",
            &[tmux(10.0, "$ mv a b\n$ ls")],
        );
        frames(
            &root,
            "100500_300",
            "tmux_kit_screen.jsonl",
            // The same words redrawn with other escape codes.
            &[tmux(10.0, "$ mv a b\n\u{1b}[1m$ ls\u{1b}[0m\n$ git status")],
        );
        let sources = Sources {
            transcripts: false,
            percepts: true,
            talents: TalentSource::Disabled,
            voices: None,
            heard_owner: None,
            strava_tiles: false,
        };
        let span = ["100000_300", "100500_300"];
        let (aged, _, _) = cluster_span_with_screen_age_with_memory(
            root.path(),
            DAY,
            &span,
            &sources,
            Some("laptop"),
        )
        .unwrap();
        let aged = aged.text;
        // Showing before the activity began, on the desktop and in a terminal.
        assert_eq!(
            aged.matches("(already on screen before this activity began) $ chmod 000 notes")
                .count(),
            2,
            "{aged}"
        );
        assert_eq!(
            aged.matches("(already on screen before this activity began) $ mv a b")
                .count(),
            2,
            "{aged}"
        );
        // Unchanged since its first frame in the activity.
        assert!(
            aged.contains("(unchanged since 10:00 AM) fn load() {}"),
            "{aged}"
        );
        assert!(
            aged.contains("(unchanged since 10:00 AM) \\u001b[1m$ ls"),
            "{aged}"
        );
        // New lines read as they are, and the segment before is not read out.
        assert!(aged.contains("\n    validate()?;"), "{aged}");
        assert!(aged.contains("\\n$ git status"), "{aged}");
        assert!(!aged.contains("09:55:30"), "{aged}");

        let (plain, _, _) =
            cluster_span_with_memory(root.path(), DAY, &span, &sources, Some("laptop")).unwrap();
        assert!(
            !plain.text.contains("already on screen") && !plain.text.contains("unchanged since"),
            "{}",
            plain.text
        );
    }

    #[test]
    fn the_screen_record_reads_desktop_lines_from_the_segment_before_marked_and_terminals_as_they_were()
     {
        let root = TempDir::new().unwrap();
        let file = "center_DP-1_screen.jsonl";
        frames(
            &root,
            "095500_300",
            file,
            &[screen(30.0, json!({"terminal": "$ chmod 000 notes"}))],
        );
        frames(
            &root,
            "095500_300",
            "tmux_kit_screen.jsonl",
            &[tmux(40.0, "$ mv a b")],
        );
        frames(
            &root,
            "100000_300",
            file,
            &[
                screen(
                    40.0,
                    json!({"code": "fn load() {}", "terminal": "$ chmod 000 notes"}),
                ),
                screen(
                    200.0,
                    json!({"code": "fn load() {}\n    validate()?;", "terminal": "$ chmod 000 notes"}),
                ),
            ],
        );
        frames(
            &root,
            "100000_300",
            "tmux_kit_screen.jsonl",
            &[tmux(10.0, "$ mv a b\n$ ls")],
        );
        let sources = Sources {
            transcripts: false,
            percepts: true,
            talents: TalentSource::Disabled,
            voices: None,
            heard_owner: None,
            strava_tiles: false,
        };
        let (record, _) = cluster_period_for_screen_talent(
            root.path(),
            DAY,
            "100000_300",
            &sources,
            Some("laptop"),
        );
        let record = record.text;
        assert_eq!(
            record
                .matches("(already on screen before this segment began) $ chmod 000 notes")
                .count(),
            2,
            "{record}"
        );
        assert!(
            record.contains("(unchanged since 10:00 AM) fn load() {}"),
            "{record}"
        );
        assert!(!record.contains("this activity began"), "{record}");
        // The terminal reading marks earlier work itself: its capture is read as written.
        assert!(!record.contains(") $ mv a b"), "{record}");
        let (other, _, _) =
            cluster_period_with_memory(root.path(), DAY, "100000_300", &sources, Some("laptop"));
        assert!(
            !other.text.contains("already on screen") && !other.text.contains("unchanged since"),
            "{}",
            other.text
        );
    }
}
