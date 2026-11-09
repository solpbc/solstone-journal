// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};

use super::*;

const DAY: &str = "20261010";
const NEXT: &str = "20261011";
const STREAM: &str = "import.apple_health";
const START: &str = "2026-10-10 22:10:00 -0600";
const END: &str = "2026-10-11 06:20:00 -0600";

fn journal() -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"America/Denver"}"#,
    )
    .unwrap();
    // The owner's clock, so nothing here depends on the host's zone.
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        r#"{"identity":{"timezone":"America/Denver"}}"#,
    )
    .unwrap();
    let facet = root.join("facets/body");
    fs::create_dir_all(&facet).unwrap();
    fs::write(facet.join("facet.json"), r#"{"title":"Body"}"#).unwrap();
    journal
}

fn night() -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Experience,
        "n1".to_owned(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "Sleep".to_owned(),
    );
    row.experience = Some("sleep".to_owned());
    row.start = Some(START.to_owned());
    row.end = Some(END.to_owned());
    row.by = Some("Synthetic Watch".to_owned());
    row
}

fn tile(index: u64) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::ExperienceTile,
        format!("n1#{index}"),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "Sleep".to_owned(),
    );
    row.of = Some("n1".to_owned());
    row.index = Some(index);
    row.start = Some(START.to_owned());
    row.end = Some(END.to_owned());
    row.by = Some("Synthetic Watch".to_owned());
    row
}

fn window(root: &Path, day: &str, key: &str, rows: &[BodyRow]) -> PathBuf {
    let dir = root.join("chronicle").join(day).join(STREAM).join(key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(BODY_RECORDS_FILE), encode_rows(rows)).unwrap();
    dir
}

/// The owner deletes a window.
fn delete(dir: &Path) {
    delete_for(dir, "owner_segment_delete");
}

/// A window is removed, for `reason`.
fn delete_for(dir: &Path, reason: &str) {
    fs::remove_file(dir.join(BODY_RECORDS_FILE)).unwrap();
    fs::write(
        dir.join(TOMBSTONE),
        json!({"reason": reason, "manifest": ["body.jsonl"]}).to_string(),
    )
    .unwrap();
}

fn record(root: &Path, extra: serde_json::Value) {
    let mut record = json!({
        "id": "body_0123456789abcdef",
        "source": "body",
        "activity": "sleep",
        "account": {"key": format!("{STREAM}\u{1f}n1"), "start": START, "end": END},
    });
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    let dir = root.join("facets/body/activities");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{DAY}.jsonl")), format!("{record}\n")).unwrap();
}

fn everyone(_: &str, _: &str, _: Option<&str>) -> bool {
    true
}

fn composed(root: &Path) -> Vec<ComposedBody> {
    compose_body_records(root, "body", DAY, &everyone, true).unwrap()
}

#[test]
fn a_live_account_composes_from_its_window() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    window(root, NEXT, "000000_3600", &[tile(1)]);
    record(root, json!({}));
    let found = composed(root);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].window.as_deref(), Some("220000_3600"));
    assert_eq!(found[0].account, Account::Live(Box::new(night())));
    assert_eq!(found[0].activity, "sleep");
    // The default title normalize writes isn't the owner's.
    assert!(found[0].owner.is_empty());
}

#[test]
fn a_record_reads_the_copy_the_importer_still_writes_to_when_a_merge_left_two() {
    let journal = journal();
    let root = journal.path();
    let mut older = night();
    older.by = Some("Older Copy".to_owned());
    window(root, DAY, "220000_3600", &[older]);
    window(root, DAY, "220000_3601", &[night()]);
    record(root, json!({}));
    let found = composed(root);
    assert_eq!(found[0].window.as_deref(), Some("220000_3601"));
    assert_eq!(found[0].account, Account::Live(Box::new(night())));
}

#[test]
fn a_deleted_later_piece_marks_a_live_account_part_removed_as_a_deleted_start_does() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    let later = window(root, DAY, "230000_3600", &[tile(1)]);
    window(root, NEXT, "000000_3600", &[tile(2)]);
    record(root, json!({}));
    assert_eq!(composed(root)[0].account, Account::Live(Box::new(night())));
    // The owner's delete of a later piece: the account says part of it is
    // gone, and keeps the times its own row states.
    delete(&later);
    assert_eq!(
        composed(root)[0].account,
        Account::PartRemoved(Box::new(night()))
    );
}

#[test]
fn retention_reaching_a_later_piece_marks_it_and_a_release_does_not() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    let released = window(root, NEXT, "000000_3600", &[tile(1)]);
    record(root, json!({}));
    // A run's release puts its pieces back beside it on a later import, so
    // it removes nothing from the account.
    delete_for(&released, "import_run_release");
    assert_eq!(composed(root)[0].account, Account::Live(Box::new(night())));
    let swept = window(root, NEXT, "010000_3600", &[tile(2)]);
    delete_for(&swept, "retention");
    assert_eq!(
        composed(root)[0].account,
        Account::PartRemoved(Box::new(night()))
    );
}

#[test]
fn a_deleted_whole_day_window_on_the_day_a_night_ends_leaves_it_live() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    window(root, NEXT, "000000_3600", &[tile(1)]);
    // The next day's dated scores: not a piece of the night.
    let dated = window(root, NEXT, "000000_86400", &[]);
    record(root, json!({}));
    delete(&dated);
    assert_eq!(composed(root)[0].account, Account::Live(Box::new(night())));
}

#[test]
fn a_deleted_window_on_another_grid_inside_the_span_leaves_the_account_live() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    window(root, DAY, "230000_3600", &[tile(1)]);
    // A five-minute session's window inside the night is another grid's.
    let session = window(root, DAY, "231500_300", &[]);
    record(root, json!({}));
    delete(&session);
    assert_eq!(composed(root)[0].account, Account::Live(Box::new(night())));
}

#[test]
fn removed_by_you_only_when_no_tile_of_the_span_survives() {
    let journal = journal();
    let root = journal.path();
    let start = window(root, DAY, "220000_3600", &[night()]);
    let later = window(root, NEXT, "000000_3600", &[tile(1)]);
    record(root, json!({}));
    delete(&start);
    assert_eq!(
        composed(root)[0].account,
        Account::PartRemoved(Box::new(tile(1)))
    );
    delete(&later);
    assert_eq!(composed(root)[0].account, Account::RemovedByYou);
}

#[test]
fn a_live_window_without_the_row_and_an_unreadable_one_say_so_apart() {
    let journal = journal();
    let root = journal.path();
    let dir = window(root, DAY, "220000_3600", &[]);
    record(root, json!({}));
    assert_eq!(composed(root)[0].account, Account::NotFound);
    fs::write(dir.join(BODY_RECORDS_FILE), "not json\n").unwrap();
    assert_eq!(composed(root)[0].account, Account::Unreadable);
}

fn moved(id: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Moved,
        id.to_owned(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "Sleep".to_owned(),
    );
    row.of = Some("n1".to_owned());
    row
}

/// A moved row is no account: the record reads from the window its start is in
/// now, a record left at a moved row finds nothing there, and a moved tile is
/// no piece of the span that survives a delete.
#[test]
fn a_moved_row_is_never_the_account_or_a_piece_of_it() {
    let journal = journal();
    let root = journal.path();
    // The night now starts an hour later; its old first window holds a mark.
    window(root, DAY, "210000_3600", &[moved("n1")]);
    window(root, DAY, "220000_3600", &[night()]);
    record(root, json!({}));
    let found = composed(root);
    assert_eq!(found[0].window.as_deref(), Some("220000_3600"));
    assert_eq!(found[0].account, Account::Live(Box::new(night())));

    // A record still at the old start reads as nothing found, never as the mark.
    let journal = self::journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[moved("n1")]);
    record(root, json!({}));
    assert_eq!(composed(root)[0].account, Account::NotFound);
    let mut windows = Windows::default();
    let stored = stored_body_records(root, "body", DAY).unwrap().remove(0);
    assert_eq!(windows.account_windows(root, &stored), ["220000_3600"]);

    // Its first window deleted, a moved tile later on isn't a piece that survives.
    let journal = self::journal();
    let root = journal.path();
    let start = window(root, DAY, "220000_3600", &[night()]);
    window(root, NEXT, "000000_3600", &[moved("n1#1")]);
    record(root, json!({}));
    delete(&start);
    assert_eq!(composed(root)[0].account, Account::RemovedByYou);
}

