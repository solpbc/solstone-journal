// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Segments holding only body records.
//!
//! A body window holds records of the owner's body, written by code, and owes no
//! thinking. That is decided by what the segment holds and never by its stream
//! name alone: older content in the same stream keeps its behaviour.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use solstone_core_format::content::body_records::{BODY_RECORD_STREAMS, BODY_RECORDS_FILE};
use solstone_core_journal_io::{PathOrDay, Segment, is_incidental_file_name, iter_segments};

use crate::calendar::is_calendar_segment;

const STREAM_MARKER: &str = "stream.json";
const TOMBSTONE: &str = "tombstone.json";
const TALENTS: &str = "talents";
const FACETS: &str = "facets.json";

/// What a segment's tombstone says was removed, from its manifest of top-level
/// names. The tombstone is the only thing that survives a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TombstoneRecord {
    /// `body.jsonl`, and only what a body window holds beside it.
    BodyWindow,
    /// Real content and no body records: something older than this contract.
    OlderContent,
    /// Missing, malformed, empty or mixed. It says neither.
    CantSay,
}

fn tombstone_record(path: &Path) -> TombstoneRecord {
    let Ok(bytes) = fs::read(path) else {
        return TombstoneRecord::CantSay;
    };
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return TombstoneRecord::CantSay;
    };
    let Some(manifest) = body.get("manifest").and_then(serde_json::Value::as_array) else {
        return TombstoneRecord::CantSay;
    };
    let mut records = false;
    let mut content = false;
    for entry in manifest {
        let Some(name) = entry
            .as_str()
            .and_then(|entry| Path::new(entry).file_name())
        else {
            return TombstoneRecord::CantSay;
        };
        if name == OsStr::new(BODY_RECORDS_FILE) {
            records = true;
        } else if name != OsStr::new(STREAM_MARKER)
            && name != OsStr::new(TALENTS)
            && !is_incidental_file_name(name)
        {
            content = true;
        }
    }
    match (records, content) {
        (true, false) => TombstoneRecord::BodyWindow,
        (false, true) => TombstoneRecord::OlderContent,
        _ => TombstoneRecord::CantSay,
    }
}

/// Whether this directory, in `stream`, holds body records and nothing else that
/// says otherwise. A tombstone counts only when its manifest shows a body window,
/// so one owner delete of a window doesn't turn a day into something owed, and
/// the delete of anything older keeps its behaviour. An unreadable directory
/// isn't one: it keeps counting as it always has.
#[must_use]
pub fn is_body_only_dir(stream: &str, dir: &Path) -> bool {
    if !BODY_RECORD_STREAMS.contains(&stream) {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut holds_records = false;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let name = entry.file_name();
        if name == OsStr::new(TOMBSTONE) {
            if tombstone_record(&entry.path()) != TombstoneRecord::BodyWindow {
                return false;
            }
            holds_records = true;
        } else if name == OsStr::new(BODY_RECORDS_FILE) {
            holds_records = true;
        } else if name == OsStr::new(TALENTS) {
            if !talents_hold_only_facets(&entry.path()) {
                return false;
            }
        } else if name != OsStr::new(STREAM_MARKER) && !is_incidental_file_name(&name) {
            return false;
        }
    }
    holds_records
}

/// Whether this directory holds anything a body window wouldn't: older content
/// that new records must not land in, live or deleted. A tombstone that can't say
/// what it removed isn't foreign, so a window the owner deleted stays deleted.
#[must_use]
pub fn holds_foreign_content(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.into_iter().any(|entry| {
        entry.is_ok_and(|entry| {
            let name = entry.file_name();
            if name == OsStr::new(TALENTS) {
                return !talents_hold_only_facets(&entry.path());
            }
            if name == OsStr::new(TOMBSTONE) {
                return tombstone_record(&entry.path()) == TombstoneRecord::OlderContent;
            }
            name != OsStr::new(BODY_RECORDS_FILE)
                && name != OsStr::new(STREAM_MARKER)
                && !is_incidental_file_name(&name)
        })
    })
}

fn talents_hold_only_facets(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.into_iter().all(|entry| {
        entry.is_ok_and(|entry| {
            let name = entry.file_name();
            name == OsStr::new(FACETS) || is_incidental_file_name(&name)
        })
    })
}

/// Whether a listed segment holds only body records.
#[must_use]
pub fn is_body_only_segment(segment: &Segment) -> bool {
    segment
        .stream()
        .directory()
        .and_then(OsStr::to_str)
        .is_some_and(|stream| is_body_only_dir(stream, segment.path()))
}

/// Whether a listed segment holds nothing the owner did: calendar notes say what
/// was planned, and body records come from another system's account of the day.
#[must_use]
pub fn holds_nothing_the_owner_did(journal: &Path, segment: &Segment) -> bool {
    is_calendar_segment(journal, segment) || is_body_only_segment(segment)
}

