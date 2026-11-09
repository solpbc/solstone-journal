// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use solstone_core_facets::{BodyRecordPlace, read_strava_converted};
use tempfile::TempDir;

use super::*;

const DAY: &str = "20260101";
const SENSE: &str = "exercise_070000_300";
const WORKOUT: u64 = 901;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 9, 0, 0, 0).unwrap()
}

fn journal(facets: &[&str]) -> TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "UTC"}}).to_string(),
    )
    .unwrap();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.strava-zone.json"),
        json!({"zone": "UTC"}).to_string(),
    )
    .unwrap();
    for facet in facets {
        solstone_core_facets::create_facet(root, facet, facet, "", "", "", None).unwrap();
    }
    journal
}

fn tile_dir(root: &Path, day: &str, key: &str) -> PathBuf {
    root.join("chronicle")
        .join(day)
        .join(STRAVA_STREAM)
        .join(key)
}

/// One piece of a 600 s workout.
fn tile(root: &Path, day: &str, key: &str, id: u64, index: u64, start: &str, facet: Option<&str>) {
    tile_of(root, day, key, id, index, start, 600, facet);
}

#[allow(clippy::too_many_arguments)]
fn tile_of(
    root: &Path,
    day: &str,
    key: &str,
    id: u64,
    index: u64,
    start: &str,
    elapsed: u64,
    facet: Option<&str>,
) {
    let tile = json!({"schema": "solstone.import.strava.tile.v1", "import_id": "i",
        "activity_id": id, "zone": "UTC",
        "tile": {"index": index, "count": 2, "start": start, "end": start, "seconds": 300},
        "workout": {"name": "Morning Run", "type": "Run", "start": start,
            "elapsed_seconds": elapsed, "distance_m": 5000.0}});
    let dir = tile_dir(root, day, key);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(dir.join("workout.json"), format!("{tile}\n")).unwrap();
    if let Some(facet) = facet {
        fs::write(
            dir.join("talents/facets.json"),
            json!([{ "facet": facet }]).to_string(),
        )
        .unwrap();
    }
}

/// The usual workout: two tiles, both filed under `facet`.
fn workout(root: &Path, facet: Option<&str>) {
    // Two pieces of one activity.
    const START: &str = "2026-01-01T07:00:00+00:00";
    tile(root, DAY, "070000_300", WORKOUT, 0, START, facet);
    tile(root, DAY, "070500_300", WORKOUT, 1, START, facet);
}

/// How many records an owner sees for the day, in these facets: every reader's
/// rule, whichever kind of record shows.
fn visible(root: &Path, facets: &[&str]) -> usize {
    facets
        .iter()
        .map(|facet| {
            let rows = lines(root, facet, DAY);
            let rows = rows
                .iter()
                .filter_map(Value::as_object)
                .cloned()
                .collect::<Vec<_>>();
            let gone = solstone_core_facets::superseded_record_ids(root, facet, DAY, &rows);
            rows.iter()
                .filter(|row| !row.get("hidden").is_some_and(activity_value_truthy))
                .filter(|row| !gone.contains(row["id"].as_str().unwrap_or_default()))
                .count()
        })
        .sum()
}

fn sense_lines_in_file(root: &Path, facet: &str) -> usize {
    lines(root, facet, DAY)
        .iter()
        .filter(|row| row.get("stream").is_some())
        .count()
}

fn file(root: &Path, facet: &str, day: &str) -> PathBuf {
    root.join(format!("facets/{facet}/activities/{day}.jsonl"))
}

fn add_line(root: &Path, facet: &str, day: &str, record: Value) {
    let path = file(root, facet, day);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(path, text).unwrap();
}

fn sense(extra: Value) -> Value {
    let mut record = json!({"id": SENSE, "activity": "exercise", "segments": ["070000_300", "070500_300"],
        "level_avg": 0.5, "description": "a run", "active_entities": [], "created_at": 1,
        "stream": "import.strava"});
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    record
}

fn title_edit(at: &str, title: &str) -> Value {
    json!({"timestamp": at, "actor": "owner:update", "fields": ["title"], "note": "",
        "set": {"title": title}})
}

fn hide_edit(at: &str) -> Value {
    json!({"timestamp": at, "actor": "owner:hide", "fields": ["hidden"], "note": "muted"})
}

fn lines(root: &Path, facet: &str, day: &str) -> Vec<Value> {
    fs::read_to_string(file(root, facet, day))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn run(root: &Path) -> ConversionCounts {
    sweep(root, now())
}

fn facet_of(root: &Path, day: &str, key: &str) -> Option<String> {
    window_facet(&tile_dir(root, day, key))
}

/// What a finished conversion leaves that an owner or thinking can see.
fn canon(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, into);
            } else if !path.to_string_lossy().ends_with(".lock") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                into.insert(rel, fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut seen = BTreeMap::new();
    walk(root, &root.join("facets"), &mut seen);
    walk(root, &root.join("chronicle"), &mut seen);
    seen.retain(|path, _| !path.ends_with("facet.json"));
    // A facet's id is random per journal.
    for text in seen.values_mut() {
        *text = text
            .lines()
            .map(|line| match serde_json::from_str::<Value>(line) {
                Ok(Value::Object(mut record)) => {
                    record.remove("destination_id");
                    format!("{}\n", Value::Object(record))
                }
                _ => format!("{line}\n"),
            })
            .collect();
    }
    let mut listed = read_strava_converted(root)
        .unwrap()
        .into_iter()
        .map(|line| format!("{line:?}"))
        .collect::<Vec<_>>();
    listed.sort();
    seen.insert("listed".to_owned(), listed.join("\n"));
    seen
}

