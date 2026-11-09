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
    const START: &str = "2026-01-01T07:00:00+00:00";
    tile(root, DAY, "070000_300", WORKOUT, 0, START, facet);
    tile(root, DAY, "070500_300", WORKOUT, 1, START, facet);
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
fn an_earlier_code_record_moves_once_to_where_it_was_visible_and_stays() {
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
    assert!(lines(root, "a", DAY).is_empty());
    let records = lines(root, "b", DAY);
    assert_eq!(records.len(), 1);
    assert_eq!(facet_of(root, DAY, "070000_300").as_deref(), Some("b"));

    // A later return, from a restored line in the first facet, leaves it there.
    add_line(root, "a", DAY, sense(json!({})));
    mark_conversion_needed(root).unwrap();
    assert_eq!(run(root).converted, 1);
    assert!(lines(root, "a", DAY).is_empty());
    assert_eq!(lines(root, "b", DAY).len(), 1);
    assert_eq!(facet_of(root, DAY, "070000_300").as_deref(), Some("b"));
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
    assert!(!root.join("imports/.strava-refiled.json").exists());
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
        // Whatever stopped, no owner field is only somewhere they can't see.
        let visible = lines(root, "a", DAY)
            .into_iter()
            .chain(lines(root, "b", DAY))
            .collect::<Vec<_>>();
        assert!(!visible.is_empty(), "stop {stop}");

        // After the last write nothing is left to find, so a resume may do nothing.
        let resumed = run(root);
        assert_eq!(resumed.waiting, 0, "stop {stop}: {resumed:?}");
        assert_eq!(canon(root), expected, "stop {stop}");
    }
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
