// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Oura revisions that move a document within its owner day: every live row of it
//! converges to its new tiling, and every other live row becomes `moved`. A move
//! across the owner's midnight is kept as first written.

#![allow(clippy::disallowed_methods)]

use solstone_core_location::body_records::{Account, compose_body_records, day_faces};

use super::*;

/// Every body record of `day`, across every facet, as stored.
fn records(root: &Path, day: &str) -> Vec<Value> {
    let mut found = Vec::new();
    for facet in fs::read_dir(root.join("facets"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = facet.path().join("activities").join(format!("{day}.jsonl"));
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        found.extend(
            text.lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap()),
        );
    }
    found
}

fn everyone(_: &str, _: &str, _: Option<&str>) -> bool {
    true
}

/// The owner's own edits on a day's one record, as an owner command writes them.
fn edit_record(root: &Path, day: &str) {
    let path = root
        .join("facets/health/activities")
        .join(format!("{day}.jsonl"));
    let text = fs::read_to_string(&path).unwrap();
    let mut record: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    record["title"] = json!("Synthetic owner title");
    record["description"] = json!("synthetic owner words");
    fs::write(&path, format!("{record}\n")).unwrap();
}

/// What every reader says of a day's one account: the window it composes from and
/// what it found there.
fn account_of(root: &Path, day: &str) -> (Option<String>, Account) {
    let composed = compose_body_records(root, "health", day, &everyone, true).unwrap();
    assert_eq!(composed.len(), 1, "{composed:?}");
    let faces = day_faces(root, day).unwrap();
    assert_eq!(faces.len(), 1, "{faces:?}");
    assert_eq!(faces[0].members[0].window, composed[0].window);
    (composed[0].window.clone(), composed[0].account.clone())
}

struct Case {
    name: &'static str,
    endpoint: &'static str,
    id: &'static str,
    first: Value,
    revised: Value,
    windows: Vec<(String, String)>,
}

fn cases() -> Vec<Case> {
    let night_from = |start: &str, end: &str| sleep("n1", "long_sleep", start, end);
    let mut night = windows_from("20260105", (23, 0), 3600, 1);
    night.extend(windows_from("20260106", (0, 0), 3600, 7));
    let mut shorter = windows_from("20260105", (22, 0), 3600, 2);
    shorter.extend(windows_from("20260106", (0, 0), 3600, 6));
    let mut longer = windows_from("20260105", (22, 0), 3600, 2);
    longer.extend(windows_from("20260106", (0, 0), 3600, 8));
    vec![
        Case {
            name: "a workout five minutes later",
            endpoint: "workout",
            id: "w1",
            first: workout(
                "w1",
                "running",
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:30:00.000-07:00",
            ),
            revised: workout(
                "w1",
                "running",
                "2026-01-05T08:05:00.000-07:00",
                "2026-01-05T08:35:00.000-07:00",
            ),
            windows: windows_from("20260105", (8, 5), 300, 6),
        },
        Case {
            name: "a night's start an hour later on its day",
            endpoint: "sleep",
            id: "n1",
            first: night_from(
                "2026-01-05T22:10:00.000-07:00",
                "2026-01-06T06:30:00.000-07:00",
            ),
            revised: night_from(
                "2026-01-05T23:10:00.000-07:00",
                "2026-01-06T06:30:00.000-07:00",
            ),
            windows: night,
        },
        Case {
            name: "a night's end earlier",
            endpoint: "sleep",
            id: "n1",
            first: night_from(
                "2026-01-05T22:10:00.000-07:00",
                "2026-01-06T07:40:00.000-07:00",
            ),
            revised: night_from(
                "2026-01-05T22:10:00.000-07:00",
                "2026-01-06T05:20:00.000-07:00",
            ),
            windows: shorter,
        },
        Case {
            name: "a night's end later",
            endpoint: "sleep",
            id: "n1",
            first: night_from(
                "2026-01-05T22:10:00.000-07:00",
                "2026-01-06T05:20:00.000-07:00",
            ),
            revised: night_from(
                "2026-01-05T22:10:00.000-07:00",
                "2026-01-06T07:40:00.000-07:00",
            ),
            windows: longer,
        },
        Case {
            name: "a meditation that becomes a nap",
            endpoint: "session",
            id: "s1",
            first: session(
                "s1",
                "meditation",
                "2026-01-05T13:00:00.000-07:00",
                "2026-01-05T13:20:00.000-07:00",
            ),
            revised: session(
                "s1",
                "nap",
                "2026-01-05T13:00:00.000-07:00",
                "2026-01-05T13:20:00.000-07:00",
            ),
            windows: windows_from("20260105", (13, 0), 3600, 1),
        },
        Case {
            name: "a tag at another hour",
            endpoint: "enhanced_tag",
            id: "t1",
            first: tag("t1", "2026-01-05T09:15:00-07:00"),
            revised: tag("t1", "2026-01-05T11:15:00-07:00"),
            windows: windows_from("20260105", (11, 0), 3600, 1),
        },
    ]
}

#[test]
fn every_move_within_the_owners_day_converges_to_its_new_tiling_and_its_one_record_follows() {
    for case in cases() {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let first = stage(
            &scratch.path().join("1"),
            &[(case.endpoint, vec![case.first.clone()])],
        );
        run(root, &first, "20261104_120000", Some("health"));
        let experience = case.endpoint != "enhanced_tag";
        if experience {
            edit_record(root, "20260105");
        }
        let owned = records(root, "20260105");
        let rid = oura_row_id(case.endpoint, case.id);
        let before = rows_of(root, &rid);
        let revised = stage(
            &scratch.path().join("2"),
            &[(case.endpoint, vec![case.revised.clone()])],
        );
        let outcome = run(root, &revised, "20261104_120001", Some("health"));
        assert!(
            outcome.holes.is_empty(),
            "{}: {:?}",
            case.name,
            outcome.holes
        );
        assert!(
            !outcome.summary.contains("keeps it as first written"),
            "{}: {}",
            case.name,
            outcome.summary
        );
        let expected = tiling(&rid, &as_refs(&case.windows));
        assert_converged(root, &rid, &before, &expected);
        let after = records(root, "20260105");
        if !experience {
            assert!(after.is_empty(), "{}: a tag has no record", case.name);
            continue;
        }
        // One record, its account at the new instants, the owner's fields as they were.
        assert_eq!(after.len(), 1, "{}: {after:?}", case.name);
        let new_start = case.revised["start_datetime"]
            .as_str()
            .or(case.revised["bedtime_start"].as_str())
            .unwrap();
        assert_eq!(after[0]["account"]["start"], new_start, "{}", case.name);
        for field in ["id", "title", "description", "account"] {
            if field == "account" {
                assert_eq!(
                    after[0]["account"]["key"], owned[0]["account"]["key"],
                    "{}",
                    case.name
                );
                continue;
            }
            assert_eq!(after[0][field], owned[0][field], "{}: {field}", case.name);
        }
        // Every reader shows one face, at its new window, and finds it there.
        let (window, account) = account_of(root, "20260105");
        assert_eq!(
            window.as_deref(),
            Some(case.windows[0].1.as_str()),
            "{}",
            case.name
        );
        assert!(
            matches!(account, Account::Live(_)),
            "{}: {account:?}",
            case.name
        );
    }
}

