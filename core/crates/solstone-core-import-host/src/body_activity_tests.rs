// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};
use solstone_core_import_sources::apple_health::STREAM;
use solstone_core_import_sources::body_stream::APPLE;

use super::*;

const DAY: &str = "20261010";

fn journal() -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    for facet in ["body", "other"] {
        let dir = journal.path().join("facets").join(facet);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"title": facet, "description": facet}).to_string(),
        )
        .unwrap();
    }
    journal
}

fn experience(id: &str, kind: &str, start: &str, end: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Experience,
        id.to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some(kind.to_owned());
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row.by = Some("Synthetic Watch".to_owned());
    row
}

fn reading(id: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Reading,
        id.to_owned(),
        "apple_health",
        "HKQuantityTypeIdentifierHeartRate",
        "Heart rate".to_owned(),
    );
    row.at = Some("2026-10-10 08:05:00 -0600".to_owned());
    row.value = Some("120".to_owned());
    row
}

fn window(root: &Path, key: &str, facet: Option<&str>, rows: &[BodyRow]) -> PathBuf {
    let dir = root.join("chronicle").join(DAY).join(STREAM).join(key);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(dir.join(BODY_RECORDS_FILE), encode_rows(rows)).unwrap();
    if let Some(facet) = facet {
        fs::write(
            dir.join("talents/facets.json"),
            format!("{}\n", json!([{ "facet": facet }])),
        )
        .unwrap();
    }
    dir
}

fn file(root: &Path, facet: &str) -> PathBuf {
    root.join("facets")
        .join(facet)
        .join("activities")
        .join(format!("{DAY}.jsonl"))
}

