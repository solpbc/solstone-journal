// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;

use chrono_tz::America::Denver;
use solstone_core_format::content::body_records::{BodyRow, RowKind, decode_rows};

use super::*;

const WATCH: &str = r#"sourceName="Synthetic Watch" sourceVersion="11.0" device="&lt;&lt;HKDevice: 0x1&gt;, name:Watch, uuid:ABC-DEF&gt;" creationDate="2026-10-10 08:00:00 -0600""#;

fn export_xml(records: &[String]) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<HealthData locale=\"en_US\">\n<ExportDate value=\"2026-10-12 08:00:00 -0600\"/>\n<Me HKCharacteristicTypeIdentifierBiologicalSex=\"x\"/>\n{}\n</HealthData>\n",
        records.join("\n")
    )
}

fn heart(at: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKQuantityTypeIdentifierHeartRate" {WATCH} unit="count/min" startDate="{at}" endDate="{at}" value="{value}"/>"#
    )
}

fn stage(start: &str, end: &str, value: &str) -> String {
    format!(
        r#"<Record type="HKCategoryTypeIdentifierSleepAnalysis" {WATCH} startDate="{start}" endDate="{end}" value="HKCategoryValueSleepAnalysis{value}"/>"#
    )
}

fn fixture() -> Vec<String> {
    vec![
        heart("2026-10-10 06:31:10 -0600", "4817"),
        heart("2026-10-10 06:36:10 -0600", "4818"),
        format!(
            r#"<Record type="HKQuantityTypeIdentifierStepCount" {WATCH} unit="count" startDate="2026-10-10 06:58:00 -0600" endDate="2026-10-10 07:12:00 -0600" value="1200"/>"#
        ),
        format!(
            r#"<Record type="HKQuantityTypeIdentifierActiveEnergyBurned" {WATCH} unit="kcal" startDate="2026-10-10 07:00:00 -0600" endDate="2026-10-10 07:01:00 -0600" value="9"/>"#
        ),
        r#"<Record type="HKQuantityTypeIdentifierMadeUpKind" sourceName="App" unit="x" startDate="2026-10-10 07:00:00 -0600" endDate="2026-10-10 07:01:00 -0600" value="1"/>"#.to_owned(),
        r#"<Record type="HKQuantityTypeIdentifierDietaryEnergyConsumed" sourceName="Food App" unit="kcal" creationDate="2026-10-10 12:00:00 -0600" startDate="2026-10-10 12:00:00 -0600" endDate="2026-10-10 12:00:00 -0600" value="312"><MetadataEntry key="HKFoodType" value="Oatmeal"/></Record>"#.to_owned(),
        r#"<Record type="HKCategoryTypeIdentifierHeadache" sourceName="Health" creationDate="2026-10-10 15:00:00 -0600" startDate="2026-10-10 14:00:00 -0600" endDate="2026-10-10 15:00:00 -0600" value="HKCategoryValueSeverityMild"/>"#.to_owned(),
        format!(
            r#"<Record type="HKCategoryTypeIdentifierMindfulSession" {WATCH} startDate="2026-10-10 12:30:00 -0600" endDate="2026-10-10 12:40:00 -0600" value="HKCategoryValueNotApplicable"/>"#
        ),
        stage("2026-10-10 22:40:00 -0600", "2026-10-10 23:40:00 -0600", "AsleepCore"),
        stage("2026-10-10 23:50:00 -0600", "2026-10-11 01:00:00 -0600", "AsleepDeep"),
        stage("2026-10-11 01:10:00 -0600", "2026-10-11 06:10:00 -0600", "AsleepREM"),
        // More than 30 minutes on, so another night.
        stage("2026-10-11 13:00:00 -0600", "2026-10-11 13:30:00 -0600", "AsleepCore"),
        format!(
            r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" {WATCH} duration="49" durationUnit="min" totalDistance="8.1" totalDistanceUnit="km" totalEnergyBurned="512" totalEnergyBurnedUnit="kcal" startDate="2026-10-10 06:31:00 -0600" endDate="2026-10-10 07:20:00 -0600"><MetadataEntry key="HKWorkoutBrandName" value="Douro Loop"/><MetadataEntry key="HKExternalUUID" value="uuid-secret"/><WorkoutEvent type="HKWorkoutEventTypePause" date="2026-10-10 06:50:00 -0600"/><WorkoutStatistics type="HKQuantityTypeIdentifierActiveEnergyBurned" startDate="2026-10-10 06:31:00 -0600" endDate="2026-10-10 07:20:00 -0600" sum="512" unit="kcal"/><WorkoutRoute sourceName="x"><MetadataEntry key="HKMetadataKeySyncVersion" value="1"/><FileReference path="/route.gpx"/></WorkoutRoute></Workout>"#
        ),
        r#"<Workout workoutActivityType="HKWorkoutActivityTypeWalking" sourceName="App" startDate="2026-10-10 09:00:00 -0600" endDate="2026-10-10 08:00:00 -0600"/>"#.to_owned(),
        r#"<Workout workoutActivityType="HKWorkoutActivityTypeWalking" sourceName="App" startDate="2026-01-01 09:00:00 -0600" endDate="2026-03-01 08:00:00 -0600"/>"#.to_owned(),
        r#"<ActivitySummary dateComponents="2026-10-10" activeEnergyBurned="400"/>"#.to_owned(),
        // No end at all, and a logged span that ends before it starts: counted, not dropped.
        r#"<Record type="HKQuantityTypeIdentifierStepCount" sourceName="App" startDate="2026-10-10 10:00:00 -0600" value="5"/>"#.to_owned(),
        r#"<Record type="HKCategoryTypeIdentifierHeadache" sourceName="App" startDate="2026-10-10 11:00:00 -0600" endDate="2026-10-10 10:00:00 -0600" value="HKCategoryValueSeverityMild"/>"#.to_owned(),
        r#"<Workout workoutActivityType="HKWorkoutActivityTypeRowing" sourceName="App" startDate="2026-10-10 16:00:00 -0600" endDate="2026-10-10 16:10:00 -0600"><WorkoutEvent type="HKWorkoutEventTypePause" date="not a date"/></Workout>"#.to_owned(),
    ]
}

