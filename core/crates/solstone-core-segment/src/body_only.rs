// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Segments holding only body records.
//!
//! A body window holds records of the owner's body, written by code, and owes no
//! thinking. That is decided by what the segment holds and never by its stream
//! name alone: older content in the same stream keeps its behaviour.
//!
//! A Strava tile is body data too: its `workout.json` is another system's
//! account of a workout. What thinking once wrote under its `talents/` is a
//! reading of that body data, so a tile is body-only whatever `talents/` holds.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use solstone_core_format::content::body_records::{BODY_RECORD_STREAMS, BODY_RECORDS_FILE};
use solstone_core_format::content::strava_body::{STRAVA_STREAM, STRAVA_TILE_FILE};
use solstone_core_journal_io::{PathOrDay, Segment, is_incidental_file_name, iter_segments};

use crate::calendar::is_calendar_segment;

const STREAM_MARKER: &str = "stream.json";
const TOMBSTONE: &str = "tombstone.json";
const TALENTS: &str = "talents";
const FACETS: &str = "facets.json";

/// One import's own rows, split out of a window it shares while the trash
/// moves them (`body.<import>.jsonl`). In passing, never content of its own.
#[must_use]
pub fn is_split_rows_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name != BODY_RECORDS_FILE && name.starts_with("body.") && name.ends_with(".jsonl")
    })
}

/// What a segment's tombstone says was removed, from its manifest of top-level
/// names. The tombstone is the only thing that survives a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TombstoneRecord {
    /// The stream's records file (`body.jsonl`, or a Strava tile's
    /// `workout.json`), and only what may sit beside it.
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
    tombstone_record_of(&bytes, BODY_RECORDS_FILE)
}

/// The file that holds a stream's body records: a Strava tile's `workout.json`,
/// or a body window's `body.jsonl`. `None` for a stream that holds none.
fn records_file(stream: &str) -> Option<&'static str> {
    if stream == STRAVA_STREAM {
        Some(STRAVA_TILE_FILE)
    } else if BODY_RECORD_STREAMS.contains(&stream) {
        Some(BODY_RECORDS_FILE)
    } else {
        None
    }
}