#[test]
fn a_reader_that_may_not_read_the_window_gets_no_record() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    record(root, json!({}));
    let nobody = |_: &str, _: &str, _: Option<&str>| false;
    assert!(
        compose_body_records(root, "body", DAY, &nobody, true)
            .unwrap()
            .is_empty()
    );
    let only_that = |_: &str, stream: &str, window: Option<&str>| {
        stream == STREAM && window == Some("220000_3600")
    };
    assert_eq!(
        compose_body_records(root, "body", DAY, &only_that, true)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn owner_fields_and_hidden_come_from_the_record() {
    let journal = journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    record(
        root,
        json!({
            "title": "Long night",
            "hidden": true,
            "edits": [{"actor": "owner:update", "fields": ["title"], "note": "", "timestamp": "2026-10-12T00:00:00Z"}],
        }),
    );
    assert!(
        compose_body_records(root, "body", DAY, &everyone, false)
            .unwrap()
            .is_empty()
    );
    let found = composed(root);
    assert!(found[0].hidden);
    assert_eq!(found[0].owner.get("title"), Some(&json!("Long night")));
}

fn stored(
    key: &str,
    activity: &str,
    kind: Option<&str>,
    recorder: &str,
    start: &str,
    end: &str,
) -> StoredBody {
    StoredBody {
        id: format!("body_{key}"),
        facet: "body".to_owned(),
        day: DAY.to_owned(),
        activity: activity.to_owned(),
        key: format!("{STREAM}\u{1f}{key}"),
        stream: STREAM.to_owned(),
        row_id: key.to_owned(),
        stored_start: format!("2026-10-10 {start}:00 -0600"),
        stored_end: format!("2026-10-10 {end}:00 -0600"),
        workout_type: kind.map(str::to_owned),
        recorder: recorder.to_owned(),
        arrived: Some(DAY.to_owned()),
        owner: BTreeMap::new(),
        hidden: false,
        apart: Vec::new(),
    }
}

fn run(key: &str, recorder: &str, start: &str, end: &str) -> StoredBody {
    stored(key, "workout", Some("running"), recorder, start, end)
}

fn keys(records: &[StoredBody]) -> Vec<Vec<String>> {
    group_partition(records)
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .map(|index| records[index].row_id.clone())
                .collect()
        })
        .collect()
}

#[test]
fn near_misses_stay_alone() {
    // Back to back from one recorder, and from two.
    assert_eq!(
        keys(&[
            run("a", "watch", "07:00", "07:30"),
            run("b", "watch", "07:30", "08:00")
        ]),
        [["a"], ["b"]]
    );
    assert_eq!(
        keys(&[
            run("a", "watch", "07:00", "07:30"),
            run("b", "phone", "07:30", "08:00")
        ]),
        [["a"], ["b"]]
    );
    // One recorder's two overlapping runs.
    assert_eq!(
        keys(&[
            run("a", "watch", "07:00", "08:00"),
            run("b", "watch", "07:01", "08:01")
        ]),
        [["a"], ["b"]]
    );
    // A walk inside a run.
    let walk = stored("w", "workout", Some("walking"), "phone", "07:10", "07:20");
    assert_eq!(
        keys(&[run("a", "watch", "07:00", "08:00"), walk]),
        [["a"], ["w"]]
    );
    // A walk inside a longer walk from another recorder.
    let long = stored("l", "workout", Some("walking"), "watch", "07:00", "08:00");
    let short = stored("s", "workout", Some("walking"), "phone", "07:10", "07:30");
    assert_eq!(keys(&[long, short]), [["l"], ["s"]]);
    // A nap inside a night.
    let night = stored("n", "sleep", None, "watch", "00:00", "07:00");
    let nap = stored("p", "sleep", None, "phone", "03:00", "04:00");
    assert_eq!(keys(&[night, nap]), [["n"], ["p"]]);
    // An unmapped workout type pairs with nothing.
    let odd = stored("o", "workout", None, "watch", "07:00", "08:00");
    let odd2 = stored("q", "workout", None, "phone", "07:00", "08:00");
    assert_eq!(keys(&[odd, odd2]), [["o"], ["q"]]);
}

#[test]
fn one_experience_from_two_recorders_groups() {
    // A watch run and a third-party app's run.
    assert_eq!(
        keys(&[
            run("w", "watch", "07:00", "07:40"),
            run("x", "strava", "07:02", "07:41")
        ]),
        [["w", "x"]]
    );
    // A phone night and a watch night 25 minutes apart at each end.
    let phone = stored("p", "sleep", None, "phone", "00:00", "07:00");
    let watch = stored("q", "sleep", None, "watch", "00:25", "06:35");
    assert_eq!(keys(&[phone, watch]), [["p", "q"]]);
}

#[test]
fn a_set_that_isnt_a_clique_stands_alone_and_order_never_matters() {
    // W and X pass, A1 and X pass, W and A1 don't: three alone.
    let w = run("w", "watch", "07:00", "08:00");
    let x = run("x", "strava", "07:07", "08:07");
    let a = run("a", "phone", "07:15", "08:15");
    assert!(pair(&w, &x) && pair(&a, &x) && !pair(&w, &a), "fixture");
    assert_eq!(
        keys(&[w.clone(), x.clone(), a.clone()]),
        [["w"], ["x"], ["a"]]
    );
    // Three that all pass group, in every order.
    let e = [
        run("e1", "watch", "07:00", "08:00"),
        run("e2", "strava", "07:01", "08:01"),
        run("e3", "phone", "07:02", "08:02"),
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let records = order.map(|index| e[index].clone());
        let mut found = keys(&records);
        for group in &mut found {
            group.sort();
        }
        assert_eq!(found, [["e1", "e2", "e3"]], "{order:?}");
    }
}

#[test]
fn apart_and_two_edits_keep_accounts_apart_and_a_third_stays_grouped() {
    let mut w = run("w", "watch", "07:00", "08:00");
    let x = run("x", "strava", "07:01", "08:01");
    let mut a = run("a", "phone", "07:02", "08:02");
    // Separating A from {W, X, A}: each side lists the other.
    a.apart = vec![w.key.clone(), x.key.clone()];
    w.apart = vec![a.key.clone()];
    assert_eq!(
        keys(&[w.clone(), x.clone(), a.clone()]),
        [vec!["w", "x"], vec!["a"]]
    );
    // Two records the owner edited never group; one edited is the anchor.
    let mut left = run("l", "watch", "07:00", "08:00");
    let mut right = run("r", "strava", "07:01", "08:01");
    right
        .owner
        .insert("title".to_owned(), json!("Hill repeats"));
    let records = [left.clone(), right.clone()];
    assert_eq!(keys(&records), [["l", "r"]]);
    assert_eq!(records[anchor(&records, &[0, 1])].row_id, "r");
    left.owner.insert("details".to_owned(), json!("felt slow"));
    assert_eq!(keys(&[left, right]), [["l"], ["r"]]);
}

#[test]
fn the_anchor_is_the_earliest_start_when_nobody_edited() {
    let records = [
        run("late", "strava", "07:01", "08:01"),
        run("early", "watch", "07:00", "08:00"),
    ];
    assert_eq!(records[anchor(&records, &[0, 1])].row_id, "early");
}

#[test]
fn separate_keeps_one_member_out_for_good_and_the_rest_grouped() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    solstone_core_facets::create_facet(root, "body", "Body", "Test", "#123456", "", None).unwrap();
    let line = |key: &str, recorder: &str, start: &str, end: &str| {
        json!({
            "id": format!("body_{key}"), "source": "body", "activity": "workout",
            "account": {"key": format!("{STREAM}\u{1f}{key}"), "recorder": recorder, "type": "running",
                "start": format!("2026-10-10 {start}:00 -0600"), "end": format!("2026-10-10 {end}:00 -0600")},
        })
    };
    for row in [
        line("w", "watch", "07:00", "08:00"),
        line("x", "strava", "07:01", "08:01"),
        line("a", "phone", "07:02", "08:02"),
    ] {
        let _ = solstone_core_facets::append_activity_record(
            root,
            "body",
            DAY,
            row.as_object().unwrap().clone(),
        )
        .unwrap();
    }
    let records = stored_body_records(root, "body", DAY).unwrap();
    assert_eq!(group_partition(&records), [[0, 1, 2]]);
    let (member, others) = (
        records[2].clone(),
        vec![records[0].clone(), records[1].clone()],
    );
    separate_account(root, &member, &others, "2026-10-12T00:00:00Z").unwrap();
    let after = stored_body_records(root, "body", DAY).unwrap();
    assert_eq!(group_partition(&after), [vec![0, 1], vec![2]]);
    // An owner field it gains later doesn't bring it back, and no edit undoes it.
    let raw = solstone_core_facets::read_activity_records_strict(root, "body", DAY).unwrap();
    let edits = raw[2]["edits"].as_array().unwrap();
    assert_eq!(edits.last().unwrap()["actor"], SEPARATE_ACTOR);
    assert!(!solstone_core_facets::edit_removable(
        edits.last().unwrap().as_object().unwrap()
    ));
    // Separating again writes nothing.
    let before = fs::read(root.join(format!("facets/body/activities/{DAY}.jsonl"))).unwrap();
    separate_account(
        root,
        &after[2],
        &[after[0].clone(), after[1].clone()],
        "2026-10-12T00:01:00Z",
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join(format!("facets/body/activities/{DAY}.jsonl"))).unwrap(),
        before
    );
}

fn workout_row(id: &str, by: &str, start: &str, end: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Experience,
        id.to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some("workout".to_owned());
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row.by = Some(by.to_owned());
    row
}