fn read(records: &[String]) -> AppleExport {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("export.xml"), export_xml(records)).unwrap();
    read_export(directory.path()).unwrap()
}

fn rows_of(plan: &Plan, day: &str, key: &str) -> Vec<BodyRow> {
    let file = plan
        .files
        .iter()
        .find(|file| file.day == day && file.segment == key)
        .unwrap_or_else(|| panic!("{day}/{key} not planned"));
    decode_rows(&file.contents).unwrap()
}

fn experience<'a>(export: &'a AppleExport, kind: &str) -> Vec<&'a BodyRow> {
    export
        .experiences
        .iter()
        .filter(|row| row.experience.as_deref() == Some(kind))
        .collect()
}

#[test]
fn only_the_read_list_is_read_and_everything_else_is_counted_and_named() {
    let export = read(&fixture());
    assert_eq!(export.read["heart rate"], 2);
    assert_eq!(export.read["step count"], 1);
    assert_eq!(export.read["dietary energy consumed"], 1);
    assert_eq!(export.read["headache"], 1);
    for name in [
        "HKQuantityTypeIdentifierActiveEnergyBurned",
        "HKQuantityTypeIdentifierMadeUpKind",
        "ActivitySummary",
        "WorkoutRoute",
        "ExportDate",
        "Me",
    ] {
        assert_eq!(export.not_read.get(name), Some(&1), "{name}");
    }
    assert_eq!(export.refused["end_before_start"], 2);
    assert_eq!(export.refused["time"], 1);
    assert_eq!(export.refused["event_time"], 1);
    assert_eq!(export.refused["span_too_long"], 1);
    let every_row = serde_json::to_string(&export.rows).unwrap();
    assert!(!every_row.contains("ActiveEnergyBurned"));
    assert!(!every_row.contains("route.gpx"));
}