/// Name every tile as an earlier build did, by its index. A journal holding those,
/// imported again with the same pages, ends with tiles named by their windows,
/// every index tile moved, one record, and nothing stale that reads. That build
/// kept no record of what Oura runs fetched, so the next sync starts one from the
/// rows the journal holds.
fn to_index_tiles(root: &Path) {
    fs::remove_file(root.join(solstone_core_import::oura_fetched::FETCHED_REL)).unwrap();
    for ((day, key), rows) in stored(root) {
        let mut rows: Vec<BodyRow> = rows
            .into_iter()
            .map(|mut row| {
                if row.kind == RowKind::ExperienceTile {
                    row.id = format!("{}-{}", row.of.as_deref().unwrap(), row.index.unwrap());
                }
                row
            })
            .collect();
        solstone_core_format::content::body_records::sort_rows(&mut rows);
        fs::write(
            window(root, &day, &key).join(BODY_RECORDS_FILE),
            solstone_core_format::content::body_records::encode_rows(&rows),
        )
        .unwrap();
    }
}

#[test]
fn windows_with_index_tiles_import_again_into_window_tiles_with_one_record_and_nothing_stale() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = stage(
        &scratch.path().join("1"),
        &[
            (
                "workout",
                vec![workout(
                    "w1",
                    "running",
                    "2026-01-05T08:00:00.000-07:00",
                    "2026-01-05T08:30:00.000-07:00",
                )],
            ),
            (
                "sleep",
                vec![sleep(
                    "n1",
                    "long_sleep",
                    "2026-01-05T22:10:00.000-07:00",
                    "2026-01-06T06:30:00.000-07:00",
                )],
            ),
        ],
    );
    run(root, &pages, "20261104_120000", Some("health"));
    let window_tiles = stream_bytes(root);
    to_index_tiles(root);
    let workout_id = oura_row_id("workout", "w1");
    let night_id = oura_row_id("sleep", "n1");
    let before: Vec<_> = [&workout_id, &night_id]
        .iter()
        .map(|rid| rows_of(root, rid))
        .collect();
    let again = run(root, &pages, "20261104_120001", Some("health"));
    assert!(again.holes.is_empty(), "{:?}", again.holes);
    // Only what its tiles are named changed, which is no time of Oura's.
    assert!(
        !again.summary.contains("has since changed"),
        "{}",
        again.summary
    );
    let mut night = windows_from("20260105", (22, 0), 3600, 2);
    night.extend(windows_from("20260106", (0, 0), 3600, 7));
    assert_converged(
        root,
        &workout_id,
        &before[0],
        &tiling(
            &workout_id,
            &as_refs(&windows_from("20260105", (8, 0), 300, 6)),
        ),
    );
    assert_converged(
        root,
        &night_id,
        &before[1],
        &tiling(&night_id, &as_refs(&night)),
    );
    for (rid, before) in [(&workout_id, &before[0]), (&night_id, &before[1])] {
        let marks = rows_of(root, rid)
            .into_values()
            .filter(|row| row.kind == RowKind::Moved)
            .count();
        let index_tiles = before
            .values()
            .filter(|row| row.kind == RowKind::ExperienceTile)
            .count();
        assert_eq!(marks, index_tiles);
    }
    assert_eq!(records(root, "20260105").len(), 2);
    // Nothing stale reads: the next morning continues the night once.
    solstone_core_location::body_day::BodyDays::default().reconcile(root);
    let next = fs::read_to_string(root.join("chronicle/20260106/body.md")).unwrap();
    assert_eq!(next.matches("continuing").count(), 1, "{next}");
    for day in ["20260105", "20260106"] {
        for (window, rows) in stored(root).into_iter().filter(|((on, _), _)| on == day) {
            let live: Vec<_> = rows
                .iter()
                .filter(|row| row.kind != RowKind::Moved)
                .collect();
            let text = live
                .iter()
                .map(|row| solstone_core_format::content::body_records::render_row(row))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.matches("part ").count() <= 1, "{window:?}: {text}");
        }
    }

    // The twin: unchanged since the window-keyed build, it changes nothing.
    let fresh = self::journal();
    run(fresh.path(), &pages, "20261104_120000", Some("health"));
    assert_eq!(stream_bytes(fresh.path()), window_tiles);
    let twice = run(fresh.path(), &pages, "20261104_120001", Some("health"));
    assert_eq!(twice.published, 0, "{}", twice.summary);
    assert_eq!(stream_bytes(fresh.path()), window_tiles);
}

fn night_move(dir: &Path, start: &str) -> PathBuf {
    stage(
        dir,
        &[(
            "sleep",
            vec![sleep(
                "n1",
                "long_sleep",
                start,
                "2026-01-06T06:30:00.000-07:00",
            )],
        )],
    )
}

#[test]
fn a_first_window_deleted_after_the_scan_drops_the_move_on_every_day_through_a_redo() {
    // The night's old first window, at 22:00, or its new one, at 23:00.
    for (redo, deleted) in [
        (false, "220000_3600"),
        (true, "220000_3600"),
        (false, "230000_3600"),
    ] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        run(
            root,
            &night_move(&scratch.path().join("1"), "2026-01-05T22:10:00.000-07:00"),
            "20261104_120000",
            Some("health"),
        );
        let pages = night_move(&scratch.path().join("2"), "2026-01-05T23:10:00.000-07:00");
        let stop = stopping_at("scanned", 1);
        let mut stopped = request(&pages, "20261104_120001", Some("health"));
        stopped.stop_at = Some(&stop);
        run_oura_segments(root, &stopped).unwrap_err();
        delete(root, "20260105", deleted);
        let kept = stream_bytes(root);
        let records_before = records(root, "20260105");
        if redo {
            // Decided at the start day, then stopped before any of its files.
            let stop = stopping_at("pending", 1);
            let mut stopped = request(&pages, "20261104_120002", Some("health"));
            stopped.stop_at = Some(&stop);
            run_oura_segments(root, &stopped).unwrap_err();
        }
        let resumed = run(root, &pages, "20261104_120003", Some("health"));
        assert_eq!(stream_bytes(root), kept, "{redo}");
        assert!(
            resumed.summary.contains(
                "1 entry wasn't added, because you deleted the part of the journal it belongs to."
            ),
            "{redo}: {}",
            resumed.summary
        );
        assert!(
            !resumed.summary.contains("now shows"),
            "{}",
            resumed.summary
        );
        assert!(resumed.holes.is_empty(), "{:?}", resumed.holes);
        assert_eq!(records(root, "20260105"), records_before);
    }
}

