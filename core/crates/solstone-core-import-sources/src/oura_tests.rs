// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::json;

use super::*;
use crate::apple_health::MAX_ROW_BYTES;

fn fixture_page(endpoint: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/importers/health/oura_pages")
        .join(endpoint)
        .join("0-0.json");
    let page: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    page["data"].as_array().unwrap().clone()
}

fn row(endpoint: &str, document: &Value) -> ReadRow {
    match read_document(endpoint, document) {
        Document::Row(read) => *read,
        other => panic!("{endpoint}: {other:?}"),
    }
}

/// Every key of the document, as Oura gave it.
fn whole(document: &Value) -> BTreeMap<String, Value> {
    document
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[test]
fn a_sleep_reads_as_a_night_from_bedtime_start_with_every_field_kept() {
    let pages = fixture_page("sleep");
    let night = row("sleep", &pages[0]);
    let BodyRow {
        kind,
        experience,
        start,
        end,
        by,
        label,
        record_type,
        fields,
        values,
        words,
        value,
        ..
    } = night.row.clone();
    assert_eq!(kind, RowKind::Experience);
    assert_eq!(experience.as_deref(), Some("sleep"));
    assert_eq!(record_type, "long_sleep");
    assert_eq!(label, "long sleep");
    assert_eq!(start.as_deref(), Some("2026-01-01T22:41:00.000-07:00"));
    assert_eq!(end.as_deref(), Some("2026-01-02T06:32:00.000-07:00"));
    assert_eq!(by.as_deref(), Some("Oura"));
    // Nulls, samples, the embedded readiness and Oura's own `day` stay as given.
    assert_eq!(fields, whole(&pages[0]));
    assert_eq!(fields["sleep_analysis_reason"], Value::Null);
    assert!(values.is_empty() && words.is_empty() && value.is_none());
    assert_eq!(night.oura_day, NaiveDate::from_ymd_opt(2026, 1, 2));
    assert_eq!(
        night.identity,
        "oura\u{1f}sleep\u{1f}synthetic-sleep-2026-01-02"
    );

    // Only the required keys.
    let nap = row("sleep", &pages[1]);
    assert_eq!(nap.row.label, "late nap");
    assert_eq!(nap.row.fields, whole(&pages[1]));

    // A rejected detection and a deleted sleep aren't nights.
    for (document, kind) in [(&pages[2], "rest"), (&pages[3], "deleted")] {
        match read_document("sleep", document) {
            Document::NotANight {
                id, kind: found, ..
            } => {
                assert_eq!(found, kind);
                assert_eq!(
                    id,
                    row_id(&identity("sleep", document["id"].as_str().unwrap()))
                );
            }
            other => panic!("{other:?}"),
        }
    }
    let mut untyped = pages[1].clone();
    untyped["type"] = Value::Null;
    assert!(matches!(
        read_document("sleep", &untyped),
        Document::NotRead(name) if name == "sleep with no type"
    ));
}

#[test]
fn a_workout_restates_distance_in_metres_and_energy_and_the_owners_label_only() {
    let pages = fixture_page("workout");
    let walk = row("workout", &pages[0]).row;
    assert_eq!(walk.experience.as_deref(), Some("workout"));
    assert_eq!(walk.record_type, "walking");
    assert_eq!(walk.label, "walking");
    assert_eq!(
        walk.values,
        BTreeMap::from([
            ("distance".to_owned(), "2412.9 m".to_owned()),
            ("energy".to_owned(), "148.5 kcal".to_owned()),
        ])
    );
    // A null label is no words.
    assert!(walk.words.is_empty());
    assert_eq!(walk.fields, whole(&pages[0]));

    // No distance key, null calories, and the sandbox's literal "None".
    let strength = row("workout", &pages[1]).row;
    assert_eq!(strength.record_type, "strength_training");
    assert_eq!(strength.label, "strength training");
    assert!(strength.values.is_empty(), "{:?}", strength.values);
    assert!(strength.words.is_empty(), "{:?}", strength.words);
    assert_eq!(strength.fields["label"], json!("None"));

    let run = row("workout", &pages[2]).row;
    assert_eq!(run.words, ["synthetic hill loop"]);
    assert_eq!(run.values["distance"], "8020.0 m");
}

/// What the reader keeps as the owner's words is exactly what the owner wrote: a
/// workout's label and a tag's name and comment. The sandbox's literal "None"
/// label is no words, and neither is anything Oura assesses.
#[test]
fn the_owners_words_are_a_workouts_label_and_a_tags_name_and_comment_and_never_none() {
    use solstone_core_format::content::body_records::owner_words;
    let workouts = fixture_page("workout");
    assert_eq!(workouts[1]["label"], json!("None"), "fixture");
    assert!(owner_words("oura", &row("workout", &workouts[1]).row).is_empty());
    assert_eq!(
        owner_words("oura", &row("workout", &workouts[2]).row),
        ["synthetic hill loop"]
    );
    let mut tag = fixture_page("enhanced_tag")[1].clone();
    tag["comment"] = json!("after the long run");
    assert_eq!(
        owner_words("oura", &row("enhanced_tag", &tag).row),
        ["synthetic sauna", "after the long run"]
    );
    let mut session = fixture_page("session")[0].clone();
    session["mood"] = json!("bad");
    assert!(owner_words("oura", &row("session", &session).row).is_empty());
}

#[test]
fn a_session_is_mindfulness_on_the_grid_and_a_nap_is_sleep() {
    let pages = fixture_page("session");
    let meditation = row("session", &pages[0]).row;
    assert_eq!(meditation.experience.as_deref(), Some("mindfulness"));
    assert_eq!(meditation.label, "meditation session");
    assert_eq!(meditation.fields, whole(&pages[0]));
    // Mood is the owner's pick in Oura's words, so it stays in `fields`.
    assert!(meditation.values.is_empty() && meditation.words.is_empty());

    // Every sample null.
    let breathing = row("session", &pages[1]).row;
    assert_eq!(breathing.label, "breathing session");
    assert_eq!(breathing.fields["heart_rate"], Value::Null);
    assert_eq!(breathing.fields, whole(&pages[1]));

    let nap = row("session", &pages[2]).row;
    assert_eq!(nap.experience.as_deref(), Some("sleep"));
    assert_eq!(nap.record_type, "nap");
    assert_eq!(nap.label, "nap");

    assert_eq!(row("session", &pages[3]).row.label, "body status session");
}

#[test]
fn a_tag_is_logged_by_its_start_alone_or_with_its_end_and_keeps_the_owners_words() {
    let pages = fixture_page("enhanced_tag");
    let coffee = row("enhanced_tag", &pages[0]);
    assert_eq!(coffee.row.kind, RowKind::Logged);
    assert_eq!(coffee.row.label, "caffeine");
    assert_eq!(coffee.row.record_type, "tag_generic_caffeine");
    assert_eq!(
        coffee.row.start.as_deref(),
        Some("2026-01-02T09:15:00-07:00")
    );
    // A null end means no duration.
    assert!(coffee.row.end.is_none());
    assert_eq!(coffee.row.words, ["second cup"]);
    assert_eq!(coffee.oura_day, NaiveDate::from_ymd_opt(2026, 1, 2));
    assert_eq!(coffee.row.fields, whole(&pages[0]));

    let sauna = row("enhanced_tag", &pages[1]).row;
    assert_eq!(sauna.label, "custom");
    assert_eq!(sauna.end.as_deref(), Some("2026-01-02T16:30:00-07:00"));
    assert_eq!(sauna.words, ["synthetic sauna"]);

    let text = row("enhanced_tag", &pages[2]).row;
    assert_eq!(text.label, "tag");
    assert_eq!(text.record_type, "tag");
    assert_eq!(text.words, ["early night"]);

    assert_eq!(tag_label(Some("generic_excited")), "excited");
    assert_eq!(tag_label(Some("tag_generic_late_meal")), "late meal");
    assert_eq!(tag_label(Some("tag_")), "tag");
}

#[test]
fn a_heartrate_row_is_a_reading_whose_identity_is_its_instant_in_either_shape() {
    let pages = fixture_page("heartrate");
    // The sandbox's shape: no `timestamp_unix`, a null `producer_timestamp`.
    let sandbox = row("heartrate", &pages[0]);
    assert_eq!(sandbox.row.kind, RowKind::Reading);
    assert_eq!(sandbox.row.label, "heart rate");
    assert_eq!(sandbox.row.at.as_deref(), Some("2026-01-02T15:20:00.000Z"));
    assert_eq!(sandbox.row.value.as_deref(), Some("112"));
    assert_eq!(sandbox.row.unit.as_deref(), Some("count/min"));
    assert_eq!(sandbox.row.fields, whole(&pages[0]));
    // The documented shape, with `timestamp_unix`.
    let documented = row("heartrate", &pages[2]);
    assert_eq!(documented.row.fields, whole(&pages[2]));

    // Oura's `source` is content: the same instant reclassified is the same identity.
    let mut workout = pages[1].clone();
    workout["source"] = json!("workout");
    let rest = row("heartrate", &pages[1]);
    let moved = row("heartrate", &workout);
    assert_eq!(rest.identity, moved.identity);
    assert_ne!(rest.row.fields, moved.row.fields);
    // An instant written at another offset is the same instant.
    let mut offset = pages[1].clone();
    offset["timestamp"] = json!("2026-01-02T16:00:00.000-07:00");
    assert_eq!(row("heartrate", &offset).identity, rest.identity);
}

#[test]
fn a_daily_document_is_a_dated_row_with_no_value_and_every_field() {
    for (endpoint, label) in [
        ("daily_readiness", "daily readiness"),
        ("daily_sleep", "daily sleep"),
        ("daily_activity", "daily activity"),
        ("daily_stress", "daily stress"),
        ("daily_resilience", "daily resilience"),
        ("daily_spo2", "daily SpO2"),
        ("daily_cardiovascular_age", "cardiovascular age"),
        ("vO2_max", "VO2 max"),
    ] {
        let document = &fixture_page(endpoint)[0];
        let dated = row(endpoint, document).row;
        assert_eq!(dated.kind, RowKind::Dated, "{endpoint}");
        assert_eq!(dated.label, label);
        assert_eq!(dated.record_type, endpoint);
        assert_eq!(dated.date.as_deref(), Some("2026-01-02"));
        assert!(dated.start.is_none() && dated.at.is_none());
        // Scores and assessment words stay in `fields` only.
        assert!(dated.value.is_none(), "{endpoint}");
        assert!(dated.values.is_empty() && dated.words.is_empty());
        assert_eq!(dated.fields, whole(document), "{endpoint}");
    }
    let stress = row("daily_stress", &fixture_page("daily_stress")[0]).row;
    assert_eq!(stress.fields["day_summary"], json!("stressful"));
}

#[test]
fn an_endpoint_outside_the_table_is_not_read() {
    let document = &fixture_page("blood_glucose")[0];
    assert!(matches!(
        read_document("blood_glucose", document),
        Document::NotRead(name) if name == "blood_glucose"
    ));
    assert!(read_endpoint("blood_glucose").is_none());
    assert!(
        read_endpoint("vo2_max").is_none(),
        "Oura's casing is vO2_max"
    );
    for endpoint in ENDPOINTS {
        assert!(read_endpoint(endpoint).is_some(), "{endpoint}");
    }
}

#[test]
fn a_document_that_cant_be_one_is_refused_by_its_endpoint_and_stage() {
    let refused = |endpoint: &str, document: Value| match read_document(endpoint, &document) {
        Document::Refused(stage) => stage,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused("workout", json!([1])), "workout: not_a_document");
    assert_eq!(
        refused("workout", json!({"activity": "running"})),
        "workout: id"
    );
    assert_eq!(
        refused(
            "workout",
            json!({"id": "w", "activity": "running", "start_datetime": "later"})
        ),
        "workout: time"
    );
    assert_eq!(
        refused(
            "sleep",
            json!({"id": "s", "type": "sleep", "bedtime_start": "2026-01-02T06:00:00Z", "bedtime_end": "2026-01-02T05:00:00Z"})
        ),
        "sleep: end_before_start"
    );
    assert_eq!(
        refused(
            "session",
            json!({"id": "m", "type": "meditation", "start_datetime": "2026-01-01T00:00:00Z", "end_datetime": "2026-02-01T00:00:00Z"})
        ),
        "session: span_too_long"
    );
    assert_eq!(
        refused("daily_sleep", json!({"id": "d", "day": "yesterday"})),
        "daily_sleep: day"
    );
    assert_eq!(
        refused("heartrate", json!({"timestamp": "2026-01-02T00:00:00Z"})),
        "heartrate: bpm"
    );
}

/// A whole day of one-minute MET and five-minute classes.
fn full_daily_activity() -> Value {
    let mut document = fixture_page("daily_activity")[0].clone();
    let met: Vec<Value> = (0..1_440)
        .map(|minute| json!(f64::from(minute % 70) / 10.0 + 0.9))
        .collect();
    document["met"] =
        json!({"interval": 60.0, "items": met, "timestamp": "2026-01-02T04:00:00.000-07:00"});
    document["class_5_min"] = json!("1234".repeat(72));
    document
}

/// Fourteen hours of sleep with every 30-second string and both samples.
fn long_night() -> Value {
    let mut document = fixture_page("sleep")[0].clone();
    let halves = 14 * 120;
    let fives = 14 * 12;
    document["bedtime_start"] = json!("2026-01-01T18:00:00.000-07:00");
    document["bedtime_end"] = json!("2026-01-02T08:00:00.000-07:00");
    document["movement_30_sec"] = json!("1234".repeat(halves / 4));
    document["sleep_phase_30_sec"] = json!("4213".repeat(halves / 4));
    document["sleep_phase_5_min"] = json!("1234".repeat(fives / 4));
    document["app_sleep_phase_5_min"] = json!("4321".repeat(fives / 4));
    let items = |base: f64| -> Vec<Value> {
        (0..fives)
            .map(|index| json!(base + (index % 17) as f64))
            .collect()
    };
    document["heart_rate"] = json!({"interval": 300.0, "items": items(48.0), "timestamp": "2026-01-01T18:00:00.000-07:00"});
    document["hrv"] = json!({"interval": 300.0, "items": items(40.0), "timestamp": "2026-01-01T18:00:00.000-07:00"});
    document
}

#[test]
fn the_largest_documented_shapes_are_kept_whole_within_the_row_bound() {
    for (endpoint, document) in [
        ("daily_activity", full_daily_activity()),
        ("sleep", long_night()),
    ] {
        let read = row(endpoint, &document);
        let content = serde_json::to_string(&read.row).unwrap();
        assert!(
            content.len() <= MAX_ROW_BYTES,
            "{endpoint}: {} bytes",
            content.len()
        );
        assert_eq!(read.row.fields, whole(&document), "{endpoint}");
    }
    assert_eq!(
        full_daily_activity()["met"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1_440
    );
}

#[test]
fn a_rows_holes_name_its_endpoint_and_oura_day_or_a_readings_hour() {
    let sleep = row("sleep", &fixture_page("sleep")[0]).row;
    assert_eq!(hole_points(&sleep), [("sleep", "2026-01-02".to_owned())]);
    let tag = row("enhanced_tag", &fixture_page("enhanced_tag")[0]).row;
    assert_eq!(
        hole_points(&tag),
        [("enhanced_tag", "2026-01-02".to_owned())]
    );
    let nap = row("session", &fixture_page("session")[2]).row;
    assert_eq!(hole_points(&nap), [("session", "2026-01-03".to_owned())]);
    let dated = row("vO2_max", &fixture_page("vO2_max")[0]).row;
    assert_eq!(hole_points(&dated), [("vO2_max", "2026-01-02".to_owned())]);
    let reading = row("heartrate", &fixture_page("heartrate")[0]).row;
    assert_eq!(
        hole_points(&reading),
        [("heartrate", "2026-01-02T15:00:00Z".to_owned())]
    );
    // A tile names every date its span was written across.
    let mut tile = sleep.clone();
    tile.kind = RowKind::ExperienceTile;
    tile.fields.clear();
    assert_eq!(
        hole_points(&tile),
        [
            ("sleep", "2026-01-01".to_owned()),
            ("sleep", "2026-01-02".to_owned())
        ]
    );
    // Another source's rows are never Oura's holes.
    let mut apple = reading.clone();
    apple.source = "apple_health".to_owned();
    assert!(hole_points(&apple).is_empty());
}