#[test]
fn a_workout_keeps_every_field_and_tiles_its_real_span() {
    let export = read(&fixture());
    let workouts = experience(&export, "workout");
    let workout = workouts.iter().find(|row| row.label == "running").unwrap();
    assert_eq!(workout.label, "running");
    assert_eq!(workout.words, ["Douro Loop"]);
    assert_eq!(workout.values["distance"], "8.1 km");
    assert_eq!(workout.values["active energy burned sum"], "512 kcal");
    assert_eq!(workout.fields["metadata"]["HKExternalUUID"], "uuid-secret");
    assert_eq!(workout.fields["statistics"][0]["sum"], "512");
    assert!(
        workout.fields["device"]
            .as_str()
            .unwrap()
            .contains("ABC-DEF")
    );

    let journal = tempfile::tempdir().unwrap();
    let plan = plan(journal.path(), &export, Denver, None, false).unwrap();
    let first = rows_of(&plan, "20261010", "063000_300");
    assert!(first.iter().any(|row| row.kind == RowKind::Experience));
    let tile = rows_of(&plan, "20261010", "071500_300");
    let tile = tile
        .iter()
        .find(|row| row.kind == RowKind::ExperienceTile && row.of.as_ref() == Some(&workout.id))
        .unwrap();
    assert_eq!(tile.index, Some(9));
    assert_eq!(tile.count, Some(10));
    assert!(
        tile.values.is_empty() && tile.fields.is_empty(),
        "no totals on a tile"
    );
    let event = rows_of(&plan, "20261010", "065000_300");
    assert!(event.iter().any(|row| row.kind == RowKind::Event));
}

#[test]
fn a_night_is_this_sources_adjacent_stages_and_says_so() {
    let export = read(&fixture());
    let nights = experience(&export, "sleep");
    assert_eq!(
        nights.len(),
        2,
        "a gap over 30 minutes starts another night"
    );
    let night = nights
        .iter()
        .find(|row| row.start.as_deref() == Some("2026-10-10 22:40:00 -0600"))
        .unwrap();
    assert_eq!(night.end.as_deref(), Some("2026-10-11 06:10:00 -0600"));
    let assembled = night.assembled.as_deref().unwrap();
    assert!(assembled.contains("not a session"));
    assert!(
        !assembled
            .chars()
            .any(|c| c.is_ascii_digit() && c != '3' && c != '0')
    );

    let journal = tempfile::tempdir().unwrap();
    let plan = plan(journal.path(), &export, Denver, None, false).unwrap();
    // Crosses midnight, tiled on both days, with a stage where it starts.
    let first = rows_of(&plan, "20261010", "224000_300");
    assert!(first.iter().any(|row| row.kind == RowKind::Experience));
    assert!(
        first
            .iter()
            .any(|row| row.kind == RowKind::Stage && row.label == "asleep core")
    );
    let next_day = rows_of(&plan, "20261011", "000000_300");
    assert!(
        next_day
            .iter()
            .any(|row| row.kind == RowKind::ExperienceTile && row.of.as_ref() == Some(&night.id))
    );
}