#[test]
fn a_new_first_window_the_owner_deleted_keeps_the_move_out_and_stages_no_tile() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, start: &str, end: &str| {
        stage(
            &scratch.path().join(name),
            &[(
                "workout",
                vec![
                    workout("w1", "running", start, end),
                    workout(
                        "w2",
                        "walking",
                        "2026-01-05T08:40:00.000-07:00",
                        "2026-01-05T08:44:00.000-07:00",
                    ),
                ],
            )],
        )
    };
    run(
        root,
        &pages(
            "1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        None,
    );
    delete(root, "20260105", "084000_300");
    let kept = stream_bytes(root);
    let revised = pages(
        "2",
        "2026-01-05T08:40:00.000-07:00",
        "2026-01-05T09:10:00.000-07:00",
    );
    // The scan stages nothing of it: no move, no row, and it stayed deleted.
    let stop = stopping_at("scanned", 1);
    let mut stopped = request(&revised, "20261104_120001", None);
    stopped.stop_at = Some(&stop);
    run_oura_segments(root, &stopped).unwrap_err();
    let state: Value =
        serde_json::from_slice(&fs::read(root.join(OURA.run_dir_rel).join(STATE_FILE)).unwrap())
            .unwrap();
    assert!(state["findings"].get("converging").is_none(), "{state}");
    // It, and the workout whose window the owner deleted.
    assert_eq!(state["findings"]["stayed_deleted"], 2, "{state}");
    assert_eq!(state["scan"]["rows"], 0, "{state}");
    let outcome = run(root, &revised, "20261104_120002", None);
    assert_eq!(stream_bytes(root), kept);
    assert!(!window(root, "20260105", "084500_300").exists());
    assert!(
        outcome
            .summary
            .contains("entries weren't added, because you deleted"),
        "{}",
        outcome.summary
    );
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
}

#[test]
fn a_tile_window_the_owner_deleted_stays_deleted_and_the_rest_converges() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    run(
        root,
        &one_workout(
            &scratch.path().join("1"),
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        None,
    );
    delete(root, "20260105", "081000_300");
    let rid = oura_row_id("workout", "w1");
    let before = rows_of(root, &rid);
    let outcome = run(
        root,
        &one_workout(
            &scratch.path().join("2"),
            "2026-01-05T07:50:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120001",
        None,
    );
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
    let mut expected = tiling(&rid, &as_refs(&windows_from("20260105", (7, 50), 300, 8)));
    expected.retain(|(_, key, _)| key != "081000_300");
    assert_converged(root, &rid, &before, &expected);
    assert!(
        window(root, "20260105", "081000_300")
            .join("tombstone.json")
            .is_file()
    );
    assert!(
        !window(root, "20260105", "081000_300")
            .join(BODY_RECORDS_FILE)
            .exists()
    );
}

fn tag_pages(dir: &Path, start: &str) -> PathBuf {
    stage(dir, &[("enhanced_tag", vec![tag("t1", start)])])
}

#[test]
fn a_tag_the_owner_deleted_after_its_move_stays_out_wherever_oura_moves_it_next() {
    for back in [false, true] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        run(
            root,
            &tag_pages(&scratch.path().join("1"), "2026-01-05T09:15:00-07:00"),
            "20261104_120000",
            None,
        );
        run(
            root,
            &tag_pages(&scratch.path().join("2"), "2026-01-05T11:15:00-07:00"),
            "20261104_120001",
            None,
        );
        delete(root, "20260105", "110000_3600");
        let mark =
            fs::read(window(root, "20260105", "090000_3600").join(BODY_RECORDS_FILE)).unwrap();
        let next = if back {
            "2026-01-05T09:20:00-07:00"
        } else {
            "2026-01-05T13:15:00-07:00"
        };
        let outcome = run(
            root,
            &tag_pages(&scratch.path().join("3"), next),
            "20261104_120002",
            None,
        );
        // The sync fetched t1 before and the journal holds it nowhere live: the
        // owner deleted it, so it stays out, and its delete is no hole.
        assert!(
            outcome
                .summary
                .contains("1 entry wasn't added, because you deleted"),
            "{back}: {}",
            outcome.summary
        );
        assert!(
            !outcome.summary.contains("couldn't be read"),
            "{back}: {}",
            outcome.summary
        );
        assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
        let rid = oura_row_id("enhanced_tag", "t1");
        assert!(
            rows_of(root, &rid)
                .values()
                .all(|row| row.kind == RowKind::Moved),
            "{back}"
        );
        assert_eq!(
            fs::read(window(root, "20260105", "090000_3600").join(BODY_RECORDS_FILE)).unwrap(),
            mark
        );
        assert!(!window(root, "20260105", "130000_3600").exists());
    }
}

#[test]
fn a_workout_the_owner_deleted_after_its_move_stays_deleted_and_its_record_is_kept() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages =
        |name: &str, start: &str, end: &str| one_workout(&scratch.path().join(name), start, end);
    run(
        root,
        &pages(
            "1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        Some("health"),
    );
    run(
        root,
        &pages(
            "2",
            "2026-01-05T08:40:00.000-07:00",
            "2026-01-05T09:10:00.000-07:00",
        ),
        "20261104_120001",
        Some("health"),
    );
    delete(root, "20260105", "084000_300");
    let kept = stream_bytes(root);
    let recorded = records(root, "20260105");
    assert_eq!(
        recorded[0]["account"]["start"],
        "2026-01-05T08:40:00.000-07:00"
    );
    let outcome = run(
        root,
        &pages(
            "3",
            "2026-01-05T09:30:00.000-07:00",
            "2026-01-05T10:00:00.000-07:00",
        ),
        "20261104_120002",
        Some("health"),
    );
    assert_eq!(stream_bytes(root), kept);
    assert_eq!(records(root, "20260105"), recorded);
    assert!(
        outcome.summary.contains("1 entry wasn't added"),
        "{}",
        outcome.summary
    );
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
}

