// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable relocation of a segment directory to a new day or key.
//!
//! A segment move is a segment write, so the mutation lives behind this door
//! rather than in the CLI that plans and renders it. Four durable steps happen
//! here: the directory move, `events.jsonl` re-stamping, the successor marker
//! repoint, and the stream-tail repair. Planning, index maintenance, and
//! post-move verification stay with the caller.
//!
//! The directory move is the pivot. It either happens or nothing does, so its
//! failure is a refusal. Every later step can fail on its own without unwinding
//! the move, so each reports separately and the caller decides what that means.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteOptions, JsonWriteOptions, LockOptions, atomic_replace, ensure_directory,
    find_available_segment, read_text, rename_within, write_json,
};

use crate::stream_repair::{MarkerTail, RepairOutcome, repair_stream_tail_from_markers};

/// A relocation step that could not be completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelocationError(String);

impl RelocationError {
    /// Describe a relocation step failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for RelocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for RelocationError {}

/// One end of a relocation: where a segment directory sits on disk.
#[derive(Clone, Debug)]
pub struct RelocationEnd {
    /// The `YYYYMMDD` day directory holding the segment.
    pub day: String,
    /// The `HHMMSS_LEN` segment key.
    pub segment: String,
    /// The absolute segment directory.
    pub path: PathBuf,
    /// The journal-relative segment directory.
    pub disk_rel: String,
    /// The journal-relative parent of the segment directory.
    pub parent_rel: String,
}

/// A planned relocation, ready to be applied.
pub struct Relocation<'a> {
    /// The journal root both ends are relative to.
    pub journal: &'a Path,
    /// Where the segment is now.
    pub source: &'a RelocationEnd,
    /// Where the segment is going.
    pub destination: &'a RelocationEnd,
    /// The successor whose chain marker points at `source`, when the caller's
    /// plan resolved exactly one. An absent or ambiguous chain is the caller's
    /// policy to report; this door patches only what it is handed.
    pub successor: Option<&'a RelocationEnd>,
    /// The stream named by the source marker, whose tail is repaired.
    pub stream: &'a str,
    /// The source marker sequence carried onto the destination.
    pub sequence: u64,
    /// How the stream record is locked during the tail repair.
    pub locks: LockOptions,
}

/// Why a relocation did not move the directory. Nothing was written.
#[derive(Debug)]
pub enum RelocationRefusal {
    /// The destination appeared between planning and the move.
    DestinationExists,
    /// The directory move itself failed.
    Failed(RelocationError),
}

/// What each durable step of an applied relocation did.
///
/// The directory move succeeded. Every field records a step taken after it.
#[derive(Debug)]
pub struct RelocationOutcome {
    /// Re-stamped `events.jsonl` object lines, or why none were re-stamped.
    pub events: Result<u64, RelocationError>,
    /// The successor marker repoint, when a successor was supplied.
    pub successor: Option<Result<(), RelocationError>>,
    /// The stream-tail repair.
    pub tail: RepairOutcome,
}

/// Choose an unused segment key under `parent`, claiming nothing.
///
/// Planning must know the key the move will land on before anything is
/// written, so the search lives with the mutation that consumes it rather than
/// with the caller that renders it.
pub fn available_segment_key(
    parent: &Path,
    candidate: &str,
    max_attempts: usize,
) -> Result<Option<String>, RelocationError> {
    find_available_segment(parent, candidate, max_attempts)
        .map_err(|error| RelocationError(error.to_string()))
}

/// Move a segment directory and re-author the identity that pointed at it.
pub fn relocate_segment(
    relocation: &Relocation<'_>,
) -> Result<RelocationOutcome, RelocationRefusal> {
    move_directory(relocation)?;
    let destination = relocation.destination;
    let events = rewrite_events(destination);
    let successor = relocation
        .successor
        .map(|end| patch_successor(end, &destination.day, &destination.segment));
    let tail = repair_stream_tail_from_markers(
        relocation.journal,
        relocation.stream,
        &MarkerTail {
            last_day: &destination.day,
            last_segment: &destination.segment,
            max_seq: relocation.sequence,
        },
        relocation.locks,
    );
    Ok(RelocationOutcome {
        events,
        successor,
        tail,
    })
}

fn move_directory(relocation: &Relocation<'_>) -> Result<(), RelocationRefusal> {
    let journal = relocation.journal;
    let destination = relocation.destination;
    ensure_directory(&journal.join(&destination.parent_rel))
        .map_err(|error| RelocationRefusal::Failed(RelocationError(error.to_string())))?;
    // The caller's collision check can become stale while it reads markers and
    // the index. This narrows that window to check-to-syscall, but cannot
    // make rename atomic: rename(2) has no no-replace mode through this seam.
    if destination.path.exists() {
        return Err(RelocationRefusal::DestinationExists);
    }
    rename_within(journal, &relocation.source.disk_rel, &destination.disk_rel)
        .map_err(|error| RelocationRefusal::Failed(RelocationError(error.to_string())))
}