#[test]
fn one_untouched_record_becomes_one_code_record_and_nothing_else_stays() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    let counts = run(root);
    assert_eq!(counts.converted, 1, "{counts:?}");
    let records = lines(root, "a", DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["source"], "body");
    assert_eq!(records[0]["hidden"], false);
    assert!(records[0].get("description").is_none());
    assert_eq!(
        read_strava_converted(root).unwrap().len(),
        1,
        "the record is listed"
    );
}

#[test]
fn the_same_id_in_two_facets_is_one_record_where_it_was_visible() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"hidden": true, "edits": [hide_edit("2026-01-03T00:00:00Z")]})),
    );
    add_line(
        root,
        "b",
        DAY,
        sense(json!({"title": "Run", "edits": [title_edit("2026-01-02T00:00:00Z", "Run")]})),
    );
    assert_eq!(run(root).converted, 1);
    assert!(lines(root, "a", DAY).is_empty());
    let records = lines(root, "b", DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["title"], "Run");
    assert_eq!(records[0]["hidden"], false);
    assert_eq!(facet_of(root, DAY, "070000_300").as_deref(), Some("b"));
    assert_eq!(facet_of(root, DAY, "070500_300").as_deref(), Some("b"));
    assert_eq!(read_strava_converted(root).unwrap().len(), 2);
}

#[test]
fn hidden_in_both_facets_is_hidden_and_reads_as_the_owners_hide() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    for facet in ["a", "b"] {
        add_line(
            root,
            facet,
            DAY,
            sense(json!({"hidden": true, "edits": [hide_edit("2026-01-03T00:00:00Z")]})),
        );
    }
    run(root);
    let records = lines(root, "a", DAY);
    assert_eq!(records[0]["hidden"], true);
    let last_hide = records[0]["edits"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|edit| {
            edit["fields"]
                .as_array()
                .unwrap()
                .contains(&json!("hidden"))
        })
        .unwrap();
    assert!(last_hide["actor"].as_str().unwrap().starts_with("owner:"));
}

#[test]
fn a_workout_crossing_midnight_is_one_record_on_its_start_day() {
    let journal = journal(&["a"]);
    let root = journal.path();
    const START: &str = "2026-01-01T23:55:00+00:00";
    tile(root, DAY, "235500_300", WORKOUT, 0, START, Some("a"));
    tile(root, "20260102", "000000_300", WORKOUT, 1, START, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"id": "exercise_235500_300", "segments": ["235500_300"]})),
    );
    add_line(
        root,
        "a",
        "20260102",
        sense(json!({"id": "exercise_000000_300", "segments": ["000000_300"]})),
    );
    assert_eq!(run(root).converted, 1);
    assert_eq!(lines(root, "a", DAY).len(), 1);
    assert!(lines(root, "a", "20260102").is_empty());
}

#[test]
fn a_record_spanning_two_workouts_gives_fields_to_its_start_and_its_hide_to_both() {
    let journal = journal(&["a"]);
    let root = journal.path();
    const FIRST: &str = "2026-01-01T07:00:00+00:00";
    const SECOND: &str = "2026-01-01T07:10:00+00:00";
    tile_of(root, DAY, "070000_300", 901, 0, FIRST, 300, Some("a"));
    tile_of(root, DAY, "071000_300", 902, 0, SECOND, 300, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(
            json!({"segments": ["070000_300", "071000_300"], "title": "Both",
            "hidden": true, "edits": [title_edit("2026-01-02T00:00:00Z", "Both"),
                hide_edit("2026-01-03T00:00:00Z")]}),
        ),
    );
    assert_eq!(run(root).converted, 1);
    let records = lines(root, "a", DAY);
    assert_eq!(records.len(), 2);
    let titled = records
        .iter()
        .filter(|record| record.get("title").is_some())
        .count();
    assert_eq!(titled, 1);
    assert!(records.iter().all(|record| record["hidden"] == true));
    let first_key = account_key(STRAVA_STREAM, "901");
    let first = records
        .iter()
        .find(|record| record["account"]["key"] == first_key)
        .unwrap();
    assert_eq!(first["title"], "Both");
}

#[test]
fn a_deleted_first_tile_files_on_the_first_live_tiles_day() {
    let journal = journal(&["a"]);
    let root = journal.path();
    const START: &str = "2026-01-01T23:55:00+00:00";
    tile(root, "20260102", "000000_300", WORKOUT, 1, START, Some("a"));
    add_line(
        root,
        "a",
        "20260102",
        sense(json!({"id": "exercise_000000_300", "segments": ["000000_300"]})),
    );
    assert_eq!(run(root).converted, 1);
    assert_eq!(lines(root, "a", "20260102")[0]["source"], "body");
}