fn workout_record(root: &Path, day: &str, row: &BodyRow, extra: serde_json::Value) {
    let mut record = json!({
        "id": format!("body_{}", row.id), "source": "body", "activity": "workout",
        "account": {"key": format!("{STREAM}\u{1f}{}", row.id), "start": row.start, "end": row.end,
            "recorder": solstone_core_format::content::body_records::recorder_id(STREAM, row.by.as_deref()),
            "type": "running"},
    });
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    let path = root.join(format!("facets/body/activities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(path, text).unwrap();
}

#[test]
fn a_face_holds_only_members_a_reader_may_read_and_shows_under_its_anchors_day() {
    let journal = journal();
    let root = journal.path();
    let watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    let app = workout_row(
        "x",
        "Synthetic App",
        "2026-10-10 07:01:00 -0600",
        "2026-10-10 07:41:00 -0600",
    );
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    window(root, DAY, "070000_300", std::slice::from_ref(&app));
    workout_record(root, DAY, &watch, json!({}));
    workout_record(root, DAY, &app, json!({"hidden": true}));

    let faces = body_faces(root, "body", DAY, &everyone, true).unwrap();
    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].anchor.id, "body_w");
    assert_eq!(faces[0].members.len(), 2);
    assert!(faces[0].hidden, "hidden when any member is");
    assert!(
        body_faces(root, "body", DAY, &everyone, false)
            .unwrap()
            .is_empty()
    );

    // A reader that may not read the watch's window sees the app's account alone.
    let not_watch = |_: &str, _: &str, window: Option<&str>| window != Some("070000_3600");
    let faces = body_faces(root, "body", DAY, &not_watch, true).unwrap();
    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].anchor.id, "body_x");
    let text = face_json(&faces[0]).to_string();
    assert!(
        !text.contains("body_w") && !text.contains("Synthetic Watch"),
        "{text}"
    );

    // Nothing shows on a day the anchor isn't filed on, and nothing at all
    // without body data.
    assert!(
        body_faces(root, "body", NEXT, &everyone, true)
            .unwrap()
            .is_empty()
    );
    fs::remove_file(root.join("imports/.apple_health-zone.json")).unwrap();
    assert!(
        body_faces(root, "body", DAY, &everyone, true)
            .unwrap()
            .is_empty()
    );
}

