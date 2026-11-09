// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::{append_activity_record, create_facet, refresh_body_account, set_activity_hidden};

const KEY: &str = "import.strava\u{1f}123";
const ID: &str = "body_0123456789abcdef";

fn place(day: &str, facet: &str) -> BodyRecordPlace {
    BodyRecordPlace {
        day: day.to_owned(),
        facet: facet.to_owned(),
    }
}

fn journal() -> TempDir {
    let root = tempfile::tempdir().unwrap();
    for facet in ["a", "b"] {
        create_facet(root.path(), facet, facet, "", "", "", None).unwrap();
    }
    let _ = append_activity_record(root.path(), "a", "20260101", body_record()).unwrap();
    root
}

fn body_record() -> ActivityRecord {
    json!({"id": ID, "source": "body", "activity": "workout",
        "account": {"key": KEY, "start": "2026-01-01T07:00:00+00:00",
            "end": "2026-01-01T07:30:00+00:00", "recorder": "r"}})
    .as_object()
    .unwrap()
    .clone()
}

fn rows(root: &Path, facet: &str, day: &str) -> Vec<ActivityRecord> {
    let path = root.join(format!("facets/{facet}/activities/{day}.jsonl"));
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| value.as_object().cloned())
        .collect()
}

/// An intent as a move leaves it right after step 1.
fn started(root: &Path, from: &BodyRecordPlace, to: &BodyRecordPlace) -> OpenBodyMove {
    let mut copy = rows(root, &from.facet, &from.day).remove(0);
    let to_id = admit_activity_destination(root, &to.facet).unwrap();
    copy.insert("destination_id".to_owned(), Value::String(to_id));
    append_line(
        root,
        &json!({"move": "m1", "key": KEY, "record": ID, "from": place_json(from),
            "to": place_json(to), "copy": copy}),
    )
    .unwrap();
    open_body_moves(root).unwrap().remove(0)
}

#[test]
fn a_move_leaves_one_copy_at_the_target_and_an_empty_log() {
    let root = journal();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    assert_eq!(
        move_body_record(root.path(), ID, &from, &to).unwrap(),
        BodyMoveOutcome::Moved
    );
    assert!(rows(root.path(), "a", "20260101").is_empty());
    let moved = rows(root.path(), "b", "20260102");
    assert_eq!(moved.len(), 1);
    assert_eq!(
        moved[0].get("destination_id"),
        Some(&Value::String(
            admit_activity_destination(root.path(), "b").unwrap()
        ))
    );
    assert!(!moves_path(root.path()).exists());
}

#[test]
fn a_move_to_where_it_is_and_of_nothing_change_nothing() {
    let root = journal();
    let here = place("20260101", "a");
    assert_eq!(
        move_body_record(root.path(), ID, &here, &here).unwrap(),
        BodyMoveOutcome::AlreadyThere
    );
    assert_eq!(
        move_body_record(root.path(), "body_none", &here, &place("20260102", "b")).unwrap(),
        BodyMoveOutcome::NotFound
    );
    assert_eq!(
        move_body_record(
            root.path(),
            ID,
            &place("20270101", "a"),
            &place("20260102", "b")
        )
        .unwrap(),
        BodyMoveOutcome::NotFound
    );
    assert_eq!(rows(root.path(), "a", "20260101").len(), 1);
}

#[test]
fn an_open_move_freezes_the_source_and_refuses_a_new_copy() {
    let root = journal();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    let open = started(root.path(), &from, &to);

    let hide = set_activity_hidden(
        root.path(),
        "a",
        "20260101",
        ID,
        true,
        "owner:hide",
        None,
        "2026-01-03T00:00:00Z",
    );
    assert!(
        matches!(hide, Err(ActivityRecordStoreError::BodyRecordMoving { .. })),
        "{hide:?}"
    );
    let refresh = refresh_body_account(
        root.path(),
        "a",
        "20260101",
        ID,
        "workout",
        "x",
        "y",
        "r",
        None,
    );
    assert!(
        matches!(
            refresh,
            Err(ActivityRecordStoreError::BodyRecordMoving { .. })
        ),
        "{refresh:?}"
    );
    let create = append_activity_record(root.path(), "b", "20260105", body_record());
    assert!(
        matches!(
            create,
            Err(ActivityRecordStoreError::BodyRecordMoving { .. })
        ),
        "{create:?}"
    );
    assert!(rows(root.path(), "b", "20260105").is_empty());

    complete_body_move(root.path(), &open).unwrap();
    assert!(rows(root.path(), "a", "20260101").is_empty());
    assert_eq!(rows(root.path(), "b", "20260102").len(), 1);
    assert!(open_body_moves(root.path()).unwrap().is_empty());
}

#[test]
fn the_target_stays_writable_while_a_move_is_open() {
    let root = journal();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    let open = started(root.path(), &from, &to);
    let mut copy = open.copy.clone();
    copy.remove("destination_id");
    locked_modify_day_records(
        root.path(),
        "b",
        "20260102",
        true,
        &admit_activity_destination(root.path(), "b").unwrap(),
        |mut rows| {
            rows.push(copy);
            Ok((rows, ()))
        },
    )
    .unwrap();
    let hide = set_activity_hidden(
        root.path(),
        "b",
        "20260102",
        ID,
        true,
        "owner:hide",
        None,
        "2026-01-03T00:00:00Z",
    )
    .unwrap();
    assert!(hide.is_some());
}

