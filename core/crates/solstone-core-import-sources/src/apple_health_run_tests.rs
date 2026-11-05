// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;

use chrono_tz::America::Denver;
use solstone_core_format::content::body_records::decode_rows;

use super::*;
use crate::apple_health::{decide, plan_with, read_export};

const WATCH: &str = r#"sourceName="Synthetic Watch" sourceVersion="11.0" device="&lt;&lt;HKDevice: 0x1&gt;, name:Watch&gt;" creationDate="2026-10-10 08:00:00 -0600""#;
const RING: &str = r#"sourceName="Other Ring" sourceVersion="2""#;

fn export_xml(records: &[String]) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<HealthData locale=\"en_US\">\n<ExportDate value=\"2026-10-12 08:00:00 -0600\"/>\n{}\n</HealthData>\n",
        records.join("\n")
    )
}

fn heart(at: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKQuantityTypeIdentifierHeartRate" {WATCH} unit="count/min" startDate="{at}" endDate="{at}" value="{value}"/>"#
    )
}

fn stage(source: &str, start: &str, end: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKCategoryTypeIdentifierSleepAnalysis" {source} startDate="{start}" endDate="{end}" value="HKCategoryValueSleepAnalysis{value}"/>"#
    )
}

fn at(minutes: i64) -> String {
    let base =
        chrono::DateTime::parse_from_str("2026-10-09 00:00:00 -0600", "%Y-%m-%d %H:%M:%S %z")
            .unwrap();
    (base + chrono::Duration::minutes(minutes))
        .format("%Y-%m-%d %H:%M:%S %z")
        .to_string()
}

/// Everything the run has to get right in one export: identity groups with
/// repeats and edits, two sources' nights interleaved, a night across midnight,
/// a night too long to keep, workouts with events, counts, logged entries and
/// rows that aren't read.
fn records() -> Vec<String> {
    let mut records = Vec::new();
    for minute in (0..3 * 24 * 60).step_by(7) {
        records.push(heart(&at(minute), &format!("{}", 60 + minute % 30)));
    }
    // An exact repeat, and the same identity with another value.
    records.push(heart(&at(70), "70"));
    records.push(heart(&at(70), "71"));
    records.push(heart(&at(70), "72"));
    // Two sources' nights, interleaved, the watch's across midnight.
    for (index, value) in ["AsleepCore", "AsleepDeep", "AsleepREM", "AsleepCore"]
        .iter()
        .enumerate()
    {
        let start = 22 * 60 + 40 + index as i64 * 70;
        records.push(stage(WATCH, &at(start), &at(start + 60), value));
        records.push(stage(RING, &at(start + 5), &at(start + 50), value));
    }
    // Two stages starting in the same second.
    records.push(stage(WATCH, &at(22 * 60 + 40), &at(22 * 60 + 45), "Awake"));
    // A chain of stages over fifteen days: refused, and its stages must not land.
    for step in 0..(15 * 24 * 3) {
        let start = 4 * 24 * 60 + step * 20;
        records.push(stage(
            r#"sourceName="Odd App""#,
            &at(start),
            &at(start + 15),
            "AsleepCore",
        ));
    }
    records.push(format!(
        r#"<Record type="HKQuantityTypeIdentifierStepCount" {WATCH} unit="count" startDate="{}" endDate="{}" value="1200"/>"#,
        at(400),
        at(414)
    ));
    records.push(format!(
        r#"<Record type="HKQuantityTypeIdentifierActiveEnergyBurned" {WATCH} unit="kcal" startDate="{}" endDate="{}" value="9"/>"#,
        at(400),
        at(401)
    ));
    records.push(format!(
        r#"<Record type="HKQuantityTypeIdentifierDietaryWater" sourceName="Water" unit="mL" creationDate="{}" startDate="{}" endDate="{}" value="250"><MetadataEntry key="HKMetadataKeySyncIdentifier" value="w-1"/></Record>"#,
        at(600),
        at(600),
        at(600)
    ));
    records.push(format!(
        r#"<Record type="HKCategoryTypeIdentifierHeadache" sourceName="Health" creationDate="{}" startDate="{}" endDate="{}" value="HKCategoryValueSeverityMild"/>"#,
        at(900),
        at(840),
        at(900)
    ));
    records.push(format!(
        r#"<Record type="HKCategoryTypeIdentifierMindfulSession" {WATCH} startDate="{}" endDate="{}" value="HKCategoryValueNotApplicable"/>"#,
        at(750),
        at(760)
    ));
    for day in 0..3 {
        let start = day * 24 * 60 + 6 * 60 + 31;
        records.push(format!(
            r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" {WATCH} duration="49" durationUnit="min" totalDistance="8.1" totalDistanceUnit="km" startDate="{}" endDate="{}"><MetadataEntry key="HKWorkoutBrandName" value="Loop {day}"/><WorkoutEvent type="HKWorkoutEventTypeSegment" date="{}"/><WorkoutEvent type="HKWorkoutEventTypeSegment" date="{}" duration="1"/><WorkoutEvent type="HKWorkoutEventTypePause" date="{}"/><WorkoutStatistics type="HKQuantityTypeIdentifierActiveEnergyBurned" startDate="{}" endDate="{}" sum="512" unit="kcal"/></Workout>"#,
            at(start),
            at(start + 49),
            at(start + 10),
            at(start + 10),
            at(start + 20),
            at(start),
            at(start + 49)
        ));
    }
    records.push(format!(
        r#"<Workout workoutActivityType="HKWorkoutActivityTypeWalking" sourceName="App" startDate="{}" endDate="{}"/>"#,
        at(100),
        at(50)
    ));
    records.push(
        r#"<ActivitySummary dateComponents="2026-10-10" activeEnergyBurned="400"/>"#.to_owned(),
    );
    records
}