fn rows(root: &Path, facet: &str) -> Vec<Value> {
    fs::read_to_string(file(root, facet))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn each_experience_gets_one_record_that_refers_to_its_account() {
    let journal = journal();
    let root = journal.path();
    window(
        root,
        "080000_3600",
        Some("body"),
        &[
            experience(
                "w1",
                "workout",
                "2026-10-10 08:00:00 -0600",
                "2026-10-10 08:40:00 -0600",
            ),
            reading("r1"),
        ],
    );
    window(
        root,
        "220000_3600",
        Some("body"),
        &[experience(
            "s1",
            "sleep",
            "2026-10-10 22:10:00 -0600",
            "2026-10-11 06:20:00 -0600",
        )],
    );
    let counts = write_records(root, &APPLE, DAY);
    assert_eq!(counts.written, 2);
    let written = rows(root, "body");
    assert_eq!(written.len(), 2);
    let key = account_key(STREAM, "w1");
    let workout = written
        .iter()
        .find(|row| row["activity"] == "workout")
        .unwrap();
    assert_eq!(workout["id"], record_id(&key));
    assert_eq!(workout["source"], "body");
    let arrived = workout["account"]["arrived"].as_str().unwrap().to_owned();
    assert!(
        arrived.len() == 8 && arrived.bytes().all(|byte| byte.is_ascii_digit()),
        "{arrived}"
    );
    assert_eq!(
        workout["account"],
        json!({"key": key, "start": "2026-10-10 08:00:00 -0600", "end": "2026-10-10 08:40:00 -0600",
            "recorder": recorder_id(STREAM, Some("Synthetic Watch")), "type": "running", "arrived": arrived})
    );
    // A reference only: nothing of the row is copied, and no talent can select it.
    let text = fs::read_to_string(file(root, "body")).unwrap();
    // The value is matched quoted: a bare "120" also turns up inside a random id.
    for absent in ["Synthetic Watch", "Running", "segments", "\"120\""] {
        assert!(!text.contains(absent), "{absent} in {text}");
    }
    // No default reader returns it.
    assert!(
        solstone_core_facets::load_activity_records(root, "body", DAY, true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn writing_again_appends_nothing() {
    let journal = journal();
    let root = journal.path();
    window(
        root,
        "080000_3600",
        Some("body"),
        &[experience(
            "w1",
            "workout",
            "2026-10-10 08:00:00 -0600",
            "2026-10-10 08:40:00 -0600",
        )],
    );
    assert_eq!(write_records(root, &APPLE, DAY).written, 1);
    let before = fs::read(file(root, "body")).unwrap();
    assert_eq!(write_records(root, &APPLE, DAY), RecordCounts::default());
    assert_eq!(fs::read(file(root, "body")).unwrap(), before);
}

#[test]
fn no_facet_means_no_record_until_the_window_has_one() {
    let journal = journal();
    let root = journal.path();
    let rows_in = [experience(
        "w1",
        "workout",
        "2026-10-10 08:00:00 -0600",
        "2026-10-10 08:40:00 -0600",
    )];
    window(root, "080000_3600", None, &rows_in);
    let counts = write_records(root, &APPLE, DAY);
    assert_eq!((counts.written, counts.no_facet), (0, 1));
    assert!(!root.join("facets/body/activities").exists());
    // The same export again with a facet: a record, once.
    window(root, "080000_3600", Some("body"), &rows_in);
    assert_eq!(write_records(root, &APPLE, DAY).written, 1);
    assert_eq!(rows(root, "body").len(), 1);
}

#[test]
fn a_record_stays_in_its_facet_when_the_window_moves() {
    let journal = journal();
    let root = journal.path();
    let rows_in = [experience(
        "w1",
        "workout",
        "2026-10-10 08:00:00 -0600",
        "2026-10-10 08:40:00 -0600",
    )];
    window(root, "080000_3600", Some("body"), &rows_in);
    assert_eq!(write_records(root, &APPLE, DAY).written, 1);
    window(root, "080000_3600", Some("other"), &rows_in);
    assert_eq!(write_records(root, &APPLE, DAY), RecordCounts::default());
    assert_eq!(rows(root, "body").len(), 1);
    assert!(rows(root, "other").is_empty());
}

#[test]
fn a_deleted_window_gets_no_record() {
    let journal = journal();
    let root = journal.path();
    let dir = window(
        root,
        "080000_3600",
        Some("body"),
        &[experience(
            "w1",
            "workout",
            "2026-10-10 08:00:00 -0600",
            "2026-10-10 08:40:00 -0600",
        )],
    );
    fs::write(dir.join("tombstone.json"), "{}").unwrap();
    assert_eq!(write_records(root, &APPLE, DAY), RecordCounts::default());
}

#[test]
fn a_refused_append_is_counted_and_the_next_facet_still_gets_its_record() {
    let journal = journal();
    let root = journal.path();
    window(
        root,
        "070000_3600",
        Some("gone"),
        &[experience(
            "w0",
            "workout",
            "2026-10-10 07:00:00 -0600",
            "2026-10-10 07:30:00 -0600",
        )],
    );
    window(
        root,
        "080000_3600",
        Some("body"),
        &[experience(
            "w1",
            "workout",
            "2026-10-10 08:00:00 -0600",
            "2026-10-10 08:40:00 -0600",
        )],
    );
    let counts = write_records(root, &APPLE, DAY);
    assert_eq!(counts.written, 1);
    assert_eq!(counts.refused.len(), 1);
    assert_eq!(counts.refused[0].0, "gone");
    assert_eq!(rows(root, "body").len(), 1);
}

#[test]
fn a_day_holding_a_half_written_line_keeps_it_and_says_what_was_written() {
    let journal = journal();
    let root = journal.path();
    let half = r#"{"id":"meet_0900","activity":"mee"#;
    fs::create_dir_all(file(root, "body").parent().unwrap()).unwrap();
    fs::write(file(root, "body"), format!("{half}\n")).unwrap();
    window(
        root,
        "080000_3600",
        Some("body"),
        &[experience(
            "w1",
            "workout",
            "2026-10-10 08:00:00 -0600",
            "2026-10-10 08:40:00 -0600",
        )],
    );
    let counts = write_records(root, &APPLE, DAY);
    assert_eq!(counts.written, 1);
    assert!(counts.refused.is_empty());
    let text = fs::read_to_string(file(root, "body")).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[0], half);
    assert!(
        lines[1].contains(&record_id(&account_key(STREAM, "w1"))),
        "{text}"
    );
}

/// A line that isn't text, beside a record in the same file, names no key and
/// hides no other: a refiled window gets no second record, so a hide holds.
#[test]
fn a_line_that_isnt_text_doesnt_make_a_refiled_window_record_twice() {
    let journal = journal();
    let root = journal.path();
    let rows_in = [experience(
        "w1",
        "workout",
        "2026-10-10 08:00:00 -0600",
        "2026-10-10 08:40:00 -0600",
    )];
    window(root, "080000_3600", Some("body"), &rows_in);
    assert_eq!(write_records(root, &APPLE, DAY).written, 1);
    let mut bytes = fs::read(file(root, "body")).unwrap();
    bytes.extend_from_slice(b"{\"id\":\"torn\",\"title\":\"\xff\xfe\"}\n");
    fs::write(file(root, "body"), &bytes).unwrap();
    window(root, "080000_3600", Some("other"), &rows_in);
    assert_eq!(write_records(root, &APPLE, DAY), RecordCounts::default());
    assert!(rows(root, "other").is_empty());
    assert_eq!(fs::read(file(root, "body")).unwrap(), bytes);
}

/// A day file that is there but can't be read is refused for the day: nothing
/// is written beside it, and the next run tries again.
#[cfg(unix)]
#[test]
fn a_day_file_that_cant_be_read_writes_nothing_that_day() {
    use std::os::unix::fs::PermissionsExt;

    let journal = journal();
    let root = journal.path();
    let rows_in = [experience(
        "w1",
        "workout",
        "2026-10-10 08:00:00 -0600",
        "2026-10-10 08:40:00 -0600",
    )];
    window(root, "080000_3600", Some("body"), &rows_in);
    assert_eq!(write_records(root, &APPLE, DAY).written, 1);
    window(root, "080000_3600", Some("other"), &rows_in);
    fs::set_permissions(file(root, "body"), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(file(root, "body")).is_ok() {
        // Running as root: the mode can't stop a read.
        return;
    }
    let counts = write_records(root, &APPLE, DAY);
    fs::set_permissions(file(root, "body"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(counts.written, 0);
    assert_eq!(counts.refused.len(), 1);
    assert_eq!(counts.refused[0].0, "body");
    assert!(rows(root, "other").is_empty());
}

#[test]
fn a_plain_file_among_the_facets_is_no_facet_and_refuses_nothing() {
    let journal = journal();
    let root = journal.path();
    // A facet tool keeps its candidates in a file beside the facet folders.
    fs::write(root.join("facets/review-candidates.jsonl"), "{}\n").unwrap();
    window(
        root,
        "080000_3600",
        Some("body"),
        &[experience(
            "w1",
            "workout",
            "2026-10-10 08:00:00 -0600",
            "2026-10-10 08:40:00 -0600",
        )],
    );
    let counts = write_records(root, &APPLE, DAY);
    assert!(counts.refused.is_empty(), "{:?}", counts.refused);
    assert_eq!(counts.written, 1);
    assert_eq!(write_records(root, &APPLE, DAY), RecordCounts::default());
}

const NEXT: &str = "20261011";

/// One piece of a Strava workout as Strava's import writes it, filed under
/// `facet` when it has one. Without `fields`, a tile from before every other
/// column was kept.
fn strava_tile(
    root: &Path,
    day: &str,
    key: &str,
    (activity_id, index, count): (u64, u64, u64),
    (start, elapsed): (&str, u64),
    facet: Option<&str>,
    fields: Option<Value>,
) -> PathBuf {
    let mut workout = json!({"name": "Morning Run", "type": "Run", "start": start,
        "elapsed_seconds": elapsed, "distance_m": 5000.0, "commute": false,
        "entered_by_hand": false});
    if let Some(fields) = fields {
        workout["strava_fields"] = fields;
    }
    let tile = json!({"schema": "solstone.import.strava.tile.v1", "import_id": "20261010_120000",
        "activity_id": activity_id, "zone": "America/Denver",
        "tile": {"index": index, "count": count, "start": start, "end": start, "seconds": 300},
        "workout": workout});
    let import = root.join("imports/20261010_120000");
    fs::create_dir_all(&import).unwrap();
    fs::write(
        import.join("import.json"),
        json!({"attempt": {
            "attempt_id":"20261010_120000:1", "generation":1, "state":"completed", "started_at_ms":0
        }})
        .to_string(),
    )
    .unwrap();
    let dir = root
        .join("chronicle")
        .join(day)
        .join(STRAVA.stream)
        .join(key);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(dir.join("workout.json"), format!("{tile}\n")).unwrap();
    if let Some(facet) = facet {
        fs::write(
            dir.join("talents/facets.json"),
            format!("{}\n", json!([{ "facet": facet }])),
        )
        .unwrap();
    }
    dir
}

/// A Strava journal: its zone, and a workout of three tiles from 07:00.
fn strava_journal() -> tempfile::TempDir {
    let journal = journal();
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.strava-zone.json"),
        "{\"zone\":\"America/Denver\"}\n",
    )
    .unwrap();
    for (index, key) in ["070000_300", "070500_300", "071000_120"]
        .iter()
        .enumerate()
    {
        strava_tile(
            root,
            DAY,
            key,
            (5001, index as u64, 3),
            ("2026-10-10T07:00:00-06:00", 720),
            (index == 0).then_some("body"),
            Some(json!({"Activity Private Note": "synthetic note"})),
        );
    }
    journal
}

/// An activity record an earlier reading of segments filed, in `stream`.
fn sense_record(root: &Path, facet: &str, day: &str, stream: &str, segments: &[&str]) {
    let path = root
        .join("facets")
        .join(facet)
        .join("activities")
        .join(format!("{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let record = json!({"id": format!("run_{}", segments[0]), "facet": facet, "activity": "run",
        "segments": segments, "level_avg": 0.5, "description": "a synthetic run",
        "active_entities": [], "created_at": 1_760_000_000_000_u64, "stream": stream});
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(path, text).unwrap();
}

fn strava_records(root: &Path, facet: &str) -> Vec<Value> {
    rows(root, facet)
        .into_iter()
        .filter(|row| row["source"] == "body")
        .collect()
}

#[test]
fn each_strava_workout_gets_one_record_on_its_first_tile_and_none_again() {
    let journal = strava_journal();
    let root = journal.path();
    // A second workout, in a tile from before every column was kept.
    strava_tile(
        root,
        DAY,
        "180000_300",
        (5002, 0, 1),
        ("2026-10-10T18:00:00-06:00", 240),
        Some("body"),
        None,
    );
    let counts = write_strava_records(root, DAY);
    assert_eq!(counts.written, 2, "{counts:?}");
    let written = strava_records(root, "body");
    assert_eq!(written.len(), 2);
    let key = account_key("import.strava", "5001");
    let first = written
        .iter()
        .find(|row| row["account"]["key"] == key)
        .unwrap();
    assert_eq!(first["id"], record_id(&key));
    assert_eq!(first["activity"], "workout");
    assert_eq!(first["account"]["start"], "2026-10-10T07:00:00-06:00");
    assert_eq!(first["account"]["end"], "2026-10-10T07:12:00-06:00");
    assert_eq!(first["account"]["type"], "running");
    assert_eq!(
        first["account"]["recorder"],
        recorder_id("import.strava", Some("Strava"))
    );
    // A reference only: none of the workout's words or values are copied.
    let text = fs::read_to_string(file(root, "body")).unwrap();
    for absent in ["Morning Run", "synthetic note", "5000", "segments"] {
        assert!(!text.contains(absent), "{absent} in {text}");
    }
    // Writing again appends nothing.
    let before = fs::read(file(root, "body")).unwrap();
    assert_eq!(write_strava_records(root, DAY), RecordCounts::default());
    assert_eq!(fs::read(file(root, "body")).unwrap(), before);
}

#[test]
fn a_strava_workout_with_no_facet_gets_no_record() {
    let journal = strava_journal();
    let root = journal.path();
    fs::remove_file(
        root.join("chronicle")
            .join(DAY)
            .join("import.strava/070000_300/talents/facets.json"),
    )
    .unwrap();
    let counts = write_strava_records(root, DAY);
    assert_eq!((counts.written, counts.no_facet), (0, 1));
}

/// A run that stops after one workout's record: the next writes only the other.
#[test]
fn a_run_that_stopped_part_way_leaves_one_record_per_workout_after_the_next() {
    let journal = strava_journal();
    let root = journal.path();
    let late = strava_tile(
        root,
        DAY,
        "180000_300",
        (5002, 0, 1),
        ("2026-10-10T18:00:00-06:00", 240),
        Some("body"),
        None,
    );
    // The first run reaches only the morning's workout.
    let hidden = late.with_file_name(".180000_300");
    fs::rename(&late, &hidden).unwrap();
    assert_eq!(write_strava_records(root, DAY).written, 1);
    fs::rename(&hidden, &late).unwrap();
    assert_eq!(write_strava_records(root, DAY).written, 1);
    assert_eq!(write_strava_records(root, DAY), RecordCounts::default());
    let keys = strava_records(root, "body")
        .iter()
        .map(|row| row["account"]["key"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            account_key("import.strava", "5001"),
            account_key("import.strava", "5002")
        ]
    );
}

#[test]
fn a_workout_whose_first_tile_was_sensed_keeps_that_record_alone() {
    let journal = strava_journal();
    let root = journal.path();
    sense_record(root, "other", DAY, "import.strava", &["070000_300"]);
    let before = fs::read(file(root, "other")).unwrap();
    let row = window_rows(
        &root.join(format!("chronicle/{DAY}/import.strava/070000_300")),
        WindowFile::StravaTile,
    )
    .unwrap()
    .remove(0);
    assert_eq!(
        Sensed::default()
            .facet(root, DAY, "070000_300", &row)
            .unwrap()
            .as_deref(),
        Some("other")
    );
    let counts = write_strava_records(root, DAY);
    assert_eq!((counts.written, counts.sensed), (0, 1), "{counts:?}");
    assert!(strava_records(root, "body").is_empty());
    assert_eq!(fs::read(file(root, "other")).unwrap(), before);
}

#[test]
fn a_record_naming_only_a_later_tile_counts_as_sensed() {
    let journal = strava_journal();
    let root = journal.path();
    sense_record(root, "body", DAY, "import.strava", &["071000_120"]);
    let counts = write_strava_records(root, DAY);
    assert_eq!((counts.written, counts.sensed), (0, 1), "{counts:?}");
}

/// A later tile the owner deleted is still the workout's: where Strava cuts it.
#[test]
fn a_record_naming_a_deleted_later_tile_counts_as_sensed() {
    let journal = strava_journal();
    let root = journal.path();
    let tile = root
        .join("chronicle")
        .join(DAY)
        .join("import.strava/070500_300");
    fs::remove_file(tile.join("workout.json")).unwrap();
    fs::write(
        tile.join("tombstone.json"),
        json!({"reason": "owner_segment_delete", "manifest": ["workout.json"]}).to_string(),
    )
    .unwrap();
    sense_record(root, "body", DAY, "import.strava", &["070500_300"]);
    let before = fs::read(file(root, "body")).unwrap();
    let row = window_rows(
        &root.join(format!("chronicle/{DAY}/import.strava/070000_300")),
        WindowFile::StravaTile,
    )
    .unwrap()
    .remove(0);
    assert!(
        Sensed::default()
            .facet(root, DAY, "070000_300", &row)
            .unwrap()
            .is_none()
    );
    assert_eq!(write_strava_records(root, DAY).sensed, 1);
    assert_eq!(fs::read(file(root, "body")).unwrap(), before);
}

/// A tile the import placed beside older content, under another key, is
/// still the workout's: by what it holds.
#[test]
fn a_record_naming_a_tile_placed_under_another_key_counts_as_sensed() {
    let journal = strava_journal();
    let root = journal.path();
    let tiles = root.join("chronicle").join(DAY).join("import.strava");
    fs::rename(tiles.join("071000_120"), tiles.join("071000_121")).unwrap();
    sense_record(root, "body", DAY, "import.strava", &["071000_121"]);
    assert_eq!(write_strava_records(root, DAY).sensed, 1);
}

/// A record an earlier version filed at a key it misplaced the workout at.
#[test]
fn a_record_naming_a_misplaced_key_counts_as_sensed() {
    let journal = strava_journal();
    let root = journal.path();
    let (start, elapsed) = (
        chrono::DateTime::parse_from_rfc3339("2026-10-10T07:00:00-06:00").unwrap(),
        720,
    );
    let zone = solstone_core_journal_config::placed_zone(root, STRAVA_SOURCE).expect("placed zone");
    let (day, key) = solstone_core_import_sources::strava::misread_tile_keys(start, elapsed, zone)
        .into_iter()
        .next()
        .expect("a misplaced key");
    sense_record(root, "body", &day, "import.strava", &[&key]);
    assert_eq!(write_strava_records(root, DAY).sensed, 1);
}

/// A workout across midnight: a record on the next day naming its tile there.
#[test]
fn a_record_on_a_later_day_the_workout_covers_counts_as_sensed() {
    let journal = journal();
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.strava-zone.json"),
        "{\"zone\":\"America/Denver\"}\n",
    )
    .unwrap();
    let span = ("2026-10-10T23:58:00-06:00", 420);
    strava_tile(
        root,
        DAY,
        "235800_300",
        (5003, 0, 2),
        span,
        Some("body"),
        None,
    );
    strava_tile(root, NEXT, "000300_120", (5003, 1, 2), span, None, None);
    // Another stream's record of the same key on that day isn't Strava's.
    sense_record(root, "body", NEXT, "phone", &["000300_120"]);
    assert_eq!(write_strava_records(root, DAY).written, 1);
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("facets/body")).unwrap();
    strava_tile(
        root,
        DAY,
        "235800_300",
        (5003, 0, 2),
        span,
        Some("body"),
        None,
    );
    strava_tile(root, NEXT, "000300_120", (5003, 1, 2), span, None, None);
    sense_record(root, "body", NEXT, "import.strava", &["000300_120"]);
    let counts = write_strava_records(root, DAY);
    assert_eq!((counts.written, counts.sensed), (0, 1), "{counts:?}");
}

#[test]
fn another_streams_record_with_the_same_key_doesnt_count_as_sensed() {
    let journal = strava_journal();
    let root = journal.path();
    sense_record(root, "other", DAY, "phone", &["070000_300"]);
    sense_record(root, "other", DAY, "import.apple_health", &["070000_300"]);
    let counts = write_strava_records(root, DAY);
    assert_eq!((counts.written, counts.sensed), (1, 0), "{counts:?}");
}

#[test]
fn a_sensed_record_in_a_muted_facet_still_counts() {
    let journal = strava_journal();
    let root = journal.path();
    let quiet = root.join("facets/quiet");
    fs::create_dir_all(&quiet).unwrap();
    fs::write(
        quiet.join("facet.json"),
        json!({"title": "quiet", "muted": true}).to_string(),
    )
    .unwrap();
    sense_record(root, "quiet", DAY, "import.strava", &["070000_300"]);
    let counts = write_strava_records(root, DAY);
    assert_eq!((counts.written, counts.sensed), (0, 1), "{counts:?}");
}

/// A day the workout covers whose file can't be read decides nothing: no
/// record for that workout, counted, and the day's other workouts still get
/// theirs.
#[cfg(unix)]
#[test]
fn a_day_file_that_cant_be_read_refuses_the_workouts_it_would_decide() {
    use std::os::unix::fs::PermissionsExt;

    let journal = strava_journal();
    let root = journal.path();
    let span = ("2026-10-10T23:58:00-06:00", 420);
    strava_tile(
        root,
        DAY,
        "235800_300",
        (5003, 0, 2),
        span,
        Some("body"),
        None,
    );
    strava_tile(root, NEXT, "000300_120", (5003, 1, 2), span, None, None);
    sense_record(root, "other", NEXT, "phone", &["000300_120"]);
    let next = root
        .join("facets/other/activities")
        .join(format!("{NEXT}.jsonl"));
    fs::set_permissions(&next, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&next).is_ok() {
        // Running as root: the mode can't stop a read.
        return;
    }
    let counts = write_strava_records(root, DAY);
    fs::set_permissions(&next, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(counts.written, 1, "{counts:?}");
    assert_eq!(counts.refused.len(), 1, "{counts:?}");
    assert_eq!(counts.refused[0].0, "other");
    let keys = strava_records(root, "body")
        .iter()
        .map(|row| row["account"]["key"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(keys, [account_key("import.strava", "5001")]);
    // Readable again: the next run writes it.
    assert_eq!(write_strava_records(root, DAY).written, 1);
}