fn arrived_record(
    root: &Path,
    day: &str,
    row: &BodyRow,
    arrived: Option<&str>,
    extra: serde_json::Value,
) {
    let mut account = json!({"key": format!("{STREAM}\u{1f}{}", row.id), "start": row.start, "end": row.end,
        "recorder": solstone_core_format::content::body_records::recorder_id(STREAM, row.by.as_deref()),
        "type": "running"});
    if let Some(arrived) = arrived {
        account["arrived"] = json!(arrived);
    }
    let mut record = json!({"id": format!("body_{}", row.id), "source": "body", "activity": row.experience, "account": account});
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    let path = root.join(format!("facets/body/activities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(path, text).unwrap();
}

fn day_ids(root: &Path, day: &str) -> Vec<Vec<String>> {
    day_faces(root, day)
        .unwrap()
        .iter()
        .map(|face| {
            face.members
                .iter()
                .map(|member| member.id.clone())
                .collect()
        })
        .collect()
}

#[test]
fn the_home_reads_a_day_whenever_its_records_arrived() {
    let journal = journal();
    let root = journal.path();
    let watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    let app = workout_row(
        "x",
        "Synthetic App",
        "2026-10-10 07:01:00 -0600",
        "2026-10-10 07:41:00 -0600",
    );
    let late = workout_row(
        "l",
        "Synthetic Ring",
        "2026-10-10 07:02:00 -0600",
        "2026-10-10 07:42:00 -0600",
    );
    let bare = workout_row(
        "b",
        "Synthetic Phone",
        "2026-10-10 12:00:00 -0600",
        "2026-10-10 12:30:00 -0600",
    );
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    window(root, DAY, "070000_300", std::slice::from_ref(&app));
    window(root, DAY, "070500_300", std::slice::from_ref(&late));
    window(root, DAY, "120000_3600", std::slice::from_ref(&bare));
    arrived_record(root, DAY, &watch, Some("20261011"), json!({}));
    arrived_record(root, DAY, &app, Some("20261017"), json!({}));
    // Read when the owner looks, so a late arrival counts like any other, and
    // so does a record with no arrival day.
    arrived_record(root, DAY, &late, Some("20261209"), json!({}));
    arrived_record(root, DAY, &bare, None, json!({}));
    assert_eq!(
        day_ids(root, DAY),
        [vec!["body_w", "body_x", "body_l"], vec!["body_b"]]
    );
    // Nothing on the next day: the face belongs to the day it started.
    assert!(day_ids(root, NEXT).is_empty());
}

#[test]
fn the_home_leaves_out_hidden_muted_and_deleted_faces_and_takes_a_night_on_its_first_day() {
    let journal = journal();
    let root = journal.path();
    let start = window(root, DAY, "220000_3600", &[night()]);
    arrived_record(root, DAY, &night(), Some(DAY), json!({}));
    assert_eq!(day_ids(root, DAY), [vec!["body_n1"]]);
    assert!(day_ids(root, NEXT).is_empty());
    // Deleted with no surviving tile: nothing.
    delete(&start);
    assert!(day_ids(root, DAY).is_empty());
    // Hidden: nothing.
    let journal = journal_fixture_with_live_night();
    let root = journal.path();
    let path = root.join(format!("facets/body/activities/{DAY}.jsonl"));
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("\"source\"", "\"hidden\":true,\"source\"");
    fs::write(&path, text).unwrap();
    assert!(day_ids(root, DAY).is_empty());
    // Muted facet: nothing.
    let journal = journal_fixture_with_live_night();
    let root = journal.path();
    fs::write(
        root.join("facets/body/facet.json"),
        r#"{"title":"Body","muted":true}"#,
    )
    .unwrap();
    assert!(day_ids(root, DAY).is_empty());
}

fn journal_fixture_with_live_night() -> tempfile::TempDir {
    let journal = journal();
    window(journal.path(), DAY, "220000_3600", &[night()]);
    arrived_record(journal.path(), DAY, &night(), Some(DAY), json!({}));
    journal
}

/// Anchor and member ids of faces, in order.
fn face_ids(faces: &[Face]) -> Vec<(String, Vec<String>, bool)> {
    faces
        .iter()
        .map(|face| {
            (
                face.anchor.id.clone(),
                face.members
                    .iter()
                    .map(|member| member.id.clone())
                    .collect(),
                face.hidden,
            )
        })
        .collect()
}

#[test]
fn a_range_of_days_shows_the_faces_each_day_shows_alone() {
    let journal = journal();
    let root = journal.path();
    let watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    let app = workout_row(
        "x",
        "Synthetic App",
        "2026-10-10 07:01:00 -0600",
        "2026-10-10 07:41:00 -0600",
    );
    let next = workout_row(
        "n",
        "Synthetic Watch",
        "2026-10-11 18:00:00 -0600",
        "2026-10-11 18:30:00 -0600",
    );
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    window(root, DAY, "070000_300", std::slice::from_ref(&app));
    window(root, NEXT, "180000_3600", std::slice::from_ref(&next));
    workout_record(root, DAY, &watch, json!({}));
    workout_record(root, DAY, &app, json!({}));
    workout_record(root, NEXT, &next, json!({"hidden": true}));

    let not_watch = |_: &str, _: &str, window: Option<&str>| window != Some("070000_3600");
    for readable in [&everyone as Readable<'_>, &not_watch] {
        for include_hidden in [true, false] {
            let range =
                body_faces_range(root, "body", DAY, NEXT, Some(readable), include_hidden).unwrap();
            for day in [DAY, NEXT] {
                let alone = body_faces(root, "body", day, readable, include_hidden).unwrap();
                assert_eq!(
                    face_ids(range.get(day).map_or(&[][..], Vec::as_slice)),
                    face_ids(&alone),
                    "{day} include_hidden={include_hidden}"
                );
            }
            assert!(
                range
                    .keys()
                    .all(|day| day.as_str() == DAY || day.as_str() == NEXT)
            );
        }
    }
    let all = body_faces_range(root, "body", DAY, NEXT, None, true).unwrap();
    assert_eq!(
        face_ids(&all[DAY]),
        vec![(
            "body_w".to_owned(),
            vec!["body_w".to_owned(), "body_x".to_owned()],
            false
        )]
    );
    assert_eq!(
        face_ids(&all[NEXT]),
        vec![("body_n".to_owned(), vec!["body_n".to_owned()], true)]
    );
    // The owner's range reads every window, the same as a reader that may read all.
    assert_eq!(
        body_faces_range(root, "body", DAY, NEXT, None, true)
            .unwrap()
            .values()
            .map(|faces| face_ids(faces))
            .collect::<Vec<_>>(),
        body_faces_range(root, "body", DAY, NEXT, Some(&everyone), true)
            .unwrap()
            .values()
            .map(|faces| face_ids(faces))
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_account_composes_only_from_a_window_its_start_falls_in() {
    let journal = journal();
    let root = journal.path();
    // The night starts at 22:10; its row sits only in a noon window, which a
    // reader's gate never checks for it.
    window(root, DAY, "220000_3600", &[]);
    window(root, DAY, "120000_3600", &[night()]);
    record(root, json!({}));
    let found = composed(root);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].account, Account::NotFound);
    assert_eq!(found[0].window.as_deref(), Some("220000_3600"));
}

#[test]
fn separate_keeps_a_line_it_cannot_read() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    solstone_core_facets::create_facet(root, "body", "Body", "Test", "#123456", "", None).unwrap();
    for (key, recorder, start) in [("w", "watch", "07:00"), ("x", "strava", "07:01")] {
        let row = json!({
            "id": format!("body_{key}"), "source": "body", "activity": "workout",
            "account": {"key": format!("{STREAM}\u{1f}{key}"), "recorder": recorder, "type": "running",
                "start": format!("2026-10-10 {start}:00 -0600"), "end": "2026-10-10 08:00:00 -0600"},
        });
        let _ = solstone_core_facets::append_activity_record(
            root,
            "body",
            DAY,
            row.as_object().unwrap().clone(),
        )
        .unwrap();
    }
    let records = stored_body_records(root, "body", DAY).unwrap();
    let path = root.join(format!("facets/body/activities/{DAY}.jsonl"));
    let half = r#"{"id":"body_y","source":"bo"#;
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(half);
    text.push('\n');
    fs::write(&path, text).unwrap();

    separate_account(root, &records[1], &records[..1], "2026-10-12T00:00:00Z").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(lines[2], half);
    assert!(
        lines[0].contains(r#""apart""#) && lines[1].contains(r#""apart""#),
        "{text}"
    );
}

#[test]
fn an_assembled_night_says_so_in_its_face() {
    let journal = journal();
    let root = journal.path();
    let mut assembled = night();
    assembled.assembled = Some("stages joined across gaps under an hour".to_owned());
    window(root, DAY, "220000_3600", std::slice::from_ref(&assembled));
    record(root, json!({}));
    let faces = body_faces(root, "body", DAY, &everyone, true).unwrap();
    let face = face_json(&faces[0]);
    assert_eq!(
        face["accounts"][0]["assembled"], "stages joined across gaps under an hour",
        "{face}"
    );
}

#[test]
fn removed_by_you_only_for_the_owners_own_delete() {
    for (reason, expected) in [
        ("owner_segment_delete", Account::RemovedByYou),
        ("retention_policy", Account::Removed),
        ("import_run_release", Account::Removed),
    ] {
        let journal = journal();
        let root = journal.path();
        let start = window(root, DAY, "220000_3600", &[night()]);
        record(root, json!({}));
        delete_for(&start, reason);
        assert_eq!(composed(root)[0].account, expected, "{reason}");
    }
    // A tombstone that can't be read names nobody.
    let journal = journal();
    let root = journal.path();
    let start = window(root, DAY, "220000_3600", &[night()]);
    record(root, json!({}));
    delete(&start);
    fs::write(start.join(TOMBSTONE), "{not json").unwrap();
    assert_eq!(composed(root)[0].account, Account::Removed);
}

#[test]
fn a_window_that_cannot_be_read_beside_a_deleted_one_reads_unreadable() {
    let journal = journal();
    let root = journal.path();
    let hour = window(root, DAY, "220000_3600", &[]);
    let short = window(root, DAY, "221000_300", &[night()]);
    record(root, json!({}));
    delete(&hour);
    // The row's own window is live: a deleted window beside it changes nothing.
    assert_eq!(composed(root)[0].account, Account::Live(Box::new(night())));
    fs::write(short.join(BODY_RECORDS_FILE), "not json\n").unwrap();
    assert_eq!(composed(root)[0].account, Account::Unreadable);
}

#[test]
fn a_deleted_start_with_a_later_piece_that_cannot_be_read_reads_unreadable() {
    let journal = journal();
    let root = journal.path();
    let start = window(root, DAY, "220000_3600", &[night()]);
    let later = window(root, NEXT, "000000_3600", &[tile(1)]);
    // A window the night never reached can't be read: it says nothing of it.
    let noon = window(root, NEXT, "120000_3600", &[]);
    fs::write(noon.join(BODY_RECORDS_FILE), "not json\n").unwrap();
    record(root, json!({}));
    delete(&start);
    delete(&later);
    assert_eq!(composed(root)[0].account, Account::RemovedByYou);
    // One it did reach might still hold a piece of it.
    fs::write(later.join(BODY_RECORDS_FILE), "not json\n").unwrap();
    fs::remove_file(later.join(TOMBSTONE)).unwrap();
    assert_eq!(composed(root)[0].account, Account::Unreadable);
}

#[test]
fn a_group_the_owner_hid_hides_from_a_reader_of_one_member_alone() {
    let journal = journal();
    let root = journal.path();
    let watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    let app = workout_row(
        "x",
        "Synthetic App",
        "2026-10-10 07:01:00 -0600",
        "2026-10-10 07:41:00 -0600",
    );
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    window(root, DAY, "070000_300", std::slice::from_ref(&app));
    // The watch's account was hidden before the app's arrived.
    workout_record(root, DAY, &watch, json!({"hidden": true}));
    workout_record(root, DAY, &app, json!({}));
    let not_watch = |_: &str, _: &str, window: Option<&str>| window != Some("070000_3600");
    assert!(
        body_faces(root, "body", DAY, &not_watch, false)
            .unwrap()
            .is_empty()
    );
    assert!(
        body_faces(root, "body", DAY, &everyone, false)
            .unwrap()
            .is_empty()
    );
    // Shown hidden, it names only what this reader reads.
    let faces = body_faces(root, "body", DAY, &not_watch, true).unwrap();
    assert_eq!(faces.len(), 1);
    assert!(faces[0].hidden);
    assert!(!face_json(&faces[0]).to_string().contains("body_w"));
}

fn lines_on(root: &Path, day: &str) -> Vec<serde_json::Value> {
    day_lines(root, day).unwrap()
}

fn at(time: &str, date: Option<&str>) -> serde_json::Value {
    json!({"time": time, "date": date})
}

#[test]
fn a_line_says_what_when_how_far_and_where_from_and_nothing_else() {
    let journal = journal();
    let root = journal.path();
    let mut watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    // Every value a source gives; only distance may be said.
    for (name, value) in [
        ("distance", "5.0 km"),
        ("duration", "40 min"),
        ("energy", "512 kcal"),
        ("heart rate average", "152 count/min"),
        ("heart rate maximum", "181 count/min"),
    ] {
        watch.values.insert(name.to_owned(), value.to_owned());
    }
    let app = workout_row(
        "x",
        "Synthetic App",
        "2026-10-10 07:01:00 -0600",
        "2026-10-10 07:41:00 -0600",
    );
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    window(root, DAY, "070000_300", std::slice::from_ref(&app));
    arrived_record(root, DAY, &watch, Some(DAY), json!({}));
    arrived_record(root, DAY, &app, Some(DAY), json!({}));
    let lines = lines_on(root, DAY);
    assert_eq!(
        lines,
        [
            json!({"name": "running", "start": at("07:00", None), "end": at("07:40", None),
            "distance": "5.0 km", "from": ["Synthetic Watch", "Synthetic App"],
            "logged": false, "part_removed": false})
        ]
    );
    let text = lines[0].to_string();
    for statistic in ["40 min", "kcal", "count/min", "152", "181", "512"] {
        assert!(!text.contains(statistic), "{statistic}: {text}");
    }
}

#[test]
fn a_line_takes_the_owners_title_says_logged_crosses_midnight_and_a_removed_start() {
    // The owner titled it.
    let journal = journal();
    let root = journal.path();
    let watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    arrived_record(root, DAY, &watch, Some(DAY), json!({}));
    let mut patch = Map::new();
    patch.insert("title".to_owned(), json!("Hill repeats"));
    solstone_core_facets::update_activity_record(
        root,
        "body",
        DAY,
        "body_w",
        &patch,
        "owner:web",
        "",
        "2026-10-11T00:00:00Z",
    )
    .unwrap()
    .expect("titled");
    assert_eq!(lines_on(root, DAY)[0]["name"], "Hill repeats");

    // Logged by hand.
    let journal = self::journal();
    let root = journal.path();
    let mut logged = workout_row(
        "m",
        "Synthetic Phone",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:10:00 -0600",
    );
    logged.experience = Some("mindfulness".to_owned());
    logged
        .fields
        .insert("metadata".to_owned(), json!({"HKWasUserEntered": "1"}));
    window(root, DAY, "070000_3600", std::slice::from_ref(&logged));
    arrived_record(root, DAY, &logged, Some(DAY), json!({}));
    let logged = &lines_on(root, DAY)[0];
    assert_eq!(logged["name"], "mindfulness");
    assert_eq!(logged["logged"], true);
    assert_eq!(logged["from"], json!([]));

    // A night across midnight, then with its start deleted and a later piece left.
    let journal = journal_fixture_with_live_night();
    let root = journal.path();
    let night = &lines_on(root, DAY)[0];
    assert_eq!(night["name"], "sleep");
    assert_eq!(night["start"], at("22:10", None));
    assert_eq!(night["end"], at("06:20", Some(NEXT)));
    assert_eq!(night["part_removed"], false);
    let later = window(root, NEXT, "000000_3600", &[tile(1)]);
    let _ = later;
    delete(&root.join(format!("chronicle/{DAY}/{STREAM}/220000_3600")));
    let partial = &lines_on(root, DAY)[0];
    assert_eq!(partial["part_removed"], true);
    assert_eq!(partial["distance"], serde_json::Value::Null);
}

/// A watch's run with no distance beside an app's with one: the line says the
/// app's. With no distance anywhere it says none, and never another value. A
/// logged account beside a recorded one isn't "which you logged".
#[test]
fn a_line_takes_the_first_distance_any_account_says_and_logged_only_when_every_one_was() {
    let journal = journal();
    let root = journal.path();
    let mut watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    watch
        .values
        .insert("energy".to_owned(), "512 kcal".to_owned());
    let mut app = workout_row(
        "x",
        "Synthetic App",
        "2026-10-10 07:01:00 -0600",
        "2026-10-10 07:41:00 -0600",
    );
    app.values
        .insert("distance".to_owned(), "5.1 km".to_owned());
    app.fields
        .insert("metadata".to_owned(), json!({"HKWasUserEntered": "1"}));
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    let app_window = window(root, DAY, "070000_300", std::slice::from_ref(&app));
    arrived_record(root, DAY, &watch, Some(DAY), json!({}));
    arrived_record(root, DAY, &app, Some(DAY), json!({}));
    let line = &lines_on(root, DAY)[0];
    assert_eq!(line["distance"], "5.1 km", "{line}");
    assert_eq!(line["logged"], false, "{line}");
    assert_eq!(
        line["from"],
        json!(["Synthetic Watch", "Synthetic App"]),
        "{line}"
    );
    assert!(!line.to_string().contains("kcal"), "{line}");
    // No account says a distance: the line says none, not the energy.
    app.values.clear();
    fs::write(app_window.join(BODY_RECORDS_FILE), encode_rows(&[app])).unwrap();
    let line = &lines_on(root, DAY)[0];
    assert_eq!(line["distance"], serde_json::Value::Null, "{line}");
    assert!(!line.to_string().contains("kcal"), "{line}");
}

/// A window refiled to a muted facet is muted, wherever its record is filed.
#[test]
fn the_home_leaves_out_a_window_refiled_to_a_muted_facet() {
    let journal = journal_fixture_with_live_night();
    let root = journal.path();
    let quiet = root.join("facets/quiet");
    fs::create_dir_all(&quiet).unwrap();
    fs::write(
        quiet.join("facet.json"),
        r#"{"title":"Quiet","muted":true}"#,
    )
    .unwrap();
    let start = root
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("220000_3600");
    fs::create_dir_all(start.join("talents")).unwrap();
    fs::write(
        start.join("talents/facets.json"),
        json!([{"facet": "quiet"}]).to_string(),
    )
    .unwrap();
    assert!(day_ids(root, DAY).is_empty());
    // Unmuted, it shows again.
    fs::write(quiet.join("facet.json"), r#"{"title":"Quiet"}"#).unwrap();
    assert_eq!(day_ids(root, DAY), [vec!["body_n1"]]);
}

/// Two deleted windows cover a start, one by the owner and one not. A
/// tombstone doesn't name its rows, so it reads removed, in either order.
#[test]
fn removed_by_you_only_when_every_deleted_window_over_the_start_was_the_owners() {
    for (hour, short, expected) in [
        ("owner_segment_delete", "retention_policy", Account::Removed),
        ("retention_policy", "owner_segment_delete", Account::Removed),
        (
            "owner_segment_delete",
            "owner_segment_delete",
            Account::RemovedByYou,
        ),
    ] {
        let journal = journal();
        let root = journal.path();
        let hour_window = window(root, DAY, "220000_3600", &[night()]);
        let short_window = window(root, DAY, "221000_300", &[night()]);
        record(root, json!({}));
        delete_for(&hour_window, hour);
        delete_for(&short_window, short);
        assert_eq!(composed(root)[0].account, expected, "{hour} {short}");
    }
}

/// A group whose member that isn't its anchor is hidden shows no line.
#[test]
fn the_home_leaves_out_a_group_whose_other_member_is_hidden() {
    let journal = journal();
    let root = journal.path();
    let watch = workout_row(
        "w",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 07:40:00 -0600",
    );
    let app = workout_row(
        "x",
        "Synthetic App",
        "2026-10-10 07:01:00 -0600",
        "2026-10-10 07:41:00 -0600",
    );
    window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
    window(root, DAY, "070000_300", std::slice::from_ref(&app));
    arrived_record(root, DAY, &watch, Some(DAY), json!({}));
    arrived_record(root, DAY, &app, Some(DAY), json!({}));
    let faces = day_faces(root, DAY).unwrap();
    assert_eq!(faces.len(), 1);
    let other = if faces[0].anchor.id == "body_w" {
        "body_x"
    } else {
        "body_w"
    };
    let path = root.join(format!("facets/body/activities/{DAY}.jsonl"));
    let text = fs::read_to_string(&path).unwrap().replace(
        &format!("\"id\":\"{other}\""),
        &format!("\"id\":\"{other}\",\"hidden\":true"),
    );
    fs::write(&path, text).unwrap();
    assert!(day_ids(root, DAY).is_empty());
}

/// A window the home can't read costs the section: no line is guessed.
#[test]
fn the_home_reads_no_lines_when_a_window_cant_be_read() {
    let journal = journal_fixture_with_live_night();
    let root = journal.path();
    let start = root
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("220000_3600");
    fs::write(start.join(BODY_RECORDS_FILE), "not json\n").unwrap();
    assert!(day_lines(root, DAY).is_err());
}

/// The home's clock is the owner's: a workout an import placed on the next day
/// by its own zone is the owner's evening, and its times are said in the
/// owner's zone, not the source's offset.
#[test]
fn the_home_says_a_line_on_the_owners_day_and_clock() {
    let journal = journal();
    let root = journal.path();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"Europe/Lisbon"}"#,
    )
    .unwrap();
    let late = workout_row(
        "l",
        "Synthetic Watch",
        "2026-10-11 02:00:00 +0100",
        "2026-10-11 02:30:00 +0100",
    );
    window(root, NEXT, "020000_3600", std::slice::from_ref(&late));
    arrived_record(root, NEXT, &late, Some(NEXT), json!({}));
    assert!(day_ids(root, NEXT).is_empty());
    let lines = lines_on(root, DAY);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["start"], at("19:00", None));
    assert_eq!(lines[0]["end"], at("19:30", None));
}