#[test]
fn a_stop_after_the_append_finishes_with_one_copy_keeping_later_target_edits() {
    let root = journal();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    let open = started(root.path(), &from, &to);
    // A first completion reached the target and recorded it, then stopped.
    let to_id = admit_activity_destination(root.path(), "b").unwrap();
    locked_modify_day_records(root.path(), "b", "20260102", true, &to_id, |mut rows| {
        rows.push(open.copy.clone());
        Ok((rows, ()))
    })
    .unwrap();
    append_line(root.path(), &json!({"placed": open.id, "appended": true})).unwrap();
    set_activity_hidden(
        root.path(),
        "b",
        "20260102",
        ID,
        true,
        "owner:hide",
        None,
        "2026-01-03T00:00:00Z",
    )
    .unwrap();

    let current = open_body_moves(root.path()).unwrap().remove(0);
    assert!(current.appended);
    complete_body_move(root.path(), &current).unwrap();

    let moved = rows(root.path(), "b", "20260102");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].get("hidden"), Some(&Value::Bool(true)));
    assert!(rows(root.path(), "a", "20260101").is_empty());
}

#[test]
fn a_copy_that_arrived_from_elsewhere_gains_only_the_owner_edits_it_lacks() {
    let root = journal();
    let edit = json!({"timestamp": "2026-01-02T00:00:00Z", "actor": "owner:update",
        "fields": ["title"], "note": ""});
    let other = json!({"timestamp": "2026-01-03T00:00:00Z", "actor": "convert",
        "fields": ["title"], "note": ""});
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    let mut open = started(root.path(), &from, &to);
    open.copy
        .insert("edits".to_owned(), json!([edit.clone(), other]));
    let mut delivered = body_record();
    delivered.insert("title".to_owned(), json!("Merged"));
    let to_id = admit_activity_destination(root.path(), "b").unwrap();
    locked_modify_day_records(root.path(), "b", "20260102", true, &to_id, |mut rows| {
        rows.push(delivered);
        Ok((rows, ()))
    })
    .unwrap();

    complete_held(root.path(), &open).unwrap();

    let moved = rows(root.path(), "b", "20260102");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].get("title"), Some(&json!("Merged")));
    assert_eq!(moved[0].get("edits"), Some(&json!([edit])));
    assert!(rows(root.path(), "a", "20260101").is_empty());
}

#[test]
fn a_torn_line_names_no_move_and_the_next_line_survives_it() {
    let root = journal();
    std::fs::create_dir_all(root.path().join("imports")).unwrap();
    std::fs::write(moves_path(root.path()), b"{\"move\":\"torn").unwrap();
    assert!(open_body_moves(root.path()).unwrap().is_empty());
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    let open = started(root.path(), &from, &to);
    assert_eq!(open.id, "m1");
    assert_eq!(open_body_moves(root.path()).unwrap().len(), 1);
}

#[test]
fn another_record_in_the_same_file_stays_writable() {
    let root = journal();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    started(root.path(), &from, &to);
    let other = json!({"id": "other", "activity": "meeting", "title": "x"})
        .as_object()
        .unwrap()
        .clone();
    let _ = append_activity_record(root.path(), "a", "20260101", other).unwrap();
    assert!(
        set_activity_hidden(
            root.path(),
            "a",
            "20260101",
            "other",
            true,
            "owner:hide",
            None,
            "2026-01-03T00:00:00Z",
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn two_open_moves_finish_each_once() {
    let root = journal();
    let other = json!({"id": "body_other", "source": "body", "activity": "workout",
        "account": {"key": "import.apple_health\u{1f}w", "start": "a", "end": "b", "recorder": "r"}})
    .as_object()
    .unwrap()
    .clone();
    let _ = append_activity_record(root.path(), "a", "20260101", other).unwrap();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    assert_eq!(
        move_body_record(root.path(), ID, &from, &to).unwrap(),
        BodyMoveOutcome::Moved
    );
    assert_eq!(
        move_body_record(root.path(), "body_other", &from, &to).unwrap(),
        BodyMoveOutcome::Moved
    );
    assert_eq!(rows(root.path(), "b", "20260102").len(), 2);
    assert!(rows(root.path(), "a", "20260101").is_empty());
}

#[test]
fn a_stop_after_the_intent_appends_the_intents_copy() {
    let root = journal();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    let open = started(root.path(), &from, &to);
    assert!(rows(root.path(), "b", "20260102").is_empty());
    complete_body_move(root.path(), &open).unwrap();
    assert_eq!(rows(root.path(), "b", "20260102").len(), 1);
}

#[test]
fn the_log_says_the_target_holds_it_before_the_source_goes() {
    let root = journal();
    let (from, to) = (place("20260101", "a"), place("20260102", "b"));
    let open = started(root.path(), &from, &to);
    assert!(!open.placed);
    // The source can't be rewritten, so the move stops after the target's write.
    let source = root.path().join("facets/a/activities/20260101.jsonl");
    std::fs::remove_file(&source).unwrap();
    std::fs::create_dir(&source).unwrap();
    assert!(complete_body_move(root.path(), &open).is_err());
    let stopped = open_body_moves(root.path()).unwrap().remove(0);
    assert!(stopped.placed && stopped.appended);
    assert_eq!(rows(root.path(), "b", "20260102").len(), 1);
}