#[test]
fn a_tile_placed_at_the_misread_time_is_still_matched_and_a_live_match_wins() {
    let journal = journal(&["a"]);
    let root = journal.path();
    // The zone was Denver, and an earlier version read the UTC start as wall time.
    fs::write(
        root.join("imports/.strava-zone.json"),
        json!({"zone": "America/Denver"}).to_string(),
    )
    .unwrap();
    const START: &str = "2026-01-02T01:00:00-07:00";
    tile_of(
        root,
        "20260102",
        "010000_300",
        WORKOUT,
        0,
        START,
        300,
        Some("a"),
    );
    // Its UTC start is 08:00, read as Denver wall time.
    add_line(
        root,
        "a",
        "20260102",
        sense(json!({"id": "exercise_080000_300", "segments": ["080000_300"]})),
    );
    assert_eq!(run(root).converted, 1, "the misread key names the workout");
    assert_eq!(lines(root, "a", "20260102")[0]["source"], "body");
}

#[test]
fn a_record_naming_no_live_workout_stays_and_the_marker_ends_the_walk() {
    let journal = journal(&["a"]);
    let root = journal.path();
    add_line(root, "a", DAY, sense(json!({})));
    let counts = run(root);
    assert_eq!((counts.converted, counts.no_workout), (0, 1));
    assert_eq!(lines(root, "a", DAY).len(), 1);
    assert!(root.join(MARKER).exists());

    // A later import brings a workout back; the marker skips the walk until a
    // conversion or merge clears it.
    workout(root, Some("a"));
    assert_eq!(run(root).converted, 0);
    mark_conversion_needed(root).unwrap();
    assert_eq!(run(root).converted, 1);
}

#[test]
fn an_unreadable_day_file_waits_counted_and_the_rest_converts() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    fs::create_dir_all(file(root, "a", "20260105")).unwrap();
    let counts = run(root);
    assert_eq!((counts.converted, counts.waiting), (1, 1), "{counts:?}");
    assert!(
        !root.join(MARKER).exists(),
        "a waiting file writes no marker"
    );
    assert_eq!(lines(root, "a", DAY)[0]["source"], "body");
}

#[test]
fn no_declared_facet_waits_and_the_line_stays() {
    let journal = journal(&["b"]);
    let root = journal.path();
    workout(root, None);
    add_line(root, "gone", DAY, sense(json!({})));
    let counts = run(root);
    assert_eq!((counts.converted, counts.waiting), (0, 1), "{counts:?}");
    assert_eq!(lines(root, "gone", DAY).len(), 1);
}

#[test]
fn a_held_strava_lock_is_skipped_and_writes_no_marker() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    let _held = hold_lock(root.join(STRAVA_LOCK), LockOptions::default()).unwrap();
    let counts = run(root);
    assert_eq!(
        (counts.converted, counts.skipped_locked),
        (0, 1),
        "{counts:?}"
    );
    assert!(!root.join(MARKER).exists());
    assert_eq!(lines(root, "a", DAY).len(), 1);
}

#[test]
fn an_earlier_code_record_keeps_its_facet_and_stays_visible_when_lines_fold_in() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    let key = account_key(STRAVA_STREAM, &WORKOUT.to_string());
    add_line(
        root,
        "a",
        DAY,
        json!({"id": record_id(&key), "source": "body", "activity": "workout",
            "account": {"key": key, "start": "x", "end": "y", "recorder": "r"}}),
    );
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"hidden": true, "edits": [hide_edit("2026-01-03T00:00:00Z")]})),
    );
    add_line(root, "b", DAY, sense(json!({})));
    assert_eq!(run(root).converted, 1);
    assert!(lines(root, "b", DAY).is_empty());
    let records = lines(root, "a", DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(facet_of(root, DAY, "070000_300").as_deref(), Some("a"));
    // The owner's only listing doesn't vanish because a duplicate was hidden.
    assert_eq!(records[0]["hidden"], json!(false));

    // A later return, from a restored line in another facet, leaves it there.
    add_line(root, "b", DAY, sense(json!({})));
    mark_conversion_needed(root).unwrap();
    assert_eq!(run(root).converted, 1);
    assert!(lines(root, "b", DAY).is_empty());
    assert_eq!(lines(root, "a", DAY).len(), 1);
    assert_eq!(facet_of(root, DAY, "070000_300").as_deref(), Some("a"));
}

#[test]
fn every_line_hidden_and_a_visible_earlier_record_stays_visible() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    let key = account_key(STRAVA_STREAM, &WORKOUT.to_string());
    add_line(
        root,
        "a",
        DAY,
        json!({"id": record_id(&key), "source": "body", "activity": "workout",
            "account": {"key": key, "start": "x", "end": "y", "recorder": "r"}}),
    );
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"hidden": true, "edits": [hide_edit("2026-01-03T00:00:00Z")]})),
    );
    run(root);
    assert_eq!(lines(root, "a", DAY)[0]["hidden"], json!(false));
}

#[test]
fn a_hidden_earlier_record_with_every_line_hidden_stays_hidden() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    let key = account_key(STRAVA_STREAM, &WORKOUT.to_string());
    add_line(
        root,
        "a",
        DAY,
        json!({"id": record_id(&key), "source": "body", "activity": "workout", "hidden": true,
            "account": {"key": key, "start": "x", "end": "y", "recorder": "r"}}),
    );
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"hidden": true, "edits": [hide_edit("2026-01-03T00:00:00Z")]})),
    );
    run(root);
    assert_eq!(lines(root, "a", DAY)[0]["hidden"], json!(true));
}