/// Whether `day` has segments and every one holds nothing the owner did. Such a
/// day owes no thinking.
#[must_use]
pub fn day_holds_nothing_the_owner_did(journal: &Path, day: &str) -> bool {
    iter_segments(journal, PathOrDay::Day(day)).is_ok_and(|segments| {
        !segments.is_empty()
            && segments
                .iter()
                .all(|segment| holds_nothing_the_owner_did(journal, segment))
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const STREAM: &str = "import.apple_health";

    fn window(
        root: &Path,
        day: &str,
        stream: &str,
        key: &str,
        files: &[&str],
    ) -> std::path::PathBuf {
        let dir = root.join("chronicle").join(day).join(stream).join(key);
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "{}\n").unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const BODY_TOMBSTONE: &str = r#"{"manifest":[
        "chronicle/20261010/import.apple_health/064000_300/body.jsonl",
        "chronicle/20261010/import.apple_health/064000_300/stream.json"
    ]}"#;
    const CARD_TOMBSTONE: &str = r#"{"manifest":[
        "chronicle/20261009/import.apple_health/000000_300/day_summary_transcript.md"
    ]}"#;

    fn tombstoned(root: &Path, day: &str, key: &str, tombstone: &str) -> std::path::PathBuf {
        let dir = window(root, day, STREAM, key, &[]);
        fs::write(dir.join("tombstone.json"), tombstone).unwrap();
        dir
    }

    #[test]
    fn a_tombstone_counts_only_when_its_manifest_shows_a_body_window() {
        let journal = TempDir::new();
        let root = journal.path();
        // (tombstone, body-only, new rows step around it)
        for (index, (tombstone, body_only, foreign)) in [
            (BODY_TOMBSTONE, true, false),
            (
                r#"{"manifest":["a/body.jsonl","a/body.jsonl.lock","a/.DS_Store","a/talents"]}"#,
                true,
                false,
            ),
            (CARD_TOMBSTONE, false, true),
            (
                r#"{"manifest":["a/day_summary_transcript.md","a/stream.json","a/talents"]}"#,
                false,
                true,
            ),
            // It can't say: thinking keeps today's behaviour, and rows stay out.
            (
                r#"{"manifest":["a/body.jsonl","a/day_summary_transcript.md"]}"#,
                false,
                false,
            ),
            (r#"{"manifest":["a/stream.json"]}"#, false, false),
            (r#"{"manifest":[]}"#, false, false),
            (r#"{"manifest":[7]}"#, false, false),
            (r#"{"manifest":"a/body.jsonl"}"#, false, false),
            ("{}", false, false),
            ("not json", false, false),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = tombstoned(root, "20261010", &format!("{index:06}_300"), tombstone);
            assert_eq!(is_body_only_dir(STREAM, &dir), body_only, "{tombstone}");
            assert_eq!(holds_foreign_content(&dir), foreign, "{tombstone}");
        }
    }

    #[test]
    fn body_only_follows_what_the_directory_holds() {
        let journal = TempDir::new();
        let root = journal.path();
        for (index, (files, expected)) in [
            (vec!["body.jsonl"], true),
            (vec!["body.jsonl", "body.jsonl.lock", "stream.json"], true),
            (vec!["body.jsonl", ".DS_Store", "._body.jsonl"], true),
            (vec!["body.jsonl", "talents/facets.json"], true),
            (
                vec![
                    "body.jsonl",
                    "talents/.DS_Store",
                    "talents/facets.json.lock",
                ],
                true,
            ),
            (vec!["stream.json"], false),
            (vec!["body.jsonl", "imported.md"], false),
            (vec!["body.jsonl", "audio.jsonl"], false),
            (vec!["body.jsonl", "day_summary_transcript.md"], false),
            (vec!["body.jsonl", "talents/sense.json"], false),
            (vec!["body.jsonl", "something.bin"], false),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = window(root, "20261010", STREAM, &format!("{index:06}_300"), &files);
            assert_eq!(is_body_only_dir(STREAM, &dir), expected, "{files:?}");
        }
        let strava = window(
            root,
            "20261010",
            "import.strava",
            "120000_300",
            &["body.jsonl"],
        );
        assert!(!is_body_only_dir("import.strava", &strava));
        assert!(!is_body_only_dir(STREAM, &root.join("absent")));
    }

    #[test]
    fn a_day_holds_nothing_the_owner_did_only_when_every_segment_agrees() {
        let journal = TempDir::new();
        let root = journal.path();
        window(root, "20261010", STREAM, "063000_300", &["body.jsonl"]);
        tombstoned(root, "20261010", "064000_300", BODY_TOMBSTONE);
        assert!(day_holds_nothing_the_owner_did(root, "20261010"));
        // A deleted card on a day of its own, or beside a body window, leaves it owed.
        tombstoned(root, "20261009", "000000_300", CARD_TOMBSTONE);
        assert!(!day_holds_nothing_the_owner_did(root, "20261009"));
        window(root, "20261009", STREAM, "000000_301", &["body.jsonl"]);
        assert!(!day_holds_nothing_the_owner_did(root, "20261009"));
        window(root, "20261011", STREAM, "063000_300", &["body.jsonl"]);
        window(root, "20261011", "phone", "090000_300", &["audio.jsonl"]);
        assert!(!day_holds_nothing_the_owner_did(root, "20261011"));
        fs::create_dir_all(root.join("chronicle/20261012/health")).unwrap();
        assert!(!day_holds_nothing_the_owner_did(root, "20261012"));
    }
}
