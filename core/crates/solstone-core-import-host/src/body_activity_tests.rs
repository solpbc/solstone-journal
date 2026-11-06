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
