// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::Path;

use serde_json::json;
use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};

use super::*;

const DAY: &str = "20261010";
const STREAM: &str = "import.apple_health";

fn workout(id: &str, by: &str, entered: bool) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Experience,
        id.to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some("workout".to_owned());
    row.start = Some("2026-10-10 07:00:00 -0600".to_owned());
    row.end = Some("2026-10-10 07:40:00 -0600".to_owned());
    row.by = Some(by.to_owned());
    row.values
        .insert("distance".to_owned(), "5.1 km".to_owned());
    if entered {
        row.fields
            .insert("metadata".to_owned(), json!({"HKWasUserEntered": "1"}));
    }
    row
}

/// A shared day: a capture segment, and one Apple workout with its record.
fn journal(entered: bool) -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"America/Denver"}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        r#"{"identity":{"timezone":"America/Denver"}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("facets/body")).unwrap();
    fs::write(root.join("facets/body/facet.json"), r#"{"title":"Body"}"#).unwrap();
    let capture = root.join(format!("chronicle/{DAY}/phone/090000_300"));
    fs::create_dir_all(&capture).unwrap();
    fs::write(capture.join("audio.jsonl"), "{}\n").unwrap();
    let row = workout("w1", "Synthetic Watch", entered);
    let window = root.join(format!("chronicle/{DAY}/{STREAM}/070000_3600"));
    fs::create_dir_all(&window).unwrap();
    fs::write(
        window.join("body.jsonl"),
        encode_rows(std::slice::from_ref(&row)),
    )
    .unwrap();
    let record = json!({"id":"body_w1","source":"body","activity":"workout",
        "account":{"key":format!("{STREAM}\u{1f}w1"),"start":row.start,"end":row.end,"type":"running",
            "recorder":recorder_id(STREAM, row.by.as_deref()),"arrived":DAY}});
    fs::create_dir_all(root.join("facets/body/activities")).unwrap();
    fs::write(
        root.join(format!("facets/body/activities/{DAY}.jsonl")),
        format!("{record}\n"),
    )
    .unwrap();
    journal
}