#[test]
fn one_line_that_wont_go_leaves_the_rest_removed_and_is_counted() {
    let journal = journal(&["z"]);
    let root = journal.path();
    workout(root, Some("z"));
    add_line(root, "z", DAY, sense(json!({})));
    // A folder with no declaration: the store refuses to change its files. It
    // comes first, so the rest are removed after it fails.
    add_line(root, "gone", DAY, sense(json!({})));
    let counts = run(root);
    assert_eq!((counts.waiting, counts.lines_left), (1, 1), "{counts:?}");
    assert_eq!(sense_lines_in_file(root, "z"), 0);
    assert_eq!(sense_lines_in_file(root, "gone"), 1);
    assert_eq!(visible(root, &["z", "gone"]), 1);
    assert!(!root.join(MARKER).exists());
}

#[test]
fn a_folder_with_no_declaration_is_never_where_a_workout_is_filed() {
    let journal = journal(&["z"]);
    let root = journal.path();
    workout(root, None);
    add_line(root, "gone", DAY, sense(json!({})));
    add_line(root, "z", DAY, sense(json!({})));
    run(root);
    let filed = lines(root, "z", DAY);
    assert_eq!(filed.len(), 1, "{filed:?}");
    assert_eq!(filed[0]["source"], json!("body"));
}

#[test]
fn a_workout_deleted_after_the_walk_gains_no_file_and_nothing_is_folded() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "b", DAY, sense(json!({})));
    fn delete_both(root: &Path) {
        for key in ["070000_300", "070500_300"] {
            let dir = tile_dir(root, DAY, key);
            fs::write(dir.join("tombstone.json"), b"{}").unwrap();
            fs::remove_file(dir.join("talents/facets.json")).unwrap();
        }
    }
    BEFORE_LOCK.set(Some(delete_both));
    let counts = run(root);
    assert_eq!((counts.converted, counts.waiting), (0, 1), "{counts:?}");
    for key in ["070000_300", "070500_300"] {
        assert!(
            !tile_dir(root, DAY, key)
                .join("talents/facets.json")
                .exists()
        );
    }
    assert_eq!(sense_lines_in_file(root, "b"), 1);
    assert!(lines(root, "b", DAY).len() == 1 && lines(root, "a", DAY).is_empty());
    // Nothing says it was converted, or thinking would treat the line as settled.
    assert!(read_strava_converted(root).unwrap().is_empty());
}

#[test]
fn a_tile_deleted_after_the_walk_is_left_alone_and_the_rest_files() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "b", DAY, sense(json!({})));
    fn delete_one(root: &Path) {
        let dir = tile_dir(root, DAY, "070500_300");
        fs::write(dir.join("tombstone.json"), b"{}").unwrap();
        fs::remove_file(dir.join("talents/facets.json")).unwrap();
    }
    BEFORE_LOCK.set(Some(delete_one));
    assert_eq!(run(root).converted, 1);
    assert!(
        !tile_dir(root, DAY, "070500_300")
            .join("talents/facets.json")
            .exists()
    );
    assert_eq!(facet_of(root, DAY, "070000_300").as_deref(), Some("b"));
}

#[test]
fn an_unreadable_workout_waits_counted_and_writes_no_marker() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    fs::write(
        tile_dir(root, DAY, "070500_300").join("workout.json"),
        b"not json",
    )
    .unwrap();
    let counts = run(root);
    assert_eq!(counts.waiting, 1, "{counts:?}");
    assert!(!root.join(MARKER).exists());
}

#[test]
fn a_deleted_workout_is_not_counted_as_waiting() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    fs::write(
        tile_dir(root, DAY, "070500_300").join("tombstone.json"),
        b"{}",
    )
    .unwrap();
    assert_eq!(run(root).waiting, 0);
}

/// A line folded into its record that couldn't be removed, under a folder with
/// no declaration, and then the owner deleted the workout.
fn stuck_then_deleted(root: &Path) {
    workout(root, Some("z"));
    add_line(root, "z", DAY, sense(json!({})));
    add_line(root, "gone", DAY, sense(json!({})));
    assert_eq!(run(root).lines_left, 1);
    for key in ["070000_300", "070500_300"] {
        let dir = tile_dir(root, DAY, key);
        fs::write(dir.join("tombstone.json"), b"{}").unwrap();
        fs::remove_file(dir.join("workout.json")).unwrap();
    }
}

#[test]
fn a_stuck_line_whose_workout_is_then_deleted_holds_the_marker_until_it_goes() {
    let journal = journal(&["z"]);
    let root = journal.path();
    stuck_then_deleted(root);
    let counts = run(root);
    assert_eq!((counts.waiting, counts.lines_left), (1, 1), "{counts:?}");
    assert!(!root.join(MARKER).exists());
    assert_eq!(visible(root, &["z", "gone"]), 1);

    // The folder can be written now: the conversion's last step goes through.
    solstone_core_facets::create_facet(root, "gone", "gone", "", "", "", None).unwrap();
    let counts = run(root);
    assert_eq!((counts.waiting, counts.lines_left), (0, 0), "{counts:?}");
    assert_eq!(sense_lines_in_file(root, "gone"), 0);
    assert!(root.join(MARKER).exists());
    assert_eq!(visible(root, &["z", "gone"]), 1);
}