/// Fisher-Yates over a seeded linear congruential sequence: no dependency, same
/// order for the same seed.
fn shuffle(items: &mut [String], seed: u64) {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    for index in (1..items.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let pick = usize::try_from(state >> 33).unwrap() % (index + 1);
        items.swap(index, pick);
    }
}

fn write_export(records: &[String]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("export.xml"), export_xml(records)).unwrap();
    directory
}

type Windows = BTreeMap<WindowId, String>;

/// What the whole-export reference would write, with its counts.
fn reference(journal: &Path, source: &Path) -> (Windows, ScanCounts) {
    let export = read_export(source).unwrap();
    let plan = plan_with(journal, &export, Denver, None, false).unwrap();
    let windows = plan
        .files
        .iter()
        .map(|file| {
            (
                (file.day.clone(), file.segment.clone()),
                file.contents.clone(),
            )
        })
        .collect();
    let counts = ScanCounts {
        read: export.read,
        not_read: export.not_read,
        refused: export.refused,
        rows: 0,
    };
    (windows, counts)
}

/// What the streaming run would write, spilling every few rows.
fn streamed(journal: &Path, source: &Path, spill: usize) -> (Windows, ScanCounts) {
    let staging = tempfile::tempdir().unwrap();
    let scan_dir = staging.path().join("scan");
    let placed_dir = staging.path().join("placed");
    let mut counts = scan(source, &scan_dir, spill, &mut |_| {}).unwrap();
    assert!(runs_in(&scan_dir).unwrap().len() > 1, "the scan spilled");
    let placed = place(journal, Denver, &scan_dir, &placed_dir, spill, &mut |_| {}).unwrap();
    for (stage, count) in &placed.refused {
        *counts.refused.entry(stage.clone()).or_default() += count;
    }
    let mut days = Days::open(journal, &placed_dir, &placed.refused_nights, None).unwrap();
    let mut windows = Windows::new();
    while let Some((_, day, _)) = days.next_day().unwrap() {
        for (window, rows) in day {
            let decision = decide(journal, &window, rows, None, None).unwrap();
            if let Some(contents) = decision.contents {
                windows.insert(window, contents);
            }
        }
    }
    counts.rows = 0;
    (windows, counts)
}