fn admitted(root: &Path) -> String {
    match body_block(root, BODY_HOOK, DAY).state {
        BodyState::Admitted(text) => text,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_shared_day_admits_its_facts_and_only_the_briefing_reads_them() {
    let journal = journal(false);
    let text = admitted(journal.path());
    assert!(
        text.contains("Synthetic Watch") && text.contains("5.1 km"),
        "{text}"
    );
    assert!(
        text.contains("`yesterday` only, never in `needs_attention`"),
        "{text}"
    );
    assert!(
        !text.split("```json").nth(1).unwrap().contains("\"logged\""),
        "{text}"
    );
    assert_eq!(
        body_block(journal.path(), "entities:entity_observer", DAY).state,
        BodyState::None
    );
}

#[test]
fn logged_only_where_the_source_marks_it() {
    let journal = journal(true);
    assert!(
        admitted(journal.path())
            .split("```json")
            .nth(1)
            .unwrap()
            .contains("\"logged\": true")
    );
}

#[test]
fn a_body_only_or_unlistable_day_admits_nothing() {
    let journal = journal(false);
    fs::remove_dir_all(journal.path().join(format!("chronicle/{DAY}/phone"))).unwrap();
    assert_eq!(
        body_block(journal.path(), BODY_HOOK, DAY).state,
        BodyState::None
    );
    // A stray file in the body window doesn't make the day shared.
    fs::write(
        journal
            .path()
            .join(format!("chronicle/{DAY}/{STREAM}/070000_3600/notes.txt")),
        "x",
    )
    .unwrap();
    assert_eq!(
        body_block(journal.path(), BODY_HOOK, DAY).state,
        BodyState::None
    );
    let empty = tempfile::tempdir().unwrap();
    fs::create_dir_all(empty.path().join("imports")).unwrap();
    fs::write(empty.path().join("imports/.apple_health-zone.json"), "{}").unwrap();
    assert_eq!(
        body_block(empty.path(), BODY_HOOK, DAY).state,
        BodyState::None
    );
}

#[test]
fn a_deleted_account_gives_nothing_and_an_unreadable_read_gives_its_own_state() {
    let journal = journal(false);
    let window = journal
        .path()
        .join(format!("chronicle/{DAY}/{STREAM}/070000_3600"));
    fs::remove_file(window.join("body.jsonl")).unwrap();
    fs::write(
        window.join("tombstone.json"),
        r#"{"manifest":["body.jsonl"]}"#,
    )
    .unwrap();
    assert_eq!(
        body_block(journal.path(), BODY_HOOK, DAY).state,
        BodyState::None
    );

    for edge in ["20260925", "20261012"] {
        let journal = self::journal(false);
        let path = journal
            .path()
            .join(format!("facets/body/activities/{edge}.jsonl"));
        fs::write(path, "not json\n").unwrap();
        let block = body_block(journal.path(), BODY_HOOK, DAY);
        assert_eq!(block.state, BodyState::Unreadable, "{edge}");
        assert_eq!(block.fingerprint, UNREADABLE_FINGERPRINT);
    }
}

#[test]
fn a_hide_changes_the_fingerprint_and_a_packet_without_one_reuses_only_where_there_is_none() {
    let journal = journal(false);
    let root = journal.path();
    let admitted_print = body_block(root, BODY_HOOK, DAY).fingerprint;
    let packet = json!({"prepared":{"config":{FINGERPRINT_KEY: admitted_print}}});
    assert_eq!(
        daily_body_freshness(root, BODY_HOOK, DAY, Some(&packet)),
        DailyBodyFreshness::Reuse
    );
    // A HEAD-era packet, with no body fingerprint, on an admitted day: stale.
    let head = json!({"prepared":{"config":{}}});
    assert_eq!(
        daily_body_freshness(root, BODY_HOOK, DAY, Some(&head)),
        DailyBodyFreshness::Stale
    );
    // Hidden: the block goes, and the frozen packet is stale.
    let path = root.join(format!("facets/body/activities/{DAY}.jsonl"));
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("\"source\"", "\"hidden\":true,\"source\"");
    fs::write(&path, text).unwrap();
    assert_eq!(body_block(root, BODY_HOOK, DAY).state, BodyState::None);
    assert_eq!(
        daily_body_freshness(root, BODY_HOOK, DAY, Some(&packet)),
        DailyBodyFreshness::Stale
    );
    // And now a HEAD-era packet reuses.
    assert_eq!(
        daily_body_freshness(root, BODY_HOOK, DAY, Some(&head)),
        DailyBodyFreshness::Reuse
    );
}

#[test]
fn the_facts_carry_no_assessment() {
    let journal = journal(true);
    let text = admitted(journal.path());
    let data = text.split("```json").nth(1).unwrap().to_lowercase();
    for word in [
        "good", "great", "poor", "only", "short", "long", "strong", "weak", "should", "rest",
        "recovery", "progress", "improve", "best", "worst", "goal",
    ] {
        assert!(
            !data
                .split(|c: char| !c.is_alphanumeric())
                .any(|token| token == word),
            "{word} in {data}"
        );
    }
}

#[test]
fn the_block_goes_in_only_when_it_fits_and_its_fingerprint_either_way() {
    let journal = journal(false);
    let block = body_block(journal.path(), BODY_HOOK, DAY);
    let mut config = Map::new();
    attach(&mut config, &block, Some(0));
    assert_eq!(config.get(FINGERPRINT_KEY), Some(&json!(block.fingerprint)));
    assert!(config.get(TEXT_KEY).is_none());
    let mut config = Map::new();
    attach(&mut config, &block, Some(100_000));
    assert!(config.get(TEXT_KEY).is_some());
    let mut config = Map::new();
    attach(&mut config, &BodyBlock::of(BodyState::None), Some(100_000));
    assert!(config.is_empty());
}

#[test]
fn the_block_reaches_the_model_request_after_location() {
    let journal = journal(false);
    let block = body_block(journal.path(), BODY_HOOK, DAY);
    let mut config = Map::new();
    config.insert("transcript".to_owned(), json!("the day's digest"));
    config.insert("_daily_location_text".to_owned(), json!("location words"));
    attach(&mut config, &block, Some(100_000));
    let parts = crate::model_text_parts(&config);
    assert_eq!(parts.len(), 3, "{parts:?}");
    assert_eq!(parts[1], "location words");
    assert!(parts[2].contains("Synthetic Watch"));
    // No body facts: the request is what it was.
    let mut bare = Map::new();
    bare.insert("transcript".to_owned(), json!("the day's digest"));
    attach(&mut bare, &BodyBlock::of(BodyState::None), Some(100_000));
    assert_eq!(crate::model_text_parts(&bare), ["the day's digest"]);
}

#[test]
fn location_freshness_takes_body_in_and_a_body_only_day_never_loops() {
    use crate::daily_location::{DailyLocationFreshness, daily_location_freshness};
    let journal = journal(false);
    let root = journal.path();
    let head = json!({"prepared":{"config":{}}});
    // Location doesn't apply here (no cutoff configured); body does.
    assert_eq!(
        daily_location_freshness(root, BODY_HOOK, DAY, Some(&head)).unwrap(),
        DailyLocationFreshness::Stale
    );
    let prepared = json!({"prepared":{"config":{FINGERPRINT_KEY: body_block(root, BODY_HOOK, DAY).fingerprint}}});
    assert_eq!(
        daily_location_freshness(root, BODY_HOOK, DAY, Some(&prepared)).unwrap(),
        DailyLocationFreshness::NotApplicable
    );
    // A body-only day: a packet prepared with no block stays as it is, every time.
    fs::remove_dir_all(root.join(format!("chronicle/{DAY}/phone"))).unwrap();
    for _ in 0..3 {
        assert_eq!(
            daily_location_freshness(root, BODY_HOOK, DAY, Some(&head)).unwrap(),
            DailyLocationFreshness::NotApplicable
        );
    }
    // Another talent's packets never see body at all.
    assert_eq!(
        daily_location_freshness(root, "entities:entity_suggest", DAY, Some(&prepared)).unwrap(),
        DailyLocationFreshness::NotApplicable
    );
}

#[test]
fn times_read_on_the_owners_clock_with_no_offset() {
    let journal = journal(false);
    let text = admitted(journal.path());
    let data = text.split("```json").nth(1).unwrap();
    assert!(
        data.contains("\"start\": \"07:00\"") && data.contains("\"end\": \"07:40\""),
        "{data}"
    );
    // No offset, in any form, reaches the model.
    let offset = |s: &str| {
        s.as_bytes()
            .windows(5)
            .any(|w| (w[0] == b'+' || w[0] == b'-') && w[1..].iter().all(u8::is_ascii_digit))
    };
    assert!(!offset(data), "{data}");
    // A night that ends on the next date says that date.
    assert_eq!(
        plain_time(
            Some("2026-10-10 22:10:00 -0600"),
            "America/Denver".parse().unwrap(),
            DAY
        ),
        json!("22:10")
    );
    assert_eq!(
        plain_time(
            Some("2026-10-11 06:20:00 -0600"),
            "America/Denver".parse().unwrap(),
            DAY
        ),
        json!("Oct 11 06:20")
    );
}

#[test]
fn a_day_whose_records_arrived_ninety_days_later_reads_no_body_facts() {
    let journal = journal(false);
    let root = journal.path();
    let path = root.join(format!("facets/body/activities/{DAY}.jsonl"));
    let text = fs::read_to_string(&path).unwrap().replace(
        &format!("\"arrived\":\"{DAY}\""),
        "\"arrived\":\"20270108\"",
    );
    fs::write(&path, text).unwrap();
    // Thought again for any reason, the block is the same: none.
    for _ in 0..2 {
        let block = body_block(root, BODY_HOOK, DAY);
        assert_eq!(block.state, BodyState::None);
        assert_eq!(block.fingerprint, NONE_FINGERPRINT);
    }
    // Arrived on the seventh day after: still admitted.
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("\"arrived\":\"20270108\"", "\"arrived\":\"20261017\"");
    fs::write(&path, text).unwrap();
    assert!(matches!(
        body_block(root, BODY_HOOK, DAY).state,
        BodyState::Admitted(_)
    ));
}

#[test]
fn only_experiences_reach_the_block_never_readings_alerts_or_logged_entries() {
    let journal = journal(false);
    let root = journal.path();
    let window = root.join(format!("chronicle/{DAY}/{STREAM}/070000_3600"));
    let mut rows = solstone_core_format::content::body_records::decode_rows(
        &fs::read_to_string(window.join("body.jsonl")).unwrap(),
    )
    .unwrap();
    let mut row = |kind: RowKind, id: &str, record_type: &str, label: &str, value: &str| {
        let mut row = BodyRow::new(
            kind,
            id.to_owned(),
            "apple_health",
            record_type,
            label.to_owned(),
        );
        row.at = Some("2026-10-10 07:30:00 -0600".to_owned());
        row.value = Some(value.to_owned());
        row.by = Some("Synthetic Phone".to_owned());
        rows.push(row);
    };
    row(
        RowKind::Reading,
        "m1",
        "HKQuantityTypeIdentifierBodyMass",
        "Body mass",
        "82.7 kg",
    );
    row(
        RowKind::Reading,
        "g1",
        "HKQuantityTypeIdentifierBloodGlucose",
        "Blood glucose",
        "252 mg/dL",
    );
    row(
        RowKind::Logged,
        "l1",
        "HKCategoryTypeIdentifierLowHeartRateEvent",
        "Low heart rate event",
        "",
    );
    row(
        RowKind::Logged,
        "i1",
        "HKCategoryTypeIdentifierIrregularHeartRhythmEvent",
        "Irregular heart rhythm event",
        "",
    );
    row(
        RowKind::Logged,
        "h1",
        "HKCategoryTypeIdentifierHeadache",
        "Headache",
        "moderate",
    );
    fs::write(window.join("body.jsonl"), encode_rows(&rows)).unwrap();
    let text = admitted(root);
    for absent in [
        "Body mass",
        "82.7",
        "glucose",
        "252",
        "Low heart",
        "Irregular",
        "Headache",
    ] {
        assert!(!text.contains(absent), "{absent} reached the block: {text}");
    }
    assert!(text.contains("Running"), "{text}");
}

#[test]
fn the_block_carries_distance_and_duration_only() {
    let journal = journal(false);
    let root = journal.path();
    let window = root.join(format!("chronicle/{DAY}/{STREAM}/070000_3600"));
    let write = |heart_max: &str| {
        let mut rows = solstone_core_format::content::body_records::decode_rows(
            &fs::read_to_string(window.join("body.jsonl")).unwrap(),
        )
        .unwrap();
        let workout = rows.iter_mut().find(|row| row.id == "w1").unwrap();
        workout
            .values
            .insert("duration".to_owned(), "40 min".to_owned());
        workout
            .values
            .insert("energy".to_owned(), "512 kcal".to_owned());
        workout
            .values
            .insert("heart rate average".to_owned(), "152 count/min".to_owned());
        workout
            .values
            .insert("heart rate maximum".to_owned(), heart_max.to_owned());
        fs::write(window.join("body.jsonl"), encode_rows(&rows)).unwrap();
    };
    write("181 count/min");
    let block = body_block(root, BODY_HOOK, DAY);
    let BodyState::Admitted(text) = &block.state else {
        panic!("{:?}", block.state);
    };
    let data = text.split("```json").nth(1).unwrap();
    assert!(data.contains("5.1 km") && data.contains("40 min"), "{data}");
    for absent in ["energy", "512", "heart", "152", "181"] {
        assert!(!data.contains(absent), "{absent} reached the block: {data}");
    }
    // A statistic that changes doesn't move the fingerprint: it isn't in the block.
    write("190 count/min");
    assert_eq!(
        body_block(root, BODY_HOOK, DAY).fingerprint,
        block.fingerprint
    );
}