#[test]
fn a_stuck_line_changed_after_its_workout_is_deleted_stays_hidden_and_waits() {
    let journal = journal(&["z"]);
    let root = journal.path();
    stuck_then_deleted(root);
    solstone_core_facets::create_facet(root, "gone", "gone", "", "", "", None).unwrap();
    let path = file(root, "gone", DAY);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("a run", "another run")).unwrap();
    let counts = run(root);
    assert_eq!((counts.waiting, counts.lines_left), (1, 1), "{counts:?}");
    assert_eq!(sense_lines_in_file(root, "gone"), 1);
    assert!(!root.join(MARKER).exists());
    assert_eq!(visible(root, &["z", "gone"]), 1);
}

#[test]
fn an_import_retries_a_folded_line_whose_workout_is_gone() {
    let journal = journal(&["z"]);
    let root = journal.path();
    stuck_then_deleted(root);
    solstone_core_facets::create_facet(root, "gone", "gone", "", "", "", None).unwrap();
    let held = hold_lock(root.join(STRAVA_LOCK), LockOptions::default()).unwrap();
    let scope = ConversionScope {
        chosen: None,
        publishes_tiles: false,
    };
    let counts = convert_held(root, &held, scope, now());
    assert_eq!((counts.waiting, counts.lines_left), (0, 0), "{counts:?}");
    assert_eq!(sense_lines_in_file(root, "gone"), 0);
}

#[test]
fn only_a_record_now_in_the_facet_counts_as_moved() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    run(root);
    let counted = vec![(WORKOUT.to_string(), "a".to_owned())];
    assert_eq!(listed_moved_now(root, &counted, "b"), None);
    assert_eq!(
        listed_moved_now(root, &counted, "a"),
        Some((1, vec!["a".to_owned()]))
    );
}

#[test]
fn a_line_no_workout_names_and_no_record_holds_doesnt_hold_the_marker_back() {
    let journal = journal(&["z"]);
    let root = journal.path();
    add_line(root, "z", DAY, sense(json!({})));
    let counts = run(root);
    assert_eq!(counts.no_workout, 1, "{counts:?}");
    assert_eq!(sense_lines_in_file(root, "z"), 1);
    assert!(root.join(MARKER).exists());
}

#[test]
fn no_marker_is_written_while_a_merge_is_running() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    let merging = hold_lock(
        root.join("health/locks/archive-merge"),
        LockOptions::default(),
    )
    .unwrap();
    assert_eq!(run(root).converted, 1);
    assert!(!root.join(MARKER).exists());
    drop(merging);
    run(root);
    assert!(root.join(MARKER).exists());
}

#[test]
fn a_finished_pass_says_so_and_the_next_finds_nothing_to_do() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    assert_eq!(run(root).converted, 1);
    assert!(root.join(MARKER).exists());
}

#[test]
fn a_restored_line_keeps_the_stored_decisions_and_later_owner_changes() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    let original =
        sense(json!({"title": "Run", "edits": [title_edit("2026-01-02T00:00:00Z", "Run")]}));
    add_line(root, "a", DAY, original.clone());
    run(root);
    let code = lines(root, "a", DAY)[0].clone();
    let id = code["id"].as_str().unwrap().to_owned();
    // The owner hides the code record, then a backup brings the line back.
    solstone_core_facets::write_body_record_with(
        root,
        &BodyRecordPlace {
            day: DAY.to_owned(),
            facet: "a".to_owned(),
        },
        &id,
        |current| {
            let mut record = current.unwrap();
            record.insert("hidden".to_owned(), json!(true));
            let mut edits = record["edits"].as_array().unwrap().clone();
            edits.push(hide_edit("2026-02-01T00:00:00Z"));
            record.insert("edits".to_owned(), json!(edits));
            record
        },
    )
    .unwrap();
    add_line(root, "a", DAY, original);
    mark_conversion_needed(root).unwrap();
    assert_eq!(run(root).converted, 1);
    let records = lines(root, "a", DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["title"], "Run");
    assert_eq!(records[0]["hidden"], true, "the owner's own hide stands");
}

#[test]
fn outputs_under_a_converted_id_and_the_open_state_are_removed_and_queued() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    let output = root.join(format!("facets/a/activities/{DAY}/{SENSE}/story.md"));
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(&output, "model words").unwrap();
    let state = root.join(ACTIVITY_STATE);
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, "{}").unwrap();
    run(root);
    assert!(!output.exists());
    assert!(!state.exists());
    let queued = fs::read_dir(root.join("indexer/todo"))
        .unwrap()
        .flatten()
        .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
        .any(|entry| entry.contains("story.md"));
    assert!(queued, "search is told the output is gone");
}