#[test]
fn moving_back_replaces_the_moved_row_and_keeps_the_run_that_first_wrote_it() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, start: &str| tag_pages(&scratch.path().join(name), start);
    run(
        root,
        &pages("1", "2026-01-05T09:15:00-07:00"),
        "20261104_120000",
        None,
    );
    run(
        root,
        &pages("2", "2026-01-05T11:15:00-07:00"),
        "20261104_120001",
        None,
    );
    run(
        root,
        &pages("3", "2026-01-05T09:30:00-07:00"),
        "20261104_120002",
        None,
    );
    let rid = oura_row_id("enhanced_tag", "t1");
    let rows = rows_of(root, &rid);
    let back = &rows[&("20260105".to_owned(), "090000_3600".to_owned(), rid.clone())];
    assert_eq!(back.kind, RowKind::Logged);
    assert_eq!(back.run.as_deref(), Some("20261104_120000"));
    let away = &rows[&("20260105".to_owned(), "110000_3600".to_owned(), rid.clone())];
    assert_eq!(away.kind, RowKind::Moved);
    assert_eq!(away.run.as_deref(), Some("20261104_120001"));
}

#[test]
fn a_stored_span_past_the_lookups_days_shortened_marks_every_old_tile() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, end: &str| {
        stage(
            &scratch.path().join(name),
            &[(
                "session",
                vec![session("s1", "nap", "2026-01-05T22:00:00.000-07:00", end)],
            )],
        )
    };
    run(
        root,
        &pages("1", "2026-01-09T06:00:00.000-07:00"),
        "20261104_120000",
        None,
    );
    let rid = oura_row_id("session", "s1");
    let before = rows_of(root, &rid);
    assert!(before.keys().any(|(day, ..)| day == "20260109"));
    let outcome = run(
        root,
        &pages("2", "2026-01-06T06:00:00.000-07:00"),
        "20261104_120001",
        None,
    );
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
    let mut windows = windows_from("20260105", (22, 0), 3600, 2);
    windows.extend(windows_from("20260106", (0, 0), 3600, 6));
    assert_converged(root, &rid, &before, &tiling(&rid, &as_refs(&windows)));
}

#[test]
fn a_reading_and_another_documents_row_in_a_marked_window_keep_their_bytes() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, start: &str, end: &str| {
        stage(
            &scratch.path().join(name),
            &[
                (
                    "workout",
                    vec![
                        workout("w1", "running", start, end),
                        workout(
                            "w2",
                            "walking",
                            "2026-01-05T08:10:00.000-07:00",
                            "2026-01-05T08:14:00.000-07:00",
                        ),
                    ],
                ),
                (
                    "heartrate",
                    vec![heart("2026-01-05T15:12:00.000Z", 141, "workout")],
                ),
            ],
        )
    };
    run(
        root,
        &pages(
            "1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        None,
    );
    let shared = ("20260105".to_owned(), "081000_300".to_owned());
    let others = |root: &Path| -> Vec<BodyRow> {
        stored(root)[&shared]
            .iter()
            .filter(|row| row.of.as_deref() != Some(oura_row_id("workout", "w1").as_str()))
            .cloned()
            .collect()
    };
    let kept = others(root);
    assert_eq!(kept.len(), 2, "{kept:?}");
    run(
        root,
        &pages(
            "2",
            "2026-01-05T09:00:00.000-07:00",
            "2026-01-05T09:30:00.000-07:00",
        ),
        "20261104_120001",
        None,
    );
    let rows = &stored(root)[&shared];
    assert!(
        rows.iter().any(|row| row.kind == RowKind::Moved),
        "{rows:?}"
    );
    assert_eq!(others(root), kept);
}

#[test]
fn a_finished_move_and_one_across_midnight_on_one_oura_day_leave_it_a_deferred_hole() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, moved: (&str, &str), crossing: &str| {
        stage(
            &scratch.path().join(name),
            &[(
                "workout",
                vec![
                    workout("w1", "running", moved.0, moved.1),
                    workout("w2", "walking", crossing, "2026-01-06T00:20:00.000-07:00"),
                ],
            )],
        )
    };
    let mut first = pages(
        "1",
        (
            "2026-01-06T08:00:00.000-07:00",
            "2026-01-06T08:30:00.000-07:00",
        ),
        "2026-01-05T23:40:00.000-07:00",
    );
    run(root, &first, "20261104_120000", None);
    first = pages(
        "2",
        (
            "2026-01-06T08:40:00.000-07:00",
            "2026-01-06T09:10:00.000-07:00",
        ),
        "2026-01-06T00:10:00.000-07:00",
    );
    let outcome = run(root, &first, "20261104_120001", None);
    assert_eq!(
        outcome.holes,
        [Hole {
            endpoint: "workout".to_owned(),
            at: "2026-01-06".to_owned(),
            reason: HoleReason::Deferred,
        }]
    );
    assert!(
        outcome
            .summary
            .contains("Oura has since changed the time of 1 entry"),
        "{}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains(
            "Oura moved 1 entry to the other side of midnight; your journal keeps the time"
        ),
        "{}",
        outcome.summary
    );
}

/// A journal on a fixed zone of Europe/Berlin, whose wearer's own days are New
/// York's.
fn berlin() -> tempfile::TempDir {
    let journal = journal();
    fs::write(
        journal.path().join("config/journal.json"),
        json!({"identity": {"timezone": "Europe/Berlin"}}).to_string(),
    )
    .unwrap();
    journal
}

#[test]
fn travel_moves_within_the_fixed_zones_day_and_keeps_a_split_of_it_as_first_written() {
    // 23:30 New York is 05:30 in Berlin; 00:30 New York the next day is 06:30, on
    // Oura's next day and Berlin's same one.
    for (first, revised, applied) in [
        (
            "2026-01-05T23:30:00.000-05:00",
            "2026-01-06T00:30:00.000-05:00",
            true,
        ),
        // 17:30 New York is 23:30 in Berlin; 18:30 is past Berlin's midnight, on
        // Oura's same day.
        (
            "2026-01-05T17:30:00.000-05:00",
            "2026-01-05T18:30:00.000-05:00",
            false,
        ),
    ] {
        let journal = berlin();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let end = |start: &str| {
            let at = DateTime::parse_from_rfc3339(start).unwrap() + chrono::Duration::minutes(20);
            at.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
        };
        run(
            root,
            &one_workout(&scratch.path().join("1"), first, &end(first)),
            "20261104_120000",
            None,
        );
        let rid = oura_row_id("workout", "w1");
        let before = rows_of(root, &rid);
        let bytes = stream_bytes(root);
        let outcome = run(
            root,
            &one_workout(&scratch.path().join("2"), revised, &end(revised)),
            "20261104_120001",
            None,
        );
        if applied {
            assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
            assert_converged(
                root,
                &rid,
                &before,
                &tiling(&rid, &as_refs(&windows_from("20260106", (6, 30), 300, 4))),
            );
        } else {
            assert_eq!(stream_bytes(root), bytes);
            assert_eq!(outcome.holes[0].reason, HoleReason::Deferred);
        }
    }
}