#[test]
fn a_distance_reads_as_a_person_says_it() {
    for (value, said) in [
        ("5.0 km", Some("5.0 km")),
        ("5.123456 km", Some("5.1 km")),
        ("3.14159 mi", Some("3.1 mi")),
        ("2412.9 m", Some("2.4 km")),
        ("1000 m", Some("1.0 km")),
        ("999.6 m", Some("1.0 km")),
        ("850.4 m", Some("850 m")),
        ("0.6 m", Some("1 m")),
        ("1760 yd", Some("1760 yd")),
        ("5280.4 ft", Some("5280 ft")),
        ("12.34 km", Some("12.3 km")),
        ("0 m", None),
        ("0.0 km", None),
        ("0.04 km", None),
        ("0.4 m", None),
        ("-3 km", None),
        ("2412.9", None),
        ("", None),
        ("far km", None),
        ("NaN km", None),
        ("5 km extra", None),
    ] {
        assert_eq!(line_distance(value).as_deref(), said, "{value:?}");
    }
}

#[test]
fn a_workout_that_grows_into_a_group_reads_as_one_on_the_home_and_the_day_view_keeping_its_title() {
    for refresh in [false, true] {
        let journal = journal();
        let root = journal.path();
        let mut watch = workout_row(
            "w",
            "Synthetic Watch",
            "2026-10-10 07:00:00 -0600",
            "2026-10-10 07:20:00 -0600",
        );
        let app = workout_row(
            "x",
            "Synthetic App",
            "2026-10-10 07:00:00 -0600",
            "2026-10-10 07:40:00 -0600",
        );
        arrived_record(root, DAY, &watch, Some(DAY), json!({}));
        arrived_record(root, DAY, &app, Some(DAY), json!({}));
        let mut patch = Map::new();
        patch.insert("title".to_owned(), json!("Hill repeats"));
        solstone_core_facets::update_activity_record(
            root,
            "body",
            DAY,
            "body_w",
            &patch,
            "owner:web",
            "",
            "2026-10-11T00:00:00Z",
        )
        .unwrap()
        .expect("titled");
        // A later export ends the watch's at 07:40, in its window.
        watch.end = Some("2026-10-10 07:40:00 -0600".to_owned());
        window(root, DAY, "070000_3600", std::slice::from_ref(&watch));
        window(root, DAY, "070000_300", std::slice::from_ref(&app));
        if refresh {
            let outcome = solstone_core_facets::refresh_body_account(
                root,
                "body",
                DAY,
                "body_w",
                "workout",
                "2026-10-10 07:00:00 -0600",
                "2026-10-10 07:40:00 -0600",
                &solstone_core_format::content::body_records::recorder_id(
                    STREAM,
                    Some("Synthetic Watch"),
                ),
                Some("running"),
            )
            .unwrap();
            assert_eq!(outcome, solstone_core_facets::BodyAccountRefresh::Refreshed);
        }
        let faces = body_faces_range(root, "body", DAY, DAY, None, true).unwrap();
        let sizes: Vec<usize> = faces[DAY].iter().map(|face| face.members.len()).collect();
        let lines = lines_on(root, DAY);
        if refresh {
            assert_eq!(sizes, [2]);
            assert_eq!(lines.len(), 1);
            // The owner's title still names the group.
            assert_eq!(lines[0]["name"], "Hill repeats");
            assert_eq!(
                lines[0]["from"],
                json!(["Synthetic Watch", "Synthetic App"])
            );
        } else {
            assert_eq!(sizes, [1, 1]);
            assert_eq!(lines.len(), 2);
        }
    }
}