#[test]
fn a_sweep_queues_tiles_it_moves_and_leaves_the_refile_list_empty() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"hidden": true, "edits": [hide_edit("2026-01-03T00:00:00Z")]})),
    );
    add_line(root, "b", DAY, sense(json!({})));
    run(root);
    assert!(!root.join("imports/.strava-refile.json").exists());
    let queued = fs::read_dir(root.join("indexer/todo"))
        .unwrap()
        .flatten()
        .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
        .filter(|entry| entry.contains("workout.json"))
        .count();
    assert_eq!(queued, 2);
}

/// A workout in two facets with a title and a hide, the shape every stop must
/// resume to the same end.
fn stop_fixture(root: &Path) {
    // One line hidden and one not, so the workout starts listed once.
    workout(root, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"hidden": true, "edits": [hide_edit("2026-01-03T00:00:00Z")]})),
    );
    add_line(
        root,
        "b",
        DAY,
        sense(json!({"title": "Run", "edits": [title_edit("2026-01-02T00:00:00Z", "Run")]})),
    );
}

#[test]
fn every_stop_resumes_to_what_an_uninterrupted_run_leaves() {
    let whole = journal(&["a", "b"]);
    stop_fixture(whole.path());
    WRITES_SEEN.set(0);
    STOP_AFTER.set(None);
    assert_eq!(run(whole.path()).converted, 1);
    let total = WRITES_SEEN.get();
    let expected = canon(whole.path());
    assert!(total >= 6, "{total} stop points");

    for stop in 1..=total {
        let journal = journal(&["a", "b"]);
        let root = journal.path();
        stop_fixture(root);
        WRITES_SEEN.set(0);
        STOP_AFTER.set(Some(stop));
        let stopped = run(root);
        STOP_AFTER.set(None);
        assert_eq!(stopped.waiting, 1, "stop {stop} stopped it");
        // Whatever stopped, the workout is listed exactly once.
        assert_eq!(visible(root, &["a", "b"]), 1, "stop {stop}");

        // After the last write nothing is left to find, so a resume may do nothing.
        let resumed = run(root);
        assert_eq!(resumed.waiting, 0, "stop {stop}: {resumed:?}");
        assert_eq!(canon(root), expected, "stop {stop}");
    }
}

fn spanning_fixture(root: &Path) {
    const FIRST: &str = "2026-01-01T07:00:00+00:00";
    const SECOND: &str = "2026-01-01T07:10:00+00:00";
    tile_of(root, DAY, "070000_300", 901, 0, FIRST, 300, Some("a"));
    tile_of(root, DAY, "071000_300", 902, 0, SECOND, 300, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"segments": ["070000_300", "071000_300"]})),
    );
}

#[test]
fn every_stop_on_a_line_spanning_two_workouts_resumes_and_lists_neither_twice() {
    let whole = journal(&["a"]);
    spanning_fixture(whole.path());
    WRITES_SEEN.set(0);
    STOP_AFTER.set(None);
    assert_eq!(run(whole.path()).converted, 1);
    let total = WRITES_SEEN.get();
    let expected = canon(whole.path());
    assert_eq!(visible(whole.path(), &["a"]), 2);

    let mut seen_one = false;
    for stop in 1..=total {
        let journal = journal(&["a"]);
        let root = journal.path();
        spanning_fixture(root);
        WRITES_SEEN.set(0);
        STOP_AFTER.set(Some(stop));
        let stopped = run(root);
        STOP_AFTER.set(None);
        assert_eq!(stopped.waiting, 1, "stop {stop} stopped it");
        // The shared line stands for both until the first record replaces it.
        // After that the second workout is listed nowhere until the next pass.
        let shown = visible(root, &["a"]);
        assert!((1..=2).contains(&shown), "stop {stop}: {shown}");
        seen_one |= shown == 1;

        let resumed = run(root);
        assert_eq!(resumed.waiting, 0, "stop {stop}: {resumed:?}");
        assert_eq!(canon(root), expected, "stop {stop}");
        assert_eq!(visible(root, &["a"]), 2, "stop {stop}");
    }
    assert!(seen_one);
}

#[test]
fn an_owner_edit_on_a_line_between_steps_is_kept() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    WRITES_SEEN.set(0);
    STOP_AFTER.set(Some(2));
    assert_eq!(run(root).waiting, 1);
    STOP_AFTER.set(None);
    // The line is there beside the code record: the owner edits it.
    let path = file(root, "a", DAY);
    let edited = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut value: Value = serde_json::from_str(line).unwrap();
            if value.get("stream").is_some() {
                value["title"] = json!("Mine");
                value["edits"] = json!([title_edit("2026-11-09T05:00:00Z", "Mine")]);
            }
            format!("{value}\n")
        })
        .collect::<String>();
    fs::write(&path, edited).unwrap();
    assert_eq!(run(root).converted, 1);
    let records = lines(root, "a", DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["title"], "Mine");
}