#[test]
fn a_start_moved_to_the_last_second_of_its_day_moves_and_to_midnight_is_kept() {
    for (revised, applied) in [
        ("2026-01-05T23:59:59.000-07:00", true),
        ("2026-01-06T00:00:00.000-07:00", false),
    ] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let end = "2026-01-06T00:30:00.000-07:00";
        run(
            root,
            &one_workout(
                &scratch.path().join("1"),
                "2026-01-05T23:40:00.000-07:00",
                end,
            ),
            "20261104_120000",
            None,
        );
        let rid = oura_row_id("workout", "w1");
        let before = rows_of(root, &rid);
        let bytes = stream_bytes(root);
        let outcome = run(
            root,
            &one_workout(&scratch.path().join("2"), revised, end),
            "20261104_120001",
            None,
        );
        if applied {
            assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
            let mut windows = windows_from("20260105", (23, 55), 300, 1);
            windows.extend(windows_from("20260106", (0, 0), 300, 6));
            assert_converged(root, &rid, &before, &tiling(&rid, &as_refs(&windows)));
        } else {
            assert_eq!(stream_bytes(root), bytes, "{revised}");
            assert_eq!(outcome.holes[0].reason, HoleReason::Deferred);
        }
    }
}

/// Live first rows of each document, by its row id.
fn live_firsts(root: &Path, rid: &str) -> usize {
    rows_of(root, rid)
        .into_iter()
        .filter(|((_, _, id), row)| id == rid && row.kind != RowKind::Moved)
        .count()
}

struct Variant {
    name: &'static str,
    endpoint: &'static str,
    first: Vec<Value>,
    revised: Vec<Value>,
    ids: Vec<&'static str>,
}

fn variants() -> Vec<Variant> {
    let at = |id: &str, start: &str, end: &str| workout(id, "running", start, end);
    vec![
        Variant {
            name: "moved later",
            endpoint: "workout",
            first: vec![at(
                "w1",
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:30:00.000-07:00",
            )],
            revised: vec![at(
                "w1",
                "2026-01-05T08:40:00.000-07:00",
                "2026-01-05T09:10:00.000-07:00",
            )],
            ids: vec!["w1"],
        },
        Variant {
            name: "moved earlier",
            endpoint: "workout",
            first: vec![at(
                "w1",
                "2026-01-05T08:40:00.000-07:00",
                "2026-01-05T09:10:00.000-07:00",
            )],
            revised: vec![at(
                "w1",
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:30:00.000-07:00",
            )],
            ids: vec!["w1"],
        },
        Variant {
            name: "two tags swapping hours",
            endpoint: "enhanced_tag",
            first: vec![
                tag("t1", "2026-01-05T09:15:00-07:00"),
                tag("t2", "2026-01-05T11:30:00-07:00"),
            ],
            revised: vec![
                tag("t1", "2026-01-05T11:15:00-07:00"),
                tag("t2", "2026-01-05T09:30:00-07:00"),
            ],
            ids: vec!["t1", "t2"],
        },
    ]
}

fn other_pages(dir: &Path) -> PathBuf {
    stage(
        dir,
        &[("daily_readiness", vec![readiness("r9", "2026-02-01", 70)])],
    )
}

#[test]
fn takeovers_inside_the_start_day_print_the_move_and_the_next_read_leaves_one_first_row() {
    for variant in variants() {
        for point in ["written", "marking"] {
            let journal = journal();
            let root = journal.path();
            let scratch = tempfile::tempdir().unwrap();
            let first = stage(
                &scratch.path().join("1"),
                &[(variant.endpoint, variant.first.clone())],
            );
            run(root, &first, "20261104_120000", Some("health"));
            let revised = stage(
                &scratch.path().join("2"),
                &[(variant.endpoint, variant.revised.clone())],
            );
            let stop = stopping_at(point, 1);
            let mut stopped = request(&revised, "20261104_120001", Some("health"));
            stopped.stop_at = Some(&stop);
            run_oura_segments(root, &stopped).unwrap_err();
            let label = format!("{} at {point}", variant.name);
            // Taken over by other pages: what the stopped run left is published,
            // and its move is a hole of its own until a read finishes it.
            let taken = run(
                root,
                &other_pages(&scratch.path().join("3")),
                "20261104_120002",
                Some("health"),
            );
            assert!(
                taken.holes.contains(&Hole {
                    endpoint: variant.endpoint.to_owned(),
                    at: "2026-01-05".to_owned(),
                    reason: HoleReason::Converging,
                }),
                "{label}: {:?}",
                taken.holes
            );
            assert!(
                taken.summary.contains(&format!(
                    "- {}, 2026-01-05: changed in Oura since, and the journal hasn't finished moving it to its new time; an import that asks for this day again finishes it",
                    solstone_core_import_sources::apple_health::name_words(variant.endpoint)
                )),
                "{label}: {}",
                taken.summary
            );
            for id in &variant.ids {
                let rid = oura_row_id(variant.endpoint, id);
                assert!(
                    live_firsts(root, &rid) >= 1,
                    "{label}: {id} is live nowhere"
                );
            }
            let next = run(root, &revised, "20261104_120003", Some("health"));
            assert!(next.holes.is_empty(), "{label}: {:?}", next.holes);
            for id in &variant.ids {
                let rid = oura_row_id(variant.endpoint, id);
                assert_eq!(live_firsts(root, &rid), 1, "{label}: {id}");
            }
            if variant.endpoint == "workout" {
                assert_eq!(records(root, "20260105").len(), 1, "{label}");
                let (window, account) = account_of(root, "20260105");
                let start = variant.revised[0]["start_datetime"].as_str().unwrap();
                assert_eq!(
                    window.as_deref(),
                    Some(format!("{}{}00_300", &start[11..13], &start[14..16]).as_str()),
                    "{label}"
                );
                assert!(matches!(account, Account::Live(_)), "{label}: {account:?}");
            }
        }
    }
}