#[test]
fn counts_stay_unsplit_and_logged_rows_keep_their_shape_and_words() {
    let export = read(&fixture());
    let journal = tempfile::tempdir().unwrap();
    let plan = plan(journal.path(), &export, Denver, None, false).unwrap();
    let count = rows_of(&plan, "20261010", "065500_300");
    let count = count.iter().find(|row| row.kind == RowKind::Count).unwrap();
    assert_eq!(count.value.as_deref(), Some("1200"));
    assert_eq!(count.end.as_deref(), Some("2026-10-10 07:12:00 -0600"));
    assert!(
        plan.files
            .iter()
            .filter(|file| file.contents.contains("1200"))
            .count()
            == 1
    );

    let point = rows_of(&plan, "20261010", "120000_300");
    let point = point
        .iter()
        .find(|row| row.kind == RowKind::Logged)
        .unwrap();
    assert!(point.end.is_none());
    assert_eq!(point.words, ["Oatmeal"]);
    let interval = rows_of(&plan, "20261010", "140000_300");
    let interval = interval
        .iter()
        .find(|row| row.kind == RowKind::Logged)
        .unwrap();
    assert_eq!(interval.end.as_deref(), Some("2026-10-10 15:00:00 -0600"));
}

#[test]
fn a_dated_row_lands_in_its_whole_day_window_with_no_time() {
    let mut export = AppleExport::default();
    let mut row = BodyRow::new(
        RowKind::Dated,
        "d".into(),
        "oura",
        "daily_readiness",
        "readiness score".into(),
    );
    row.date = Some("2026-10-10".into());
    row.value = Some("82".into());
    export.rows.push(row);
    let journal = tempfile::tempdir().unwrap();
    let plan = plan(journal.path(), &export, Denver, None, false).unwrap();
    assert_eq!(plan.files[0].day, "20261010");
    assert_eq!(plan.files[0].segment, DATED_WINDOW);
}

#[test]
fn the_same_data_in_another_order_is_the_same_bytes() {
    let same_second = |value: &str, food: &str| {
        format!(
            r#"<Record type="HKQuantityTypeIdentifierDietaryEnergyConsumed" sourceName="Food App" unit="kcal" creationDate="2026-10-10 12:00:00 -0600" startDate="2026-10-10 12:00:00 -0600" endDate="2026-10-10 12:00:00 -0600" value="{value}"><MetadataEntry key="HKFoodType" value="{food}"/></Record>"#
        )
    };
    let mut records = fixture();
    records.push(same_second("100", "Toast"));
    records.push(same_second("200", "Butter"));
    records.push(same_second("200", "Butter"));
    let export = read(&records);
    let journal = tempfile::tempdir().unwrap();
    let forward = plan(journal.path(), &export, Denver, None, false).unwrap();

    let mut shuffled = records.clone();
    shuffled.reverse();
    let export = read(&shuffled);
    let backward = plan(journal.path(), &export, Denver, None, false).unwrap();
    assert_eq!(forward.files, backward.files);

    let lunch = rows_of(&forward, "20261010", "120000_300");
    let foods: Vec<_> = lunch.iter().flat_map(|row| row.words.clone()).collect();
    for food in ["Oatmeal", "Toast", "Butter"] {
        assert_eq!(
            foods.iter().filter(|word| *word == food).count(),
            1,
            "{food}"
        );
    }
}

#[test]
fn two_events_at_one_moment_both_stay_in_any_order() {
    let workout = |events: &str| {
        format!(
            r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" sourceName="App" startDate="2026-10-10 06:30:00 -0600" endDate="2026-10-10 06:40:00 -0600">{events}</Workout>"#
        )
    };
    let one = r#"<WorkoutEvent type="HKWorkoutEventTypeSegment" date="2026-10-10 06:30:00 -0600" duration="5"/>"#;
    let two = r#"<WorkoutEvent type="HKWorkoutEventTypeSegment" date="2026-10-10 06:30:00 -0600" duration="8"/>"#;
    let forward = read(&[workout(&format!("{one}{two}{one}"))]);
    let backward = read(&[workout(&format!("{two}{one}"))]);
    let events = |export: &AppleExport| {
        let mut rows: Vec<_> = export
            .rows
            .iter()
            .filter(|row| row.kind == RowKind::Event)
            .cloned()
            .collect();
        sort_rows(&mut rows);
        rows
    };
    assert_eq!(events(&forward).len(), 2);
    assert_eq!(events(&forward), events(&backward));
}