/// Re-stamp `day` and `segment` on every object line, leaving the rest alone.
///
/// Non-object and unparsable lines survive byte-for-byte: a move must not be a
/// silent opportunity to normalize content the mover does not understand.
fn rewrite_events(destination: &RelocationEnd) -> Result<u64, RelocationError> {
    let path = destination.path.join("events.jsonl");
    let text =
        read_text(&path, String::new()).map_err(|error| RelocationError(error.to_string()))?;
    if text.is_empty() {
        return Ok(0);
    }
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut rewritten = Vec::with_capacity(lines.len());
    let mut count = 0;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            rewritten.push(line.to_owned());
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(mut value) if value.is_object() => {
                let object = value.as_object_mut().expect("object checked");
                object.insert("day".to_owned(), Value::String(destination.day.clone()));
                object.insert(
                    "segment".to_owned(),
                    Value::String(destination.segment.clone()),
                );
                rewritten.push(serde_json::to_string(&value).expect("value serializes"));
                count += 1;
            }
            Ok(_) | Err(_) => rewritten.push(line.to_owned()),
        }
    }
    let replacement = if rewritten.is_empty() {
        String::new()
    } else {
        format!("{}\n", rewritten.join("\n"))
    };
    atomic_replace(path, replacement.as_bytes(), AtomicWriteOptions::default())
        .map_err(|error| RelocationError(error.to_string()))?;
    Ok(count)
}

