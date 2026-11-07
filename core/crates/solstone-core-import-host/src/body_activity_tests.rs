// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};

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
    let counts = write_records(root, DAY);
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
    for absent in ["Synthetic Watch", "Running", "segments", "120"] {
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
    assert_eq!(write_records(root, DAY).written, 1);
    let before = fs::read(file(root, "body")).unwrap();
    assert_eq!(write_records(root, DAY), RecordCounts::default());
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
    let counts = write_records(root, DAY);
    assert_eq!((counts.written, counts.no_facet), (0, 1));
    assert!(!root.join("facets/body/activities").exists());
    // The same export again with a facet: a record, once.
    window(root, "080000_3600", Some("body"), &rows_in);
    assert_eq!(write_records(root, DAY).written, 1);
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
    assert_eq!(write_records(root, DAY).written, 1);
    window(root, "080000_3600", Some("other"), &rows_in);
    assert_eq!(write_records(root, DAY), RecordCounts::default());
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
    assert_eq!(write_records(root, DAY), RecordCounts::default());
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
    let counts = write_records(root, DAY);
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
    let counts = write_records(root, DAY);
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
    assert_eq!(write_records(root, DAY).written, 1);
    let mut bytes = fs::read(file(root, "body")).unwrap();
    bytes.extend_from_slice(b"{\"id\":\"torn\",\"title\":\"\xff\xfe\"}\n");
    fs::write(file(root, "body"), &bytes).unwrap();
    window(root, "080000_3600", Some("other"), &rows_in);
    assert_eq!(write_records(root, DAY), RecordCounts::default());
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
    assert_eq!(write_records(root, DAY).written, 1);
    window(root, "080000_3600", Some("other"), &rows_in);
    fs::set_permissions(file(root, "body"), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(file(root, "body")).is_ok() {
        // Running as root: the mode can't stop a read.
        return;
    }
    let counts = write_records(root, DAY);
    fs::set_permissions(file(root, "body"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(counts.written, 0);
    assert_eq!(counts.refused.len(), 1);
    assert_eq!(counts.refused[0].0, "body");
    assert!(rows(root, "other").is_empty());
}