#[test]
fn a_foreign_file_turning_up_doesnt_move_records_already_there() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    let dir = root
        .join("chronicle/20261010")
        .join(STREAM)
        .join("063000_300");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(BODY_RECORDS_FILE), "").unwrap();
    fs::write(dir.join("conflicted copy.txt"), "").unwrap();
    assert_eq!(resolve_key(root, "20261010", "063000_300"), "063000_300");
}

#[test]
fn a_dated_row_without_a_real_date_refuses_the_plan() {
    let mut export = AppleExport::default();
    let mut row = BodyRow::new(RowKind::Dated, "d".into(), "oura", "t", "t".into());
    row.date = Some("../../etc".into());
    export.rows.push(row);
    let journal = tempfile::tempdir().unwrap();
    assert!(plan(journal.path(), &export, Denver, None, false).is_err());
}

#[test]
fn identity_is_the_instant_so_another_offset_changes_nothing() {
    let export = read(&[heart("2026-10-10 06:31:10 -0600", "70")]);
    let elsewhere = read(&[heart("2026-10-10 14:31:10 +0200", "70")]);
    assert_eq!(export.rows[0].id, elsewhere.rows[0].id);
}

#[test]
fn a_tombstone_that_cant_say_what_it_removed_stays_deleted() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    let window = root
        .join("chronicle/20261010")
        .join(STREAM)
        .join("000000_300");
    fs::create_dir_all(&window).unwrap();
    fs::write(window.join("tombstone.json"), "{}").unwrap();
    let export = read(&[heart("2026-10-10 00:01:00 -0600", "70")]);
    let planned = plan(root, &export, Denver, None, false).unwrap();
    assert!(planned.files.is_empty());
    assert_eq!(planned.stayed_deleted, 1);
}

#[test]
fn a_window_lands_beside_older_content_and_stays_there() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    let card = root
        .join("chronicle/20261010")
        .join(STREAM)
        .join("000000_300");
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("day_summary_transcript.md"), "a card\n").unwrap();
    let export = read(&[heart("2026-10-10 00:01:00 -0600", "70")]);
    let planned = plan(root, &export, Denver, None, false).unwrap();
    assert_eq!(planned.files[0].segment, "000000_301");

    // Once it's there, the card going away doesn't move it back.
    let beside = root
        .join("chronicle/20261010")
        .join(STREAM)
        .join("000000_301");
    fs::create_dir_all(&beside).unwrap();
    fs::write(beside.join(BODY_RECORDS_FILE), &planned.files[0].contents).unwrap();
    fs::remove_dir_all(&card).unwrap();
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("tombstone.json"), "{}").unwrap();
    assert_eq!(resolve_key(root, "20261010", "000000_300"), "000000_301");
}

#[test]
fn a_window_that_cant_be_read_is_left_alone_and_counted() {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    let dir = root
        .join("chronicle/20261010")
        .join(STREAM)
        .join("063000_300");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(BODY_RECORDS_FILE), "{broken\n").unwrap();
    let export = read(&[heart("2026-10-10 06:31:10 -0600", "70")]);
    let planned = plan(root, &export, Denver, None, false).unwrap();
    assert!(planned.files.is_empty() && planned.publish.is_empty());
    assert_eq!(planned.unreadable, 1);
}

#[test]
fn the_summary_names_and_counts_and_holds_no_value() {
    let export = read(&fixture());
    let journal = tempfile::tempdir().unwrap();
    let planned = plan(journal.path(), &export, Denver, None, false).unwrap();
    let summary = summary_text(&export, &planned, 3, None, Denver);
    assert!(summary.contains("HKQuantityTypeIdentifierActiveEnergyBurned: 1"));
    assert!(summary.contains("end_before_start: 2"));
    assert!(summary.contains("no facet"));
    assert!(!summary.contains("4817"));
    assert!(summary_text(&export, &planned, 3, Some("health"), Denver).contains("health facet"));
}
