// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::create_facet;
use crate::store::{
    append_activity_record, load_activity_records, load_activity_records_readable,
    move_body_record, open_body_moves,
};

const LINE: &str = "exercise_070000_300";
const KEY: &str = "import.strava\u{1f}123";
const ID: &str = "body_0123456789abcdef";
const DAY: &str = "20260105";

fn journal() -> TempDir {
    let root = tempfile::tempdir().unwrap();
    for facet in ["a", "b"] {
        create_facet(root.path(), facet, facet, "", "", "", None).unwrap();
    }
    root
}

fn object(value: Value) -> ActivityRecord {
    value.as_object().unwrap().clone()
}

fn sense() -> ActivityRecord {
    object(
        json!({"id": LINE, "activity": "exercise", "stream": "import.strava",
        "segments": ["070000_300"], "created_at": 1}),
    )
}

fn code(converted: &[&str]) -> ActivityRecord {
    let lines = converted
        .iter()
        .map(|key| {
            (
                (*key).to_owned(),
                json!({"sha256": "x", "hidden": false, "fields": {}}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    object(json!({"id": ID, "source": "body", "activity": "workout",
        "account": {"key": KEY, "start": "2026-01-05T07:00:00+00:00",
            "end": "2026-01-05T07:30:00+00:00", "recorder": "r"},
        "converted": {"primary": converted.first(), "decided_at": "t", "lines": lines}}))
}

fn add(root: &Path, facet: &str, day: &str, record: ActivityRecord) {
    let _ = append_activity_record(root, facet, day, record).unwrap();
}

fn line_key(facet: &str, day: &str) -> String {
    format!("{day}|{facet}|{LINE}")
}

fn shown(root: &Path, facet: &str, day: &str) -> usize {
    let strict = load_activity_records(root, facet, day, true).unwrap().len();
    let readable = load_activity_records_readable(root, facet, day, true)
        .unwrap()
        .len();
    assert_eq!(strict, readable);
    strict
}

#[test]
fn a_line_a_code_record_holds_stops_showing_and_one_it_doesnt_stays() {
    let journal = journal();
    let root = journal.path();
    add(root, "a", DAY, sense());
    assert_eq!(shown(root, "a", DAY), 1);

    add(root, "a", DAY, code(&["20260105|a|some_other_line"]));
    assert_eq!(shown(root, "a", DAY), 1, "another line's key hides nothing");
}

#[test]
fn the_record_can_be_in_another_facet_on_another_day() {
    let journal = journal();
    let root = journal.path();
    add(root, "a", DAY, sense());
    add(root, "b", "20260102", code(&[&line_key("a", DAY)]));
    assert_eq!(shown(root, "a", DAY), 0);
}

#[test]
fn the_same_id_in_another_facet_is_a_different_line() {
    let journal = journal();
    let root = journal.path();
    add(root, "a", DAY, sense());
    add(root, "b", DAY, sense());
    add(root, "b", "20260104", code(&[&line_key("b", DAY)]));
    assert_eq!(shown(root, "a", DAY), 1);
    assert_eq!(shown(root, "b", DAY), 0);
}

#[test]
fn the_marker_says_nothing_is_left_so_nothing_is_looked_for() {
    let journal = journal();
    let root = journal.path();
    add(root, "a", DAY, sense());
    add(root, "b", DAY, code(&[&line_key("a", DAY)]));
    std::fs::create_dir_all(root.join("imports")).unwrap();
    std::fs::write(root.join("imports/.strava-converted"), b"").unwrap();
    assert!(!strava_lines_may_be_folded(root));
    assert_eq!(shown(root, "a", DAY), 1);
}

fn place(day: &str, facet: &str) -> BodyRecordPlace {
    BodyRecordPlace {
        day: day.to_owned(),
        facet: facet.to_owned(),
    }
}

fn raw(root: &Path, facet: &str, day: &str) -> Vec<ActivityRecord> {
    std::fs::read_to_string(root.join(format!("facets/{facet}/activities/{day}.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| value.as_object().cloned())
        .collect()
}

fn write_moves(root: &Path, lines: &[Value]) {
    let text = lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(root.join("imports/.body-moves.jsonl"), format!("{text}\n")).unwrap();
}

#[test]
fn an_open_move_shows_the_source_until_the_target_is_placed_and_then_only_the_target() {
    let journal = journal();
    let root = journal.path();
    add(root, "a", DAY, code(&[]));
    // Both sides hold it, as a stop just after the target's write leaves it.
    add(root, "b", DAY, code(&[]));
    std::fs::create_dir_all(root.join("imports")).unwrap();
    let intent = json!({"move": "m", "key": KEY, "record": ID,
        "from": {"day": DAY, "facet": "a"}, "to": {"day": DAY, "facet": "b"},
        "copy": code(&[])});
    write_moves(root, std::slice::from_ref(&intent));
    assert!(!open_body_moves(root).unwrap()[0].placed);
    let gone = |facet: &str| superseded_record_ids(root, facet, DAY, &raw(root, facet, DAY));
    assert!(gone("a").is_empty(), "the source shows");
    assert!(gone("b").contains(ID), "the target waits");

    write_moves(root, &[intent, json!({"placed": "m", "appended": true})]);
    assert!(open_body_moves(root).unwrap()[0].placed);
    assert!(gone("a").contains(ID), "the source stops");
    assert!(gone("b").is_empty(), "the target shows");
}

#[test]
fn a_finished_move_hides_nothing() {
    let journal = journal();
    let root = journal.path();
    add(root, "a", DAY, code(&[]));
    assert_eq!(
        move_body_record(root, ID, &place(DAY, "a"), &place(DAY, "b")).unwrap(),
        crate::store::BodyMoveOutcome::Moved
    );
    assert!(superseded_record_ids(root, "b", DAY, &raw(root, "b", DAY)).is_empty());
}

#[test]
fn a_log_that_cant_be_read_hides_nothing() {
    let journal = journal();
    let root = journal.path();
    add(root, "a", DAY, code(&[]));
    std::fs::create_dir_all(root.join("imports/.body-moves.jsonl")).unwrap();
    assert!(superseded_record_ids(root, "a", DAY, &raw(root, "a", DAY)).is_empty());
}