fn patch_successor(
    successor: &RelocationEnd,
    day: &str,
    segment: &str,
) -> Result<(), RelocationError> {
    let path = successor.path.join("stream.json");
    let text = read_text(&path, String::new())
        .map_err(|_| RelocationError::new("could not read marker"))?;
    let mut marker = serde_json::from_str::<Value>(&text)
        .map_err(|_| RelocationError::new("could not read marker"))?;
    let object = marker
        .as_object_mut()
        .ok_or_else(|| RelocationError::new("could not read marker"))?;
    object.insert("prev_day".to_owned(), Value::String(day.to_owned()));
    object.insert("prev_segment".to_owned(), Value::String(segment.to_owned()));
    write_json(path, &marker, JsonWriteOptions::default())
        .map_err(|error| RelocationError(error.to_string()))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::test_support::TempDir;

    struct Fixture {
        root: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                root: TempDir::new(),
            }
        }

        fn path(&self) -> &Path {
            self.root.path()
        }

        fn end(&self, day: &str, stream: &str, segment: &str) -> RelocationEnd {
            let (disk_rel, parent_rel) = if stream == solstone_core_journal_io::DEFAULT_STREAM {
                (
                    format!("chronicle/{day}/{segment}"),
                    format!("chronicle/{day}"),
                )
            } else {
                (
                    format!("chronicle/{day}/{stream}/{segment}"),
                    format!("chronicle/{day}/{stream}"),
                )
            };
            RelocationEnd {
                day: day.to_owned(),
                segment: segment.to_owned(),
                path: self.path().join(&disk_rel),
                disk_rel,
                parent_rel,
            }
        }

        fn create(&self, end: &RelocationEnd, marker: Value) -> &Self {
            fs::create_dir_all(&end.path).expect("segment directory");
            fs::write(
                end.path.join("stream.json"),
                serde_json::to_string(&marker).expect("marker serializes"),
            )
            .expect("marker written");
            self
        }

        fn stream_state(&self, stream: &str, day: &str, segment: &str, seq: u64) -> PathBuf {
            let path = self.path().join("streams").join(format!("{stream}.json"));
            fs::create_dir_all(path.parent().expect("streams parent")).expect("streams directory");
            fs::write(
                &path,
                serde_json::to_string(&json!({
                    "name": stream, "kind": "capture", "host": "desk", "platform": "linux",
                    "created_at": 7, "last_day": day, "last_segment": segment, "seq": seq,
                    "did": "device-1", "source": "microphone", "unknown": {"kept": true}
                }))
                .expect("state serializes"),
            )
            .expect("state written");
            path
        }
    }

    fn relocation<'a>(
        journal: &'a Path,
        source: &'a RelocationEnd,
        destination: &'a RelocationEnd,
        successor: Option<&'a RelocationEnd>,
        stream: &'a str,
        sequence: u64,
    ) -> Relocation<'a> {
        Relocation {
            journal,
            source,
            destination,
            successor,
            stream,
            sequence,
            locks: LockOptions::default(),
        }
    }

    #[test]
    fn all_four_durable_steps_land_and_preserve_unowned_marker_and_event_content() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "_default", "090000_60");
        let destination = fixture.end("20260305", "_default", "090000_60");
        let successor = fixture.end("20260307", "_default", "100000_60");
        fixture.create(&source, json!({"stream": "workstation", "seq": 5}));
        fixture.create(
            &successor,
            json!({"stream": "workstation", "seq": 6, "prev_day": "20260304",
                "prev_segment": "090000_60", "unknown": "kept"}),
        );
        fs::write(
            source.path.join("events.jsonl"),
            "{\"day\":\"20260304\",\"segment\":\"090000_60\"}\nnot json\n\n",
        )
        .expect("events written");
        let state = fixture.stream_state("workstation", "20260304", "090000_60", 5);
        let before: Value =
            serde_json::from_str(&fs::read_to_string(&state).expect("state read")).expect("state");

        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            Some(&successor),
            "workstation",
            5,
        ))
        .expect("relocation applies");

        assert!(!source.path.exists());
        assert!(destination.path.is_dir());
        assert_eq!(outcome.events, Ok(1));
        let events =
            fs::read_to_string(destination.path.join("events.jsonl")).expect("events read");
        assert!(events.contains("\"day\":\"20260305\""));
        assert!(events.contains("not json\n\n"));
        assert_eq!(outcome.successor, Some(Ok(())));
        let patched: Value = serde_json::from_str(
            &fs::read_to_string(successor.path.join("stream.json")).expect("marker read"),
        )
        .expect("marker");
        assert_eq!(patched["prev_day"], "20260305");
        assert_eq!(patched["prev_segment"], "090000_60");
        assert_eq!(patched["unknown"], "kept");
        assert!(matches!(
            outcome.tail,
            RepairOutcome::Repaired | RepairOutcome::Unchanged(_)
        ));
        let repaired: Value =
            serde_json::from_str(&fs::read_to_string(&state).expect("state read")).expect("state");
        for key in [
            "did",
            "source",
            "created_at",
            "kind",
            "host",
            "platform",
            "unknown",
        ] {
            assert_eq!(
                repaired.get(key),
                before.get(key),
                "{key} was not preserved"
            );
        }
        assert_eq!(repaired["last_day"], "20260305");
    }

    #[test]
    fn a_destination_appearing_after_planning_refuses_without_touching_either_segment() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));
        fixture.create(&destination, json!({"sentinel": true}));
        let source_before = fs::read(source.path.join("stream.json")).expect("source read");

        let refusal = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect_err("collision refuses");

        assert!(matches!(refusal, RelocationRefusal::DestinationExists));
        assert!(source.path.is_dir());
        assert_eq!(
            fs::read(source.path.join("stream.json")).expect("source read"),
            source_before
        );
        let held: Value = serde_json::from_str(
            &fs::read_to_string(destination.path.join("stream.json")).expect("destination read"),
        )
        .expect("marker");
        assert_eq!(held["sentinel"], true);
    }

    #[test]
    fn a_missing_source_refuses_and_leaves_the_created_destination_parent_empty() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");

        let refusal = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect_err("absent source refuses");

        assert!(matches!(refusal, RelocationRefusal::Failed(_)));
        assert!(!destination.path.exists());
    }

    #[test]
    fn an_absent_successor_is_not_patched_and_a_malformed_one_reports_without_unwinding() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));
        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect("relocation applies");
        assert_eq!(outcome.successor, None);
        assert!(destination.path.is_dir());

        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        let successor = fixture.end("20260306", "work", "100000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));
        fs::create_dir_all(&successor.path).expect("successor directory");
        fs::write(successor.path.join("stream.json"), "{ not json").expect("marker written");

        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            Some(&successor),
            "work",
            1,
        ))
        .expect("relocation applies");

        assert_eq!(
            outcome.successor,
            Some(Err(RelocationError::new("could not read marker")))
        );
        assert!(destination.path.is_dir(), "the move is not unwound");
    }

    #[test]
    fn a_stream_without_a_record_reports_no_record_after_the_directory_has_moved() {
        let fixture = Fixture::new();
        let source = fixture.end("20260304", "work", "090000_60");
        let destination = fixture.end("20260305", "work", "090000_60");
        fixture.create(&source, json!({"stream": "work", "seq": 1}));

        let outcome = relocate_segment(&relocation(
            fixture.path(),
            &source,
            &destination,
            None,
            "work",
            1,
        ))
        .expect("relocation applies");

        assert_eq!(outcome.tail, RepairOutcome::NoRecord);
        assert!(destination.path.is_dir());
    }

    #[test]
    fn key_search_returns_the_candidate_when_free_and_claims_nothing_either_way() {
        let fixture = Fixture::new();
        let parent = fixture.path().join("chronicle/20260305/work");
        fs::create_dir_all(&parent).expect("parent directory");

        assert_eq!(
            available_segment_key(&parent, "090000_60", 100).expect("search runs"),
            Some("090000_60".to_owned())
        );
        assert!(!parent.join("090000_60").exists(), "nothing was claimed");

        fs::create_dir_all(parent.join("090000_60")).expect("occupied key");
        let chosen = available_segment_key(&parent, "090000_60", 100)
            .expect("search runs")
            .expect("a free key exists");
        assert_ne!(chosen, "090000_60");
        assert!(!parent.join(&chosen).exists(), "nothing was claimed");

        assert!(available_segment_key(&parent, "not-a-key", 100).is_err());
    }
}