#[test]
fn a_stop_between_the_passes_leaves_every_document_live_and_the_run_carried_on_leaves_one() {
    for variant in variants() {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let mut first = vec![(variant.endpoint, variant.first.clone())];
        first.push((
            "heartrate",
            vec![heart("2026-01-05T21:10:00.000Z", 60, "rest")],
        ));
        run(
            root,
            &stage(&scratch.path().join("1"), &first),
            "20261104_120000",
            None,
        );
        // An hour of the day the owner deleted.
        delete(root, "20260105", "140000_3600");
        let revised = stage(
            &scratch.path().join("2"),
            &[(variant.endpoint, variant.revised.clone())],
        );
        let stop = stopping_at("marking", 1);
        let mut stopped = request(&revised, "20261104_120001", None);
        stopped.stop_at = Some(&stop);
        run_oura_segments(root, &stopped).unwrap_err();
        for id in &variant.ids {
            let rid = oura_row_id(variant.endpoint, id);
            assert!(live_firsts(root, &rid) >= 1, "{}: {id}", variant.name);
        }
        run(root, &revised, "20261104_120002", None);
        for id in &variant.ids {
            let rid = oura_row_id(variant.endpoint, id);
            assert_eq!(live_firsts(root, &rid), 1, "{}: {id}", variant.name);
        }
        assert!(
            window(root, "20260105", "140000_3600")
                .join("tombstone.json")
                .is_file()
        );
        assert!(
            !window(root, "20260105", "140000_3600")
                .join(BODY_RECORDS_FILE)
                .exists()
        );
    }
}

/// A night from 22:10, to `end`.
fn shortened_night(end: &str) -> Value {
    sleep("n1", "long_sleep", "2026-01-05T22:10:00.000-07:00", end)
}

#[test]
fn a_takeover_after_the_start_day_leaves_a_printed_hole_and_a_later_read_converges_it() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-05", "2026-01-06"));
    let first = Oura::new();
    first.serve(
        "sleep",
        vec![shortened_night("2026-01-06T07:40:00.000-07:00")],
    );
    finished(root, &first, &ask("20261104_120000", range));
    let rid = oura_row_id("sleep", "n1");
    let before = rows_of(root, &rid);
    let revised = Oura::new();
    revised.serve(
        "sleep",
        vec![shortened_night("2026-01-06T05:20:00.000-07:00")],
    );
    // Stopped with the next day pending, before any of its files.
    let stop = stopping_at("pending", 2);
    import(
        root,
        &revised,
        &Ask {
            stop_at: Some(&stop),
            ..ask("20261104_120001", range)
        },
    )
    .unwrap_err();
    let taken = finished(
        root,
        &Oura::new(),
        &ask("20261104_120002", Some(("2026-01-12", "2026-01-13"))),
    );
    let hole = Hole {
        endpoint: "sleep".to_owned(),
        at: "2026-01-06".to_owned(),
        reason: HoleReason::Converging,
    };
    assert!(taken.holes.contains(&hole), "{:?}", taken.holes);
    assert!(
        taken.summary.contains(
            "- sleep, 2026-01-06: changed in Oura since, and the journal hasn't finished moving it"
        ),
        "{}",
        taken.summary
    );
    assert_eq!(
        holes_of_file(root, "sleep"),
        json!([{"at": "2026-01-06", "reason": "converging"}])
    );
    // Its next day's old tiles are still live.
    assert!(rows_of(root, &rid).contains_key(&(
        "20260106".to_owned(),
        "070000_3600".to_owned(),
        format!("{rid}@20260106/070000_3600")
    )));
    let later = finished(root, &revised, &ask("20261104_120003", range));
    assert!(later.holes.is_empty(), "{:?}", later.holes);
    let mut windows = windows_from("20260105", (22, 0), 3600, 2);
    windows.extend(windows_from("20260106", (0, 0), 3600, 6));
    assert_converged(root, &rid, &before, &tiling(&rid, &as_refs(&windows)));
    assert_eq!(holes_of_file(root, "sleep"), Value::Null);
    assert_eq!(
        covered(root, "sleep"),
        json!([["2026-01-05", "2026-01-06"], ["2026-01-12", "2026-01-13"]])
    );
}

#[test]
fn two_moves_on_one_oura_day_keep_it_a_hole_while_either_is_unfinished() {
    for cut in ["takeover", "unreadable"] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        // A nap Oura dates by the next day, wholly on the first; and a night on both.
        let pages = |name: &str, nap: (&str, &str), end: &str| {
            let mut first = sleep("p1", "late_nap", nap.0, nap.1);
            first["day"] = json!("2026-01-06");
            stage(
                &scratch.path().join(name),
                &[(
                    "sleep",
                    vec![
                        first,
                        sleep("n1", "long_sleep", "2026-01-05T22:10:00.000-07:00", end),
                    ],
                )],
            )
        };
        run(
            root,
            &pages(
                "1",
                (
                    "2026-01-05T13:00:00.000-07:00",
                    "2026-01-05T13:40:00.000-07:00",
                ),
                "2026-01-06T07:40:00.000-07:00",
            ),
            "20261104_120000",
            None,
        );
        let revised = pages(
            "2",
            (
                "2026-01-05T15:00:00.000-07:00",
                "2026-01-05T15:40:00.000-07:00",
            ),
            "2026-01-06T05:20:00.000-07:00",
        );
        let hole = Hole {
            endpoint: "sleep".to_owned(),
            at: "2026-01-06".to_owned(),
            reason: HoleReason::Converging,
        };
        let holes = if cut == "takeover" {
            let stop = stopping_at("pending", 2);
            let mut stopped = request(&revised, "20261104_120001", None);
            stopped.stop_at = Some(&stop);
            run_oura_segments(root, &stopped).unwrap_err();
            run(
                root,
                &other_pages(&scratch.path().join("3")),
                "20261104_120002",
                None,
            )
            .holes
        } else {
            // The night's next day can't be read by the time it's written.
            let stop = stopping_at("placed", 1);
            let mut stopped = request(&revised, "20261104_120001", None);
            stopped.stop_at = Some(&stop);
            run_oura_segments(root, &stopped).unwrap_err();
            fs::write(
                window(root, "20260106", "070000_3600").join(BODY_RECORDS_FILE),
                "not rows\n",
            )
            .unwrap();
            run(root, &revised, "20261104_120002", None).holes
        };
        assert!(holes.contains(&hole), "{cut}: {holes:?}");
        // The nap is done either way.
        assert_eq!(live_firsts(root, &oura_row_id("sleep", "p1")), 1, "{cut}");
    }
}