#[test]
fn streaming_writes_what_the_whole_export_reference_writes_in_any_order() {
    let journal = tempfile::tempdir().unwrap();
    let base = records();
    let expected = reference(journal.path(), write_export(&base).path());
    assert!(expected.0.len() > 40, "the fixture spans many windows");
    assert_eq!(expected.1.refused.get("span_too_long"), Some(&1));
    // The fixture puts some readings on a session's tiles and some on hours, so
    // both placers must agree on which.
    let on = |length: &str| {
        expected.0.iter().any(|((_, key), contents)| {
            key.ends_with(length)
                && decode_rows(contents)
                    .unwrap()
                    .iter()
                    .any(|row| row.kind == RowKind::Reading)
        })
    };
    assert!(on("_300") && on("_3600"));
    for seed in 0..4_u64 {
        let mut shuffled = base.clone();
        shuffle(&mut shuffled, seed);
        let source = write_export(&shuffled);
        let got = streamed(journal.path(), source.path(), 20_000);
        assert_eq!(got.1, expected.1, "counts, seed {seed}");
        assert_eq!(
            got.0.keys().collect::<Vec<_>>(),
            expected.0.keys().collect::<Vec<_>>(),
            "windows, seed {seed}"
        );
        assert_eq!(got.0, expected.0, "bytes, seed {seed}");
    }
}

/// Minutes from the fixture's base when its first run starts, and how long it is.
const RUN_START: i64 = 6 * 60 + 31;
const RUN_LENGTH: i64 = 49;

fn phone_heart(at: &str) -> String {
    format!(
        r#"<Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Synthetic iPhone" unit="count/min" startDate="{at}" endDate="{at}" value="88"/>"#
    )
}