#[test]
fn an_open_move_is_finished_by_the_start_and_an_open_apple_move_with_the_marker_too() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    let key = "import.apple_health\u{1f}w1";
    add_line(
        root,
        "a",
        DAY,
        json!({"id": "body_x", "source": "body", "activity": "workout",
            "account": {"key": key, "start": "x", "end": "y", "recorder": "r"}}),
    );
    let copy = lines(root, "a", DAY).remove(0);
    fs::write(root.join(MARKER), b"").unwrap();
    fs::write(
        root.join("imports/.body-moves.jsonl"),
        format!(
            "{}\n",
            json!({"move": "m1", "key": key, "record": "body_x",
                "from": {"day": DAY, "facet": "a"}, "to": {"day": DAY, "facet": "b"},
                "copy": copy})
        ),
    )
    .unwrap();
    run(root);
    assert!(lines(root, "a", DAY).is_empty());
    assert_eq!(lines(root, "b", DAY).len(), 1);
    assert!(
        solstone_core_facets::open_body_moves(root)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_facet_choice_counts_only_listed_workouts_that_would_move() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    let download = BTreeSet::from([WORKOUT.to_string()]);
    assert_eq!(
        listed_moves(root, &download, "b"),
        Some((1, vec!["a".to_owned()]))
    );
    assert_eq!(
        listed_moves(root, &download, "a"),
        None,
        "it's there already"
    );
    assert_eq!(
        listed_moves(root, &BTreeSet::new(), "b"),
        None,
        "not in the download"
    );
    run(root);
    assert_eq!(
        listed_moves(root, &download, "b"),
        Some((1, vec!["a".to_owned()])),
        "a code record counts the same"
    );
}

#[test]
fn a_misplaced_workout_placed_again_moves_its_record_with_its_title_and_hide() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(json!({"title": "Run", "hidden": true, "edits": [
            title_edit("2026-01-02T00:00:00Z", "Run"), hide_edit("2026-01-03T00:00:00Z")]})),
    );
    run(root);
    assert_eq!(lines(root, "a", DAY).len(), 1);
    // A re-import put the workout on the next day.
    fs::remove_dir_all(root.join("chronicle").join(DAY)).unwrap();
    const START: &str = "2026-01-02T07:00:00+00:00";
    tile(root, "20260102", "070000_300", WORKOUT, 0, START, Some("a"));
    let live = LiveWorkouts::from([(
        WORKOUT,
        (
            Some(("20260102".to_owned(), "070000_300".to_owned())),
            vec![crate::strava_filing::LiveTile {
                day: "20260102".to_owned(),
                segment: "070000_300".to_owned(),
                index: 0,
            }],
        ),
    )]);
    // The record's old day is where the misread start put it.
    let days = BTreeSet::from([DAY.to_owned(), "20260102".to_owned()]);
    assert!(
        find_code_record(root, &days, &account_key(STRAVA_STREAM, "901"))
            .unwrap()
            .is_some()
    );
    let held_back = relocate_with_days(root, &live, None, &days);
    assert!(held_back.is_empty());
    assert!(lines(root, "a", DAY).is_empty());
    let moved = lines(root, "a", "20260102");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0]["title"], "Run");
    assert_eq!(moved[0]["hidden"], true);
}

fn code_record(root: &Path, facet: &str, day: &str) -> Value {
    lines(root, facet, day)
        .into_iter()
        .find(|record| record["source"] == "body")
        .unwrap()
}

fn remove_edit(root: &Path, facet: &str, record: &Value, position: usize) -> Value {
    let edit = &record["edits"][position];
    let updated = solstone_core_facets::remove_activity_edit(
        root,
        facet,
        DAY,
        record["id"].as_str().unwrap(),
        position,
        edit["timestamp"].as_str().unwrap(),
        edit["actor"].as_str().unwrap(),
        "2026-12-01T00:00:00Z",
    )
    .unwrap()
    .unwrap();
    Value::Object(updated)
}