#[test]
fn an_uninterrupted_move_claims_its_oura_day_says_so_and_prints_no_hole() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-01", "2026-01-10"));
    let first = Oura::new();
    first.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        )],
    );
    finished(root, &first, &ask("20261104_120000", range));
    let second = Oura::new();
    second.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:40:00.000-07:00",
            "2026-01-05T09:10:00.000-07:00",
        )],
    );
    let outcome = finished(root, &second, &ask("20261104_120001", range));
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
    // Every window it left is published with its mark, beside every new one.
    assert_eq!(outcome.published, 12, "{}", outcome.summary);
    assert!(
        !outcome.summary.contains("Not written in full"),
        "{}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains(
            "Oura has since changed the time of 1 entry; the journal now shows it at the new time."
        ),
        "{}",
        outcome.summary
    );
    assert_eq!(
        covered(root, "workout"),
        json!([["2026-01-01", "2026-01-10"]])
    );
    assert_eq!(holes_of_file(root, "workout"), Value::Null);
    // The listing counts the rows that moved apart from those it wrote.
    let listing =
        fs::read_to_string(root.join("imports/20261104_120001").join(LISTING_FILE)).unwrap();
    let line: Value = serde_json::from_str(listing.lines().next().unwrap()).unwrap();
    let moved: u64 = line["moved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry[1].as_u64().unwrap())
        .sum();
    assert_eq!(moved, 6, "{line}");
    assert!(
        line["windows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry[2] == 0),
        "{line}"
    );
}

#[test]
fn a_deleted_tag_comes_back_on_an_asked_range_not_a_scheduled_one_and_its_day_is_claimed() {
    for scheduled in [false, true] {
        let journal = connected("America/Denver");
        let root = journal.path();
        let range = Some(("2026-01-01", "2026-01-10"));
        let first = Oura::new();
        first.serve("enhanced_tag", vec![tag("t1", "2026-01-05T09:15:00-07:00")]);
        finished(root, &first, &ask("20261104_120000", range));
        delete(root, "20260105", "090000_3600");
        let second = Oura::new();
        second.serve("enhanced_tag", vec![tag("t1", "2026-01-05T11:15:00-07:00")]);
        let outcome = finished(
            root,
            &second,
            &Ask {
                scheduled,
                ..ask("20261104_120001", range)
            },
        );
        assert!(outcome.holes.is_empty(), "{scheduled}: {:?}", outcome.holes);
        if scheduled {
            // Nobody asked for this range: a sync, which leaves out what the owner
            // deleted, wherever Oura moves it.
            assert!(placed(root, "t1").is_empty());
            assert!(
                outcome
                    .summary
                    .contains("1 entry wasn't added, because you deleted"),
                "{}",
                outcome.summary
            );
        } else {
            // A range the owner asked for is a re-import: the tag comes back, at
            // its new hour.
            let back = placed(root, "t1");
            assert_eq!(back.len(), 1, "{}", outcome.summary);
            assert_eq!(
                (back[0].0.as_str(), back[0].1.as_str()),
                ("20260105", "110000_3600")
            );
            assert!(!outcome.summary.contains("added"), "{}", outcome.summary);
        }
        assert!(
            !outcome.summary.contains("couldn't be read"),
            "{scheduled}: {}",
            outcome.summary
        );
        assert_eq!(
            covered(root, "enhanced_tag"),
            json!([["2026-01-01", "2026-01-10"]])
        );
        assert_eq!(holes_of_file(root, "enhanced_tag"), Value::Null);
    }
}

#[test]
fn a_two_day_night_with_index_tiles_imports_again_into_window_tiles_and_claims_its_day() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-05", "2026-01-06"));
    let oura = Oura::new();
    oura.serve(
        "sleep",
        vec![sleep(
            "n1",
            "long_sleep",
            "2026-01-05T22:10:00.000-07:00",
            "2026-01-06T06:30:00.000-07:00",
        )],
    );
    finished(root, &oura, &ask("20261104_120000", range));
    to_index_tiles(root);
    let rid = oura_row_id("sleep", "n1");
    let before = rows_of(root, &rid);
    assert!(
        before
            .keys()
            .any(|(day, _, id)| day == "20260106" && id.ends_with("-3"))
    );
    let again = finished(root, &oura, &ask("20261104_120001", range));
    assert!(again.holes.is_empty(), "{:?}", again.holes);
    let mut windows = windows_from("20260105", (22, 0), 3600, 2);
    windows.extend(windows_from("20260106", (0, 0), 3600, 7));
    assert_converged(root, &rid, &before, &tiling(&rid, &as_refs(&windows)));
    assert_eq!(
        covered(root, "sleep"),
        json!([["2026-01-05", "2026-01-06"]])
    );
    assert_eq!(holes_of_file(root, "sleep"), Value::Null);
}

#[test]
fn a_restore_after_the_start_days_files_prints_the_move_until_a_run_writes_that_day() {
    let journal = connected("America/Denver");
    let root = journal.path();
    let range = Some(("2026-01-01", "2026-01-10"));
    let health = |timestamp, range| Ask {
        facet: Some("health"),
        ..ask(timestamp, range)
    };
    let first = Oura::new();
    first.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        )],
    );
    finished(root, &first, &health("20261104_120000", range));
    let second = Oura::new();
    second.serve(
        "workout",
        vec![one_day_workout(
            "w1",
            "2026-01-05T08:40:00.000-07:00",
            "2026-01-05T09:10:00.000-07:00",
        )],
    );
    // Its files are written and published, and its record isn't yet refreshed.
    let stop = stopping_at("published", 1);
    import(
        root,
        &second,
        &Ask {
            stop_at: Some(&stop),
            ..health("20261104_120001", range)
        },
    )
    .unwrap_err();
    for folder in ["placed", "pages"] {
        fs::remove_dir_all(root.join(OURA.run_dir_rel).join(folder)).unwrap();
    }
    let repaired = finished(
        root,
        &Oura::new(),
        &health("20261104_120002", Some(("2026-01-12", "2026-01-13"))),
    );
    assert!(
        repaired.summary.contains("published again"),
        "{}",
        repaired.summary
    );
    assert!(
        repaired.summary.contains(
            "- workouts, 2026-01-05: changed in Oura since, and the journal hasn't finished moving it"
        ),
        "{}",
        repaired.summary
    );
    // The record still names the old start: the limit, until a run writes that day.
    assert_eq!(
        records(root, "20260105")[0]["account"]["start"],
        "2026-01-05T08:00:00.000-07:00"
    );
    finished(root, &second, &health("20261104_120003", range));
    assert_eq!(
        records(root, "20260105")[0]["account"]["start"],
        "2026-01-05T08:40:00.000-07:00"
    );
    assert_eq!(holes_of_file(root, "workout"), Value::Null);
}