fn tombstone_record_of(bytes: &[u8], records_file: &str) -> TombstoneRecord {
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(bytes) else {
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
        if name == OsStr::new(records_file) {
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
    // Every other stream answers before its directory is listed.
    if records_file(stream).is_none() {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut names = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        names.push(entry.file_name());
    }
    is_body_only_listing(
        stream,
        &names,
        || fs::read(dir.join(TOMBSTONE)).ok(),
        || {
            fs::read_dir(dir.join(TALENTS))
                .ok()?
                .map(|entry| entry.ok().map(|entry| entry.file_name()))
                .collect()
        },
    )
}

/// The same decision from a directory's listing, for a reader that lists and
/// reads through its own bound handles. `tombstone` gives `tombstone.json`'s
/// bytes and `talents` lists `talents/`; either gives `None` when it can't be
/// read, which counts as not body-only. A Strava tile's `talents/` is never
/// listed: whatever it holds, the tile is body data.
#[must_use]
pub fn is_body_only_listing(
    stream: &str,
    names: &[std::ffi::OsString],
    tombstone: impl FnOnce() -> Option<Vec<u8>>,
    talents: impl FnOnce() -> Option<Vec<std::ffi::OsString>>,
) -> bool {
    let Some(records_file) = records_file(stream) else {
        return false;
    };
    let mut tombstone = Some(tombstone);
    let mut talents = Some(talents);
    let mut holds_records = false;
    for name in names {
        if name == OsStr::new(TOMBSTONE) {
            let record = tombstone
                .take()
                .and_then(|read| read())
                .map_or(TombstoneRecord::CantSay, |bytes| {
                    tombstone_record_of(&bytes, records_file)
                });
            if record != TombstoneRecord::BodyWindow {
                return false;
            }
            holds_records = true;
        } else if name == OsStr::new(records_file) {
            holds_records = true;
        } else if name == OsStr::new(TALENTS) {
            // A body window's may hold only the facet choice code wrote.
            let allowed = stream == STRAVA_STREAM
                || talents.take().and_then(|list| list()).is_some_and(|names| {
                    names
                        .iter()
                        .all(|name| name == OsStr::new(FACETS) || is_incidental_file_name(name))
                });
            if !allowed {
                return false;
            }
        } else if name != OsStr::new(STREAM_MARKER)
            && !is_incidental_file_name(name)
            && !is_split_rows_name(name)
        {
            return false;
        }
    }
    holds_records
}

/// Whether this directory holds anything a body window wouldn't: older content
/// that new records must not land in, live or deleted. A tombstone settles it
/// whatever else is beside it, such as a sync client's copy: one that shows older
/// content is foreign, and any other means a window the owner deleted stays deleted.
#[must_use]
pub fn holds_foreign_content(dir: &Path) -> bool {
    let tombstone = dir.join(TOMBSTONE);
    if tombstone.symlink_metadata().is_ok() {
        return tombstone_record(&tombstone) == TombstoneRecord::OlderContent;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.into_iter().any(|entry| {
        entry.is_ok_and(|entry| {
            let name = entry.file_name();
            if name == OsStr::new(TALENTS) {
                return !talents_hold_only_facets(&entry.path());
            }
            name != OsStr::new(BODY_RECORDS_FILE)
                && name != OsStr::new(STREAM_MARKER)
                && !is_incidental_file_name(&name)
                && !is_split_rows_name(&name)
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
    fn a_tombstone_settles_it_whatever_turned_up_beside_it() {
        let journal = TempDir::new();
        let root = journal.path();
        for (index, (tombstone, foreign)) in [
            (r#"{"manifest":["a/body.jsonl","a/stream.json"]}"#, false),
            ("not json", false),
            (r#"{"manifest":["a/agents/day.md"]}"#, true),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = tombstoned(root, "20261011", &format!("{index:06}_300"), tombstone);
            fs::write(dir.join("stream (conflicted copy).json"), "{}").unwrap();
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
        // A Strava tile is body-only by its workout, never by a body window's file.
        let strava = window(
            root,
            "20261010",
            "import.strava",
            "120000_300",
            &["workout.json"],
        );
        assert!(is_body_only_dir("import.strava", &strava));
        let strava = window(
            root,
            "20261010",
            "import.strava",
            "120500_300",
            &["body.jsonl"],
        );
        assert!(!is_body_only_dir("import.strava", &strava));
        // A workout in a body window's stream, or any other, isn't one.
        let apple = window(root, "20261010", STREAM, "121000_300", &["workout.json"]);
        assert!(!is_body_only_dir(STREAM, &apple));
        let phone = window(root, "20261010", "phone", "121500_300", &["workout.json"]);
        assert!(!is_body_only_dir("phone", &phone));
        assert!(!is_body_only_dir(STREAM, &root.join("absent")));
    }

    const STRAVA: &str = "import.strava";

    #[test]
    fn a_strava_tile_is_body_only_whatever_its_talents_hold() {
        let journal = TempDir::new();
        let root = journal.path();
        for (index, (files, expected)) in [
            (vec!["workout.json"], true),
            (
                vec!["workout.json", "stream.json", "workout.json.lock"],
                true,
            ),
            (
                vec!["workout.json", ".DS_Store", "._workout.json", "Thumbs.db"],
                true,
            ),
            // An interrupted write's candidate beside the published tile.
            (vec!["workout.json", ".tmp_workout.json.0a1b.tmp"], true),
            (vec!["workout.json", "talents/facets.json"], true),
            (
                vec![
                    "workout.json",
                    "stream.json",
                    "talents/sense.json",
                    "talents/facets.json",
                    "talents/activity.md",
                    "talents/density.json",
                    "talents/entities/detection.json",
                ],
                true,
            ),
            (vec!["stream.json"], false),
            (vec!["stream.json", "talents/sense.json"], false),
            (vec!["workout.json", "notes.md"], false),
            (vec!["workout.json", "audio.jsonl"], false),
            (vec!["workout.json", "body.jsonl"], false),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = window(root, "20261012", STRAVA, &format!("{index:06}_300"), &files);
            assert_eq!(is_body_only_dir(STRAVA, &dir), expected, "{files:?}");
        }
        // The same files in a body window's stream keep that stream's rule.
        let dir = window(
            root,
            "20261012",
            STREAM,
            "200000_300",
            &["body.jsonl", "talents/sense.json"],
        );
        assert!(!is_body_only_dir(STREAM, &dir));
    }

    #[test]
    fn a_strava_tombstone_counts_when_its_manifest_names_only_a_tile() {
        let journal = TempDir::new();
        let root = journal.path();
        for (index, (tombstone, expected)) in [
            (
                r#"{"manifest":[
                    "chronicle/20261013/import.strava/070000_300/workout.json",
                    "chronicle/20261013/import.strava/070000_300/stream.json",
                    "chronicle/20261013/import.strava/070000_300/talents"
                ]}"#,
                true,
            ),
            (r#"{"manifest":["a/workout.json","a/.DS_Store"]}"#, true),
            (r#"{"manifest":["a/workout.json","a/notes.md"]}"#, false),
            (r#"{"manifest":["a/body.jsonl","a/stream.json"]}"#, false),
            (r#"{"manifest":["a/stream.json","a/talents"]}"#, false),
            (r#"{"manifest":[]}"#, false),
            ("not json", false),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = window(root, "20261013", STRAVA, &format!("{index:06}_300"), &[]);
            fs::write(dir.join("tombstone.json"), tombstone).unwrap();
            assert_eq!(is_body_only_dir(STRAVA, &dir), expected, "{tombstone}");
        }
    }

    #[test]
    fn a_day_of_strava_tiles_alone_holds_nothing_the_owner_did() {
        let journal = TempDir::new();
        let root = journal.path();
        window(
            root,
            "20261014",
            STRAVA,
            "070000_300",
            &["workout.json", "talents/sense.json"],
        );
        window(root, "20261014", STRAVA, "070500_300", &["workout.json"]);
        assert!(day_holds_nothing_the_owner_did(root, "20261014"));
        window(root, "20261014", "phone", "090000_300", &["audio.jsonl"]);
        assert!(!day_holds_nothing_the_owner_did(root, "20261014"));
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