#[test]
fn removing_a_carried_edit_empties_the_field_and_undo_lands_on_what_precedence_gives() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(
            json!({"title": "Primary", "edits": [title_edit("2026-01-02T00:00:00Z", "Primary")]}),
        ),
    );
    add_line(
        root,
        "b",
        DAY,
        sense(json!({"title": "Later", "edits": [title_edit("2026-05-01T00:00:00Z", "Later")]})),
    );
    run(root);
    let record = code_record(root, "a", DAY);
    assert_eq!(record["title"], "Primary");
    let primary_at = record["edits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|edit| edit["set"]["title"] == "Primary")
        .unwrap();
    let undone = remove_edit(root, "a", &record, primary_at);
    assert_eq!(undone["title"], "Later", "the other line's value shows");
    let later_at = undone["edits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|edit| edit["set"]["title"] == "Later")
        .unwrap();
    let emptied = remove_edit(root, "a", &undone, later_at);
    // No value of the owner's is left; the title falls back as for any record.
    assert!(
        !solstone_core_format::content::activity_field_held(emptied.as_object().unwrap(), "title"),
        "{emptied}"
    );
    assert_ne!(emptied["title"], "Later");
    assert_ne!(emptied["title"], "Primary");
}

#[test]
fn the_owners_own_title_added_and_removed_lands_back_on_the_primarys() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(
        root,
        "a",
        DAY,
        sense(
            json!({"title": "Primary", "edits": [title_edit("2026-01-02T00:00:00Z", "Primary")]}),
        ),
    );
    run(root);
    let id = code_record(root, "a", DAY)["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut patch = serde_json::Map::new();
    patch.insert("title".to_owned(), json!("Mine"));
    let mine = solstone_core_facets::update_activity_record(
        root,
        "a",
        DAY,
        &id,
        &patch,
        "owner:update",
        "",
        "2026-11-20T00:00:00Z",
    )
    .unwrap()
    .unwrap();
    assert_eq!(mine["title"], "Mine");
    let record = code_record(root, "a", DAY);
    let at = record["edits"].as_array().unwrap().len() - 1;
    let back = remove_edit(root, "a", &record, at);
    assert_eq!(back["title"], "Primary");
}

#[test]
fn a_facet_the_owner_chose_stopped_after_the_record_stays_there_at_the_next_start() {
    let journal = journal(&["a", "chosen"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    let held = hold_lock(root.join(STRAVA_LOCK), LockOptions::default()).unwrap();
    let download = BTreeSet::from([WORKOUT.to_string()]);
    WRITES_SEEN.set(0);
    STOP_AFTER.set(Some(2));
    let stopped = convert_held(
        root,
        &held,
        ConversionScope {
            chosen: Some((&download, "chosen")),
            publishes_tiles: false,
        },
        now(),
    );
    STOP_AFTER.set(None);
    drop(held);
    assert_eq!(stopped.waiting, 1);
    assert_eq!(lines(root, "chosen", DAY).len(), 1);

    assert_eq!(run(root).converted, 1);
    assert_eq!(lines(root, "chosen", DAY).len(), 1);
    assert!(lines(root, "a", DAY).is_empty());
    assert_eq!(facet_of(root, DAY, "070000_300").as_deref(), Some("chosen"));
}

#[test]
fn a_stop_before_any_record_leaves_only_what_was_there() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    WRITES_SEEN.set(0);
    STOP_AFTER.set(Some(1));
    assert_eq!(run(root).waiting, 1);
    STOP_AFTER.set(None);
    let records = lines(root, "a", DAY);
    assert_eq!(records.len(), 1);
    assert!(records[0].get("stream").is_some(), "only the old line");
    assert_eq!(
        read_strava_converted(root).unwrap().len(),
        1,
        "listed first"
    );
}

#[test]
fn a_conversion_clears_a_marker_so_a_start_after_its_stop_finishes_it() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    fs::write(root.join(MARKER), b"").unwrap();
    let held = hold_lock(root.join(STRAVA_LOCK), LockOptions::default()).unwrap();
    WRITES_SEEN.set(0);
    STOP_AFTER.set(Some(2));
    let _ = convert_held(
        root,
        &held,
        ConversionScope {
            chosen: None,
            publishes_tiles: false,
        },
        now(),
    );
    STOP_AFTER.set(None);
    drop(held);
    assert!(!root.join(MARKER).exists());
    assert_eq!(run(root).converted, 1);
    assert_eq!(lines(root, "a", DAY).len(), 1);
}

#[test]
fn a_move_that_cant_finish_is_counted_and_finished_at_the_next_start() {
    let journal = journal(&["a", "b"]);
    let root = journal.path();
    let key = "import.apple_health\u{1f}w1";
    add_line(
        root,
        "a",
        DAY,
        json!({"id": "body_x", "source": "body", "activity": "workout",
            "account": {"key": key, "start": "x", "end": "y", "recorder": "r"}}),
    );
    let copy = lines(root, "a", DAY).remove(0);
    fs::write(
        root.join("imports/.body-moves.jsonl"),
        format!(
            "{}\n",
            json!({"move": "m1", "key": key, "record": "body_x",
                "from": {"day": DAY, "facet": "a"}, "to": {"day": "20260102", "facet": "b"},
                "copy": copy})
        ),
    )
    .unwrap();
    let blocked = file(root, "b", "20260102");
    fs::create_dir_all(&blocked).unwrap();
    assert_eq!(run(root).moves_open, 1);
    assert_eq!(lines(root, "a", DAY).len(), 1, "the source stays");
    fs::remove_dir(&blocked).unwrap();
    assert_eq!(run(root).moves_open, 0);
    assert_eq!(lines(root, "b", "20260102").len(), 1);
    assert!(lines(root, "a", DAY).is_empty());
}

#[test]
fn a_line_changed_just_before_its_removal_stays_until_it_is_carried() {
    let journal = journal(&["a"]);
    let root = journal.path();
    workout(root, Some("a"));
    add_line(root, "a", DAY, sense(json!({})));
    BEFORE_REMOVING.set(Some(|root| {
        let path = file(root, "a", DAY);
        let edited = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| {
                let mut value: Value = serde_json::from_str(line).unwrap();
                if value.get("stream").is_some() {
                    value["title"] = json!("Mine");
                    value["edits"] = json!([title_edit("2026-11-09T05:00:00Z", "Mine")]);
                }
                format!("{value}\n")
            })
            .collect::<String>();
        fs::write(&path, edited).unwrap();
    }));
    assert_eq!(run(root).converted, 1);
    let records = lines(root, "a", DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["title"], "Mine");
}

#[test]
fn only_an_unreadable_file_writes_no_marker() {
    let journal = journal(&["a"]);
    let root = journal.path();
    fs::create_dir_all(file(root, "a", "20260105")).unwrap();
    let counts = run(root);
    assert_eq!(counts.waiting, 1, "{counts:?}");
    assert!(!root.join(MARKER).exists());
}