const OURA: &str = "import.oura";

/// [`journal`], with Oura placed on the same zone as Apple.
fn journal_with_oura() -> tempfile::TempDir {
    let journal = journal();
    fs::write(
        journal.path().join("imports/.oura-zone.json"),
        r#"{"zone":"America/Denver"}"#,
    )
    .unwrap();
    journal
}

/// A window in any body stream.
fn window_in(root: &Path, stream: &str, day: &str, key: &str, rows: &[BodyRow]) -> PathBuf {
    let dir = root.join("chronicle").join(day).join(stream).join(key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(BODY_RECORDS_FILE), encode_rows(rows)).unwrap();
    dir
}

/// A record for an account in any body stream, as the writer files it: its
/// stream's recorder, and a workout's type in the shared vocabulary.
fn record_in(root: &Path, stream: &str, day: &str, row: &BodyRow) {
    use solstone_core_format::content::body_records::{mapped_workout_type, recorder_id};
    let mut account = json!({"key": format!("{stream}\u{1f}{}", row.id), "start": row.start,
        "end": row.end, "recorder": recorder_id(stream, row.by.as_deref()), "arrived": day});
    if row.experience.as_deref() == Some("workout")
        && let Some(kind) = mapped_workout_type(&row.source, &row.record_type)
    {
        account["type"] = json!(kind);
    }
    let record = json!({"id": format!("body_{}", row.id), "source": "body",
        "activity": row.experience, "account": account});
    let path = root.join(format!("facets/body/activities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(path, text).unwrap();
}

/// An experience as Oura's reader writes it, the whole document in `fields`.
fn oura_experience(
    id: &str,
    experience: &str,
    record_type: &str,
    label: &str,
    (start, end): (&str, &str),
    document: serde_json::Value,
) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Experience,
        id.to_owned(),
        "oura",
        record_type,
        label.to_owned(),
    );
    row.experience = Some(experience.to_owned());
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row.by = Some("Oura".to_owned());
    row.fields = document
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    row
}

fn oura_walk(source: &str) -> BodyRow {
    let mut walk = oura_experience(
        "ow",
        "workout",
        "walking",
        "walking",
        (
            "2026-10-10T08:05:00.000-06:00",
            "2026-10-10T08:41:00.000-06:00",
        ),
        json!({"id": "synthetic-workout-walk", "activity": "walking", "calories": 148.5,
            "distance": 2412.9, "intensity": "hard", "label": null, "source": source}),
    );
    walk.values
        .insert("distance".to_owned(), "2412.9 m".to_owned());
    walk.values
        .insert("energy".to_owned(), "148.5 kcal".to_owned());
    walk
}

#[test]
fn an_oura_line_says_its_distance_as_a_person_says_it_and_nothing_else() {
    let journal = journal_with_oura();
    let root = journal.path();
    let walk = oura_walk("confirmed");
    window_in(root, OURA, DAY, "080500_300", std::slice::from_ref(&walk));
    record_in(root, OURA, DAY, &walk);
    let lines = lines_on(root, DAY);
    assert_eq!(
        lines,
        [
            json!({"name": "walking", "start": at("08:05", None), "end": at("08:41", None),
            "distance": "2.4 km", "from": ["Oura"], "logged": false, "part_removed": false})
        ]
    );
    let text = lines[0].to_string();
    for statistic in ["kcal", "148", "2412", "intensity", "hard", "confirmed"] {
        assert!(!text.contains(statistic), "{statistic}: {text}");
    }
}

#[test]
fn a_workout_added_by_hand_in_oura_reads_logged_and_one_oura_detected_doesnt() {
    for (source, logged, from) in [
        ("manual", true, json!([])),
        ("autodetected", false, json!(["Oura"])),
    ] {
        let journal = journal_with_oura();
        let root = journal.path();
        let walk = oura_walk(source);
        window_in(root, OURA, DAY, "080500_300", std::slice::from_ref(&walk));
        record_in(root, OURA, DAY, &walk);
        let line = &lines_on(root, DAY)[0];
        assert_eq!(line["logged"], logged, "{source}: {line}");
        assert_eq!(line["from"], from, "{source}: {line}");
    }
}

/// A watch's run from 07:00 to 08:00, in Apple's stream.
fn watch_run() -> BodyRow {
    workout_row(
        "wr",
        "Synthetic Watch",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 08:00:00 -0600",
    )
}

/// Faces from Oura's stream and Apple's side by side, read through the range
/// and the home: each day's faces as their account ids, in any order, and how
/// many lines the home shows that day.
fn faces_across_streams(root: &Path) -> (Vec<Vec<Vec<String>>>, Vec<usize>) {
    let range = body_faces_range(root, "body", DAY, NEXT, None, true).unwrap();
    let faces = [DAY, NEXT]
        .iter()
        .map(|day| {
            range
                .get(*day)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|face| {
                    face.members
                        .iter()
                        .map(|member| member.id.clone())
                        .collect()
                })
                .collect::<std::collections::BTreeSet<Vec<String>>>()
                .into_iter()
                .collect()
        })
        .collect();
    let lines = [DAY, NEXT]
        .iter()
        .map(|day| lines_on(root, day).len())
        .collect();
    (faces, lines)
}

#[test]
fn an_oura_night_and_a_watchs_night_of_the_same_sleep_group() {
    let journal = journal_with_oura();
    let root = journal.path();
    // 22:30 to 06:30 beside 22:10 to 06:20: 470 of 480 and 490 minutes.
    let ring = oura_experience(
        "on",
        "sleep",
        "long_sleep",
        "long sleep",
        (
            "2026-10-10T22:30:00.000-06:00",
            "2026-10-11T06:30:00.000-06:00",
        ),
        json!({"id": "synthetic-night", "type": "long_sleep", "efficiency": 91,
            "readiness": {"score": 82}}),
    );
    window_in(root, OURA, DAY, "220000_3600", std::slice::from_ref(&ring));
    record_in(root, OURA, DAY, &ring);
    window(root, DAY, "220000_3600", &[night()]);
    record_in(root, STREAM, DAY, &night());
    let (faces, lines) = faces_across_streams(root);
    assert_eq!(faces, [vec![vec!["body_n1", "body_on"]], vec![]]);
    assert_eq!(lines, [1, 0]);
    let line = &lines_on(root, DAY)[0];
    assert_eq!(line["name"], "sleep");
    assert_eq!(line["from"], json!(["Synthetic Watch", "Oura"]), "{line}");
    for judged in ["91", "82", "score", "efficiency"] {
        assert!(!line.to_string().contains(judged), "{judged}: {line}");
    }
}

#[test]
fn an_oura_nap_inside_a_watchs_night_stands_alone() {
    let journal = journal_with_oura();
    let root = journal.path();
    // All of the nap, an eighth of the night.
    let nap = oura_experience(
        "op",
        "sleep",
        "nap",
        "nap",
        (
            "2026-10-11T02:00:00.000-06:00",
            "2026-10-11T03:00:00.000-06:00",
        ),
        json!({"id": "synthetic-nap", "type": "nap", "mood": "bad"}),
    );
    window_in(root, OURA, NEXT, "020000_3600", std::slice::from_ref(&nap));
    record_in(root, OURA, NEXT, &nap);
    window(root, DAY, "220000_3600", &[night()]);
    record_in(root, STREAM, DAY, &night());
    let (faces, lines) = faces_across_streams(root);
    assert_eq!(faces, [vec![vec!["body_n1"]], vec![vec!["body_op"]]]);
    assert_eq!(lines, [1, 1]);
}

