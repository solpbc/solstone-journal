// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The segment's searchable object summary, `talents/objects.md`.
//!
//! Describe and depict store raw detections on each frame or still row. This
//! summarizes what passes the read-side policy across the segment, so a
//! search for an object reaches the segment that shows it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_format::objects::{object_counts, qualified_objects};
use solstone_core_format::stored_markdown::{Document, Message};

#[derive(Default)]
struct ClassTally {
    frames: usize,
    photos: usize,
    most_at_once: usize,
}

/// Markdown for every qualified object in the segment's detection rows, or
/// `None` when nothing qualified. Unreadable files and rows are skipped:
/// detections are best-effort enrichment and never block the segment.
pub(crate) fn segment_objects_markdown(segment_dir: &Path) -> Option<Document> {
    let mut files = fs::read_dir(segment_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect::<Vec<_>>();
    files.sort();

    let mut tallies = BTreeMap::<String, ClassTally>::new();
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            if !line.contains("\"detections\"") {
                continue;
            }
            let Ok(row) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(detections) = row.get("detections").filter(|value| value.is_object()) else {
                continue;
            };
            let still = detections.get("source").and_then(Value::as_str) == Some("still");
            for (name, count) in object_counts(&qualified_objects(detections)) {
                let tally = tallies.entry(name).or_default();
                if still {
                    tally.photos += 1;
                } else {
                    tally.frames += 1;
                }
                tally.most_at_once = tally.most_at_once.max(count);
            }
        }
    }
    if tallies.is_empty() {
        return None;
    }

    let mut ordered = tallies.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        (right.1.frames + right.1.photos)
            .cmp(&(left.1.frames + left.1.photos))
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut document = Document::default();
    document.push_str("# ");
    document.push_message(Message::Objects);
    document.push_str("\n\n");
    for (class, tally) in ordered {
        document.push_str("- ");
        document.push_message(Message::Object {
            class,
            frames: tally.frames,
            photos: tally.photos,
            most: tally.most_at_once,
        });
        document.push_str("\n");
    }
    Some(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(source: &str, objects: Value) -> String {
        json!({"timestamp": 1.0, "detections": {"source": source, "objects": objects}}).to_string()
    }

    #[test]
    fn summarizes_qualified_objects_across_screen_and_still_rows() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("left_screen.jsonl"),
            [
                r#"{"raw":"left_screen.webm"}"#.to_owned(),
                row(
                    "screen",
                    json!([
                        {"class_name": "person", "score": 0.9},
                        {"class_name": "person", "score": 0.8},
                        {"class_name": "person", "score": 0.7},
                        {"class_name": "laptop", "score": 0.9}
                    ]),
                ),
                row("screen", json!([{"class_name": "person", "score": 0.5}])),
                "not json \"detections\"".to_owned(),
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            dir.path().join("still-1.jsonl"),
            row("still", json!([{"class_name": "cup", "score": 0.6}])),
        )
        .unwrap();
        fs::write(
            dir.path().join("audio.jsonl"),
            r#"{"start":"00:00:01","text":"hi"}"#,
        )
        .unwrap();

        assert_eq!(
            segment_objects_markdown(dir.path()).as_deref(),
            Some("# Objects\n\n- person: in 2 frames, up to 3 at once\n- cup: in 1 photo\n")
        );
    }

    #[test]
    fn nothing_qualified_is_none() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("screen.jsonl"),
            row("screen", json!([{"class_name": "tv", "score": 0.9}])),
        )
        .unwrap();
        assert_eq!(segment_objects_markdown(dir.path()), None);
        assert_eq!(segment_objects_markdown(&dir.path().join("missing")), None);
    }
}