#[test]
fn a_sessions_own_readings_go_on_its_tiles_and_everything_else_on_hours() {
    let journal = tempfile::tempdir().unwrap();
    let mut records = records();
    // On the run's first second, on its last, and from another source inside it.
    records.push(heart(&at(RUN_START), "101"));
    records.push(heart(&at(RUN_START + RUN_LENGTH), "102"));
    records.push(phone_heart(&at(RUN_START + 20)));
    let (windows, _) = streamed(journal.path(), write_export(&records).path(), 20_000);
    let mut tiled = 0;
    for ((_, key), contents) in &windows {
        let grid = key.ends_with("_300");
        for row in decode_rows(contents).unwrap() {
            let sleep = row.experience.as_deref() == Some("sleep") || row.kind == RowKind::Stage;
            let session_own = !sleep
                && matches!(
                    row.kind,
                    RowKind::Experience | RowKind::ExperienceTile | RowKind::Event
                );
            if row.kind == RowKind::Reading && grid {
                tiled += 1;
                assert_eq!(row.by.as_deref(), Some("Synthetic Watch"), "{key}");
            } else if row.date.is_none() {
                assert_eq!(session_own, grid, "{key}: {:?} {:?}", row.kind, row.label);
            }
        }
    }
    assert!(tiled > 0);
    let holding = |value: &str| {
        windows
            .iter()
            .find(|(_, contents)| contents.contains(&format!(r#""value":"{value}""#)))
            .map(|((_, key), _)| key.clone())
            .unwrap()
    };
    assert!(
        holding("101").ends_with("_300"),
        "a reading on the run's start"
    );
    assert!(
        holding("102").ends_with("_3600"),
        "a reading on the run's end"
    );
    assert!(holding("88").ends_with("_3600"), "another source's reading");
}

#[test]
fn a_session_sharing_a_readings_millisecond_still_takes_it() {
    // A workout's identity sorts after a reading's, so without a session first in
    // the merge the reading would be placed before the workout is open.
    let journal = tempfile::tempdir().unwrap();
    let records = vec![
        heart(&at(RUN_START), "101"),
        format!(
            r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" {WATCH} startDate="{}" endDate="{}"/>"#,
            at(RUN_START),
            at(RUN_START + RUN_LENGTH)
        ),
    ];
    let (windows, _) = streamed(journal.path(), write_export(&records).path(), 1);
    let reading = windows
        .iter()
        .find(|(_, contents)| contents.contains(r#""value":"101""#))
        .map(|((_, key), _)| key.clone())
        .unwrap();
    assert_eq!(reading, "063000_300");
}

#[test]
fn a_sessions_last_reading_stays_on_its_tile_when_the_next_record_is_later() {
    // The reading's group is placed only when a later identity arrives; the
    // session must still be open for it.
    let journal = tempfile::tempdir().unwrap();
    let records = vec![
        format!(
            r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" {WATCH} startDate="{}" endDate="{}"/>"#,
            at(RUN_START),
            at(RUN_START + RUN_LENGTH)
        ),
        heart(&at(RUN_START + RUN_LENGTH - 1), "103"),
        heart(&at(RUN_START + 3 * 60), "104"),
    ];
    let (windows, _) = streamed(journal.path(), write_export(&records).path(), 1);
    let key = |value: &str| {
        windows
            .iter()
            .find(|(_, contents)| contents.contains(&format!(r#""value":"{value}""#)))
            .map(|((_, key), _)| key.clone())
            .unwrap()
    };
    assert_eq!(key("103"), "071500_300");
    assert_eq!(key("104"), "090000_3600");
}

#[test]
fn a_too_long_nights_stages_never_land() {
    let journal = tempfile::tempdir().unwrap();
    let (windows, counts) = streamed(journal.path(), write_export(&records()).path(), 20_000);
    assert_eq!(counts.refused.get("span_too_long"), Some(&1));
    for contents in windows.values() {
        assert!(
            !contents.contains("Odd App"),
            "a refused night's stage landed"
        );
    }
}

#[test]
fn two_workouts_differing_only_in_events_keep_the_same_one_in_any_order() {
    let journal = tempfile::tempdir().unwrap();
    let workout = |event: &str| {
        format!(
            r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" {WATCH} startDate="{}" endDate="{}"><WorkoutEvent type="{event}" date="{}"/></Workout>"#,
            at(60),
            at(90),
            at(70)
        )
    };
    let records = vec![
        workout("HKWorkoutEventTypePause"),
        workout("HKWorkoutEventTypeSegment"),
    ];
    let mut reversed = records.clone();
    reversed.reverse();
    let forward = streamed(journal.path(), write_export(&records).path(), 1);
    let backward = streamed(journal.path(), write_export(&reversed).path(), 1);
    assert_eq!(forward.0, backward.0);
    let all: String = forward.0.values().cloned().collect();
    assert!(all.contains("HKWorkoutEventTypeSegment"));
    assert!(!all.contains("HKWorkoutEventTypePause"));
}

#[test]
fn days_resume_from_a_cursor() {
    let journal = tempfile::tempdir().unwrap();
    let source = write_export(&records());
    let staging = tempfile::tempdir().unwrap();
    let scan_dir = staging.path().join("scan");
    let placed_dir = staging.path().join("placed");
    scan(source.path(), &scan_dir, 20_000, &mut |_| {}).unwrap();
    let placed = place(
        journal.path(),
        Denver,
        &scan_dir,
        &placed_dir,
        20_000,
        &mut |_| {},
    )
    .unwrap();
    let mut all = Vec::new();
    let mut days = Days::open(journal.path(), &placed_dir, &placed.refused_nights, None).unwrap();
    while let Some((day, _, _)) = days.next_day().unwrap() {
        all.push(day);
    }
    assert!(all.len() > 3);
    assert_eq!(placed.days.iter().cloned().collect::<Vec<_>>(), all);
    let after = all[1].clone();
    let mut later = Vec::new();
    let mut days = Days::open(
        journal.path(),
        &placed_dir,
        &placed.refused_nights,
        Some(after),
    )
    .unwrap();
    while let Some((day, _, _)) = days.next_day().unwrap() {
        later.push(day);
    }
    assert_eq!(later, all[2..]);
}