#[test]
fn live_twice_and_then_moved_across_midnight_shows_twice_and_prints_a_hole() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages =
        |name: &str, start: &str, end: &str| one_workout(&scratch.path().join(name), start, end);
    run(
        root,
        &pages(
            "1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        None,
    );
    let revised = pages(
        "2",
        "2026-01-05T08:40:00.000-07:00",
        "2026-01-05T09:10:00.000-07:00",
    );
    let stop = stopping_at("marking", 1);
    let mut stopped = request(&revised, "20261104_120001", None);
    stopped.stop_at = Some(&stop);
    run_oura_segments(root, &stopped).unwrap_err();
    run(
        root,
        &other_pages(&scratch.path().join("3")),
        "20261104_120002",
        None,
    );
    let rid = oura_row_id("workout", "w1");
    assert_eq!(live_firsts(root, &rid), 2);
    let bytes = stream_bytes(root);
    let crossed = run(
        root,
        &pages(
            "4",
            "2026-01-06T00:10:00.000-07:00",
            "2026-01-06T00:40:00.000-07:00",
        ),
        "20261104_120003",
        None,
    );
    assert_eq!(stream_bytes(root), bytes);
    assert_eq!(live_firsts(root, &rid), 2);
    assert_eq!(crossed.holes[0].reason, HoleReason::Deferred);
    solstone_core_location::body_day::BodyDays::default().reconcile(root);
    let day = fs::read_to_string(root.join("chronicle/20260105/body.md")).unwrap();
    assert_eq!(day.matches("- workout: running").count(), 2, "{day}");
}

#[test]
fn a_moved_only_window_at_the_streams_highest_seq_is_where_repair_and_reconcile_put_its_tail() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages =
        |name: &str, start: &str, end: &str| one_workout(&scratch.path().join(name), start, end);
    run(
        root,
        &pages(
            "1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        None,
    );
    run(
        root,
        &pages(
            "2",
            "2026-01-05T08:40:00.000-07:00",
            "2026-01-05T09:10:00.000-07:00",
        ),
        "20261104_120001",
        None,
    );
    let marked = ("20260105".to_owned(), "080000_300".to_owned());
    assert!(
        stored(root)[&marked]
            .iter()
            .all(|row| row.kind == RowKind::Moved)
    );
    // The marked window holds the stream's highest place, and the stream's record
    // is behind it, as after a restore.
    let marker = |window: &WindowId| -> Value {
        serde_json::from_slice(
            &fs::read(window_dir(root, &OURA, window).join("stream.json")).unwrap(),
        )
        .unwrap()
    };
    let highest = stored(root)
        .keys()
        .map(|window| marker(window)["seq"].as_u64().unwrap())
        .max()
        .unwrap()
        + 7;
    let mut mark = marker(&marked);
    mark["seq"] = json!(highest);
    fs::write(
        window_dir(root, &OURA, &marked).join("stream.json"),
        mark.to_string(),
    )
    .unwrap();
    let record_path = root.join("streams/import.oura.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record["seq"] = json!(1);
    fs::write(&record_path, record.to_string()).unwrap();
    assert!(
        stored_windows_of_day(root, &OURA, "20260105")
            .unwrap()
            .contains(&marked)
    );
    reconcile_stream(root, &OURA).unwrap();
    let record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["seq"], json!(highest), "{record}");
    assert_eq!(record["last_segment"], json!("080000_300"), "{record}");
}

/// The limit: decided at the start day, a move stands once its new rows are
/// written, and an owner's delete of the old window while the run is stopped
/// stays a delete.
#[test]
fn a_delete_after_the_start_day_decided_leaves_the_new_row_and_the_delete() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages =
        |name: &str, start: &str, end: &str| one_workout(&scratch.path().join(name), start, end);
    run(
        root,
        &pages(
            "1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        None,
    );
    let revised = pages(
        "2",
        "2026-01-05T08:40:00.000-07:00",
        "2026-01-05T09:10:00.000-07:00",
    );
    let stop = stopping_at("pending", 1);
    let mut stopped = request(&revised, "20261104_120001", None);
    stopped.stop_at = Some(&stop);
    run_oura_segments(root, &stopped).unwrap_err();
    delete(root, "20260105", "080000_300");
    run(root, &revised, "20261104_120002", None);
    let rid = oura_row_id("workout", "w1");
    assert_eq!(live_firsts(root, &rid), 1);
    assert_eq!(placed(root, "w1")[0].1, "084000_300");
    assert!(
        window(root, "20260105", "080000_300")
            .join("tombstone.json")
            .is_file()
    );
    assert!(
        !window(root, "20260105", "080000_300")
            .join(BODY_RECORDS_FILE)
            .exists()
    );
}

#[test]
fn a_record_of_the_account_on_another_day_keeps_a_move_as_first_written() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages =
        |name: &str, start: &str, end: &str| one_workout(&scratch.path().join(name), start, end);
    run(
        root,
        &pages(
            "1",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:30:00.000-07:00",
        ),
        "20261104_120000",
        Some("health"),
    );
    // Its record filed on the day before, as a move across midnight beyond this
    // lookup's reach would leave it.
    let activities = root.join("facets/health/activities");
    fs::rename(
        activities.join("20260105.jsonl"),
        activities.join("20260104.jsonl"),
    )
    .unwrap();
    let bytes = stream_bytes(root);
    let outcome = run(
        root,
        &pages(
            "2",
            "2026-01-05T08:40:00.000-07:00",
            "2026-01-05T09:10:00.000-07:00",
        ),
        "20261104_120001",
        None,
    );
    assert_eq!(stream_bytes(root), bytes);
    assert_eq!(
        outcome.holes[0].reason,
        HoleReason::Deferred,
        "{:?}",
        outcome.holes
    );
}

#[test]
fn a_day_of_its_stored_span_that_cant_be_read_holds_a_move_back() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, end: &str| {
        stage(
            &scratch.path().join(name),
            &[(
                "session",
                vec![session("s1", "nap", "2026-01-05T22:00:00.000-07:00", end)],
            )],
        )
    };
    run(
        root,
        &pages("1", "2026-01-09T06:00:00.000-07:00"),
        "20261104_120000",
        None,
    );
    let unreadable = window(root, "20260109", "100000_3600");
    fs::create_dir_all(&unreadable).unwrap();
    fs::write(unreadable.join(BODY_RECORDS_FILE), "not rows\n").unwrap();
    let bytes = stream_bytes(root);
    let outcome = run(
        root,
        &pages("2", "2026-01-06T06:00:00.000-07:00"),
        "20261104_120001",
        None,
    );
    assert_eq!(stream_bytes(root), bytes);
    assert_eq!(
        outcome.holes[0].reason,
        HoleReason::HeldBack,
        "{:?}",
        outcome.holes
    );
}