#[test]
fn a_walk_oura_detected_inside_a_watchs_run_stands_alone() {
    let journal = journal_with_oura();
    let root = journal.path();
    // 07:10 to 07:50: all of the walk and two thirds of the run, so only the
    // type keeps them apart.
    let walk = oura_experience(
        "ow",
        "workout",
        "walking",
        "walking",
        (
            "2026-10-10T07:10:00.000-06:00",
            "2026-10-10T07:50:00.000-06:00",
        ),
        json!({"id": "synthetic-walk", "activity": "walking", "source": "autodetected"}),
    );
    window_in(root, OURA, DAY, "071000_300", std::slice::from_ref(&walk));
    record_in(root, OURA, DAY, &walk);
    window(root, DAY, "070000_3600", &[watch_run()]);
    record_in(root, STREAM, DAY, &watch_run());
    let (faces, lines) = faces_across_streams(root);
    assert_eq!(faces, [vec![vec!["body_ow"], vec!["body_wr"]], vec![]]);
    assert_eq!(lines, [2, 0]);
}

#[test]
fn an_oura_run_over_half_the_watchs_run_stands_alone_and_over_two_thirds_groups() {
    for (end, grouped) in [("07:30", false), ("07:40", true)] {
        let journal = journal_with_oura();
        let root = journal.path();
        let run = oura_experience(
            "or",
            "workout",
            "running",
            "running",
            (
                "2026-10-10T07:00:00.000-06:00",
                &format!("2026-10-10T{end}:00.000-06:00"),
            ),
            json!({"id": "synthetic-run", "activity": "running", "source": "autodetected"}),
        );
        window_in(root, OURA, DAY, "070000_300", std::slice::from_ref(&run));
        record_in(root, OURA, DAY, &run);
        window(root, DAY, "070000_3600", &[watch_run()]);
        record_in(root, STREAM, DAY, &watch_run());
        let (faces, lines) = faces_across_streams(root);
        if grouped {
            assert_eq!(faces, [vec![vec!["body_wr", "body_or"]], vec![]], "{end}");
            assert_eq!(lines, [1, 0], "{end}");
        } else {
            assert_eq!(
                faces,
                [vec![vec!["body_or"], vec!["body_wr"]], vec![]],
                "{end}"
            );
            assert_eq!(lines, [2, 0], "{end}");
        }
    }
}

/// The home says a day's lines in time order whichever source wrote them, though
/// Apple and Oura write their times in different forms.
#[test]
fn the_home_says_oura_and_apple_lines_in_time_order() {
    let journal = journal_with_oura();
    let root = journal.path();
    let walk = oura_experience(
        "ow",
        "workout",
        "walking",
        "walking",
        (
            "2026-10-10T06:00:00.000-06:00",
            "2026-10-10T06:30:00.000-06:00",
        ),
        json!({"id": "synthetic-walk", "activity": "walking", "source": "autodetected"}),
    );
    window_in(root, OURA, DAY, "060000_300", std::slice::from_ref(&walk));
    record_in(root, OURA, DAY, &walk);
    window(root, DAY, "070000_3600", &[watch_run()]);
    record_in(root, STREAM, DAY, &watch_run());
    let names: Vec<serde_json::Value> = lines_on(root, DAY)
        .iter()
        .map(|line| line["name"].clone())
        .collect();
    assert_eq!(names, [json!("walking"), json!("running")]);
}

/// What a night that grows into another recorder's night is kept as.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kept {
    Plain,
    Hidden,
    Apart,
}

/// Add `extra` to the stored record `id` on `day`.
fn patch_record(root: &Path, day: &str, id: &str, extra: serde_json::Value) {
    let path = root.join(format!("facets/body/activities/{day}.jsonl"));
    let text = fs::read_to_string(&path).unwrap();
    let mut patched = String::new();
    for line in text.lines() {
        let mut record: serde_json::Value = serde_json::from_str(line).unwrap();
        if record["id"] == id {
            for (key, value) in extra.as_object().unwrap() {
                record[key] = value.clone();
            }
        }
        patched.push_str(&format!("{record}\n"));
    }
    fs::write(path, patched).unwrap();
}

#[test]
fn a_night_that_grows_into_another_recorders_keeps_its_hide_and_apart_in_every_reader() {
    for kept in [Kept::Plain, Kept::Hidden, Kept::Apart] {
        let journal = journal_with_oura();
        let root = journal.path();
        // The watch's night ends at 02:00: all of it inside Oura's, but under
        // half of Oura's, so apart.
        let mut watch = night();
        watch.end = Some("2026-10-11 02:00:00 -0600".to_owned());
        let ring = oura_experience(
            "on",
            "sleep",
            "long_sleep",
            "long sleep",
            (
                "2026-10-10T22:12:00.000-06:00",
                "2026-10-11T06:18:00.000-06:00",
            ),
            json!({"type": "long_sleep", "efficiency": 93, "readiness": {"score": 82}}),
        );
        window(root, DAY, "220000_3600", std::slice::from_ref(&watch));
        window_in(root, OURA, DAY, "220000_3600", std::slice::from_ref(&ring));
        record_in(root, STREAM, DAY, &watch);
        record_in(root, OURA, DAY, &ring);
        match kept {
            Kept::Plain => {}
            Kept::Hidden => patch_record(root, DAY, "body_n1", json!({"hidden": true})),
            Kept::Apart => patch_record(
                root,
                DAY,
                "body_n1",
                json!({"apart": [format!("{OURA}\u{1f}on")]}),
            ),
        }
        // A later export ends the watch's night at 06:20, and its record follows.
        watch.end = Some(END.to_owned());
        window(root, DAY, "220000_3600", std::slice::from_ref(&watch));
        let outcome = solstone_core_facets::refresh_body_account(
            root,
            "body",
            DAY,
            "body_n1",
            "sleep",
            START,
            END,
            &solstone_core_format::content::body_records::recorder_id(
                STREAM,
                Some("Synthetic Watch"),
            ),
            None,
        )
        .unwrap();
        assert_eq!(outcome, solstone_core_facets::BodyAccountRefresh::Refreshed);

        // The owner's day view, hidden faces included and not.
        let shown = |include_hidden: bool| -> Vec<(usize, bool)> {
            body_faces_range(root, "body", DAY, DAY, None, include_hidden)
                .unwrap()
                .remove(DAY)
                .unwrap_or_default()
                .iter()
                .map(|face| (face.members.len(), face.hidden))
                .collect()
        };
        // A reader gated on windows, as a connection's is.
        let gated: Vec<usize> = body_faces(root, "body", DAY, &everyone, false)
            .unwrap()
            .iter()
            .map(|face| face.members.len())
            .collect();
        let lines = lines_on(root, DAY);
        match kept {
            Kept::Plain => {
                assert_eq!(shown(true), [(2, false)]);
                assert_eq!(gated, [2]);
                assert_eq!(lines.len(), 1, "{lines:?}");
            }
            // Grouped now, the ring's night is hidden with the watch's.
            Kept::Hidden => {
                assert_eq!(shown(true), [(2, true)]);
                assert!(shown(false).is_empty());
                assert!(gated.is_empty());
                assert!(lines.is_empty(), "{lines:?}");
            }
            Kept::Apart => {
                assert_eq!(shown(true), [(1, false), (1, false)]);
                assert_eq!(gated, [1, 1]);
                assert_eq!(lines.len(), 2, "{lines:?}");
            }
        }

        // The day's document is built from the windows, which hold both nights
        // whatever the records say, and it never groups.
        crate::body_day::BodyDays::default().reconcile(root);
        let document = fs::read_to_string(
            root.join("chronicle")
                .join(DAY)
                .join(crate::body_day::BODY_DOCUMENT),
        )
        .unwrap();
        for line in [
            "- sleep: Sleep, 22:10 to 2026-10-11 06:20 (Synthetic Watch)",
            "- sleep: long sleep, 22:12 to 2026-10-11 06:18 (Oura)",
        ] {
            assert!(document.contains(line), "{kept:?}: {document}");
        }
    }
}

const STRAVA: &str = "import.strava";

fn journal_with_strava() -> tempfile::TempDir {
    let journal = journal();
    fs::write(
        journal.path().join("imports/.strava-zone.json"),
        "{\"zone\":\"America/Denver\"}\n",
    )
    .unwrap();
    journal
}

/// One piece of a Strava workout, as the Strava import writes it, and the row
/// the body readers see it as.
fn strava_tile(
    root: &Path,
    day: &str,
    key: &str,
    (activity_id, index, count): (u64, u64, u64),
    (record_type, start, elapsed): (&str, &str, u64),
) -> (PathBuf, BodyRow) {
    let text = json!({
        "schema": "solstone.import.strava.tile.v1",
        "import_id": "20261010_120000",
        "activity_id": activity_id,
        "zone": "America/Denver",
        "tile": {"index": index, "count": count, "start": start, "end": start, "seconds": 300},
        "workout": {"name": "Lunch Run", "type": record_type, "start": start,
            "elapsed_seconds": elapsed, "distance_m": 8100.0, "commute": false,
            "entered_by_hand": false,
            "strava_fields": {"Activity Private Note": "synthetic note"}},
    })
    .to_string();
    let dir = root.join("chronicle").join(day).join(STRAVA).join(key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("workout.json"), &text).unwrap();
    let row = solstone_core_format::content::strava_body::strava_rows(&text)
        .unwrap()
        .remove(0);
    (dir, row)
}

#[test]
fn a_journal_holds_body_data_once_strava_placed_a_tile() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    assert!(!has_body_data(root));
    fs::write(
        root.join("imports/.strava-zone.json"),
        "{\"zone\":\"America/Denver\"}\n",
    )
    .unwrap();
    assert!(has_body_data(root));
}

#[test]
fn the_strava_zone_is_the_one_its_first_import_fixed() {
    let journal = journal_with_strava();
    assert_eq!(
        solstone_core_journal_config::placed_zone(journal.path(), "strava"),
        Some(chrono_tz::America::Denver)
    );
}

#[test]
fn a_strava_record_composes_from_its_tile_live_removed_by_you_or_unreadable() {
    let journal = journal_with_strava();
    let root = journal.path();
    let (first, run) = strava_tile(
        root,
        DAY,
        "070000_300",
        (7001, 0, 2),
        ("Run", "2026-10-10T07:00:00-06:00", 540),
    );
    let (later, piece) = strava_tile(
        root,
        DAY,
        "070500_240",
        (7001, 1, 2),
        ("Run", "2026-10-10T07:00:00-06:00", 540),
    );
    record_in(root, STRAVA, DAY, &run);
    let found = composed(root);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].stream, STRAVA);
    assert_eq!(found[0].window.as_deref(), Some("070000_300"));
    assert_eq!(found[0].account, Account::Live(Box::new(run.clone())));
    let account =
        &face_json(&body_faces(root, "body", DAY, &everyone, true).unwrap()[0])["accounts"][0];
    assert_eq!(account["state"], "live");
    assert_eq!(account["source"], "strava");
    assert_eq!(account["by"], "Strava");
    assert_eq!(account["values"]["distance"], "8100.0 m");
    // The owner deletes the first piece: the later one still says the span.
    fs::remove_file(first.join("workout.json")).unwrap();
    fs::write(
        first.join(TOMBSTONE),
        json!({"reason": "owner_segment_delete", "manifest": ["workout.json"]}).to_string(),
    )
    .unwrap();
    assert_eq!(
        composed(root)[0].account,
        Account::PartRemoved(Box::new(piece))
    );
    // And the rest of it: removed by the owner.
    fs::remove_file(later.join("workout.json")).unwrap();
    fs::write(
        later.join(TOMBSTONE),
        json!({"reason": "owner_segment_delete", "manifest": ["workout.json"]}).to_string(),
    )
    .unwrap();
    assert_eq!(composed(root)[0].account, Account::RemovedByYou);
    // A first piece that isn't a tile any more can't be read.
    fs::remove_file(first.join(TOMBSTONE)).unwrap();
    fs::write(first.join("workout.json"), "{not a tile").unwrap();
    assert_eq!(composed(root)[0].account, Account::Unreadable);
}

/// A body.jsonl in a Strava tile's folder isn't what the tile holds.
#[test]
fn a_strava_record_reads_the_lowest_key_the_strava_importer_keeps_writing() {
    let journal = journal_with_strava();
    let root = journal.path();
    let (_, run) = strava_tile(
        root,
        DAY,
        "070000_300",
        (7001, 0, 1),
        ("Run", "2026-10-10T07:00:00-06:00", 300),
    );
    // An archive merge's copy, on the next key.
    strava_tile(
        root,
        DAY,
        "070000_301",
        (7001, 0, 1),
        ("Run", "2026-10-10T07:00:00-06:00", 300),
    );
    record_in(root, STRAVA, DAY, &run);
    assert_eq!(composed(root)[0].window.as_deref(), Some("070000_300"));
}

#[test]
fn a_strava_window_is_read_by_its_own_file_only() {
    let journal = journal_with_strava();
    let root = journal.path();
    let (dir, run) = strava_tile(
        root,
        DAY,
        "070000_300",
        (7001, 0, 1),
        ("Run", "2026-10-10T07:00:00-06:00", 300),
    );
    record_in(root, STRAVA, DAY, &run);
    fs::remove_file(dir.join("workout.json")).unwrap();
    fs::write(dir.join(BODY_RECORDS_FILE), encode_rows(&[run])).unwrap();
    assert_eq!(composed(root)[0].account, Account::NotFound);
}

/// Faces on `DAY` as their account ids, in any order.
fn day_groups(root: &Path) -> Vec<Vec<String>> {
    let mut groups = body_faces(root, "body", DAY, &everyone, true)
        .unwrap()
        .iter()
        .map(|face| {
            let mut ids = face
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect::<Vec<_>>();
            ids.sort();
            ids
        })
        .collect::<Vec<_>>();
    groups.sort();
    groups
}

#[test]
fn a_watch_run_its_strava_upload_and_stravas_write_back_to_apple_group_pairwise() {
    let journal = journal_with_strava();
    let root = journal.path();
    let watch = watch_run();
    // Strava's copy of it, written back into Apple Health as Strava's own.
    let back = workout_row(
        "wb",
        "Strava",
        "2026-10-10 07:00:00 -0600",
        "2026-10-10 08:00:00 -0600",
    );
    window(root, DAY, "070000_3600", &[watch.clone(), back.clone()]);
    record_in(root, STREAM, DAY, &watch);
    record_in(root, STREAM, DAY, &back);
    let (_, strava) = strava_tile(
        root,
        DAY,
        "070000_300",
        (7002, 0, 12),
        ("Run", "2026-10-10T07:00:00-06:00", 3590),
    );
    record_in(root, STRAVA, DAY, &strava);
    let stored = stored_body_records(root, "body", DAY).unwrap();
    assert_eq!(stored.len(), 3);
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert!(pair(&stored[left], &stored[right]), "{left} {right}");
    }
    assert_eq!(day_groups(root), [vec!["body_7002", "body_wb", "body_wr"]]);
    assert_eq!(lines_on(root, DAY).len(), 1);
}

#[test]
fn a_strava_phone_walk_beside_a_watch_run_stands_alone() {
    let journal = journal_with_strava();
    let root = journal.path();
    window(root, DAY, "070000_3600", &[watch_run()]);
    record_in(root, STREAM, DAY, &watch_run());
    // 07:10 to 07:50: only the type keeps them apart.
    let (_, walk) = strava_tile(
        root,
        DAY,
        "071000_300",
        (7003, 0, 8),
        ("Walk", "2026-10-10T07:10:00-06:00", 2400),
    );
    record_in(root, STRAVA, DAY, &walk);
    assert_eq!(day_groups(root), [vec!["body_7003"], vec!["body_wr"]]);
}

#[test]
fn a_strava_phone_run_twice_the_watchs_stands_alone() {
    let journal = journal_with_strava();
    let root = journal.path();
    let watch = workout_row(
        "wr",
        "Synthetic Watch",
        "2026-10-10 12:20:00 -0600",
        "2026-10-10 12:50:00 -0600",
    );
    window(root, DAY, "120000_3600", std::slice::from_ref(&watch));
    record_in(root, STREAM, DAY, &watch);
    // 12:00 to 13:00: all of the watch's run, half of the phone's.
    let (_, phone) = strava_tile(
        root,
        DAY,
        "120000_300",
        (7004, 0, 12),
        ("Run", "2026-10-10T12:00:00-06:00", 3600),
    );
    record_in(root, STRAVA, DAY, &phone);
    assert_eq!(day_groups(root), [vec!["body_7004"], vec!["body_wr"]]);
}

#[test]
fn an_open_move_shows_a_record_from_one_side_only() {
    let journal = self::journal();
    let root = journal.path();
    window(root, DAY, "220000_3600", &[night()]);
    record(root, json!({}));
    let stored = stored_body_records(root, "body", DAY).unwrap();
    assert_eq!(stored.len(), 1);
    let raw = fs::read_to_string(root.join(format!("facets/body/activities/{DAY}.jsonl"))).unwrap();
    let row: serde_json::Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
    let intent = json!({"move": "m", "key": row["account"]["key"], "record": row["id"],
        "from": {"day": "20261001", "facet": "other"},
        "to": {"day": DAY, "facet": "body"}, "copy": row});
    fs::write(
        root.join("imports/.body-moves.jsonl"),
        format!("{intent}\n"),
    )
    .unwrap();
    assert!(stored_body_records(root, "body", DAY).unwrap().is_empty());
    let placed = json!({"placed": "m", "appended": true});
    fs::write(
        root.join("imports/.body-moves.jsonl"),
        format!("{intent}\n{placed}\n"),
    )
    .unwrap();
    assert_eq!(stored_body_records(root, "body", DAY).unwrap().len(), 1);
}
