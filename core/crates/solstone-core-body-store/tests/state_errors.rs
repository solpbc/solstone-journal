// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_store::{BodyDedupeErrorField, BodyDedupeRow, BodyDedupeState};

mod support;

use support::{observation, set_text};

const BUNDLE: &str = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y";

fn digest(hex: char) -> String {
    format!("sha256:{}", hex.to_string().repeat(64))
}

fn invalid_observation(field: &str, key: &str) -> support::Observation {
    observation(
        "apple_retain_complete_one_row",
        BUNDLE,
        key,
        &digest('a'),
        |row, _| match field {
            "source_record_id" => set_text(row, field, [0xd800]),
            "record_type" | "start_date" | "end_date" => set_text(row, field, [0xd800]),
            "raw_ref" => set_text(
                row,
                field,
                format!("imports/{BUNDLE}/raw/x")
                    .bytes()
                    .map(u32::from)
                    .chain([0xd800]),
            ),
            _ => unreachable!("known field"),
        },
    )
}

#[test]
fn rejects_lone_surrogates_at_each_error_field() {
    for (field, expected) in [
        ("source_record_id", BodyDedupeErrorField::SourceRecordId),
        ("record_type", BodyDedupeErrorField::RecordType),
        ("start_date", BodyDedupeErrorField::StartTime),
        ("end_date", BodyDedupeErrorField::EndTime),
        ("raw_ref", BodyDedupeErrorField::RawRef),
    ] {
        let validated = invalid_observation(field, &digest('b')).validate();
        let mut state = BodyDedupeState::new();
        let error = state.apply(&validated).expect_err("invalid text refuses");
        assert_eq!(error.field(), expected);
        assert_eq!(error.bundle(), validated.event().bundle_id());
        assert_eq!(error.sequence(), validated.event().sequence());
        assert!(state.is_empty());
    }
}

#[test]
fn reports_the_first_invalid_field_by_precedence() {
    for (first, second, expected) in [
        (
            "source_record_id",
            "record_type",
            BodyDedupeErrorField::SourceRecordId,
        ),
        ("end_date", "raw_ref", BodyDedupeErrorField::EndTime),
    ] {
        let observation = observation(
            "apple_retain_complete_one_row",
            BUNDLE,
            &digest('c'),
            &digest('a'),
            |row, _| {
                set_text(row, first, [0xd800]);
                if second == "raw_ref" {
                    set_text(
                        row,
                        second,
                        format!("imports/{BUNDLE}/raw/x")
                            .bytes()
                            .map(u32::from)
                            .chain([0xd800]),
                    );
                } else {
                    set_text(row, second, [0xd800]);
                }
            },
        );
        let mut state = BodyDedupeState::new();
        assert_eq!(
            state.apply(&observation.validate()).unwrap_err().field(),
            expected
        );
    }
}

#[test]
fn scalar_and_control_text_round_trip_and_failures_do_not_mutate_state() {
    for code_point in [0xd7ff, 0xe000, 0x1f600, 0, 1, 0x80, 0x1c] {
        let expected = format!(
            "before{}after",
            char::from_u32(code_point).expect("scalar code point")
        );
        let observation = observation(
            "apple_retain_complete_one_row",
            BUNDLE,
            &digest('d'),
            &digest('a'),
            |row, _| set_text(row, "record_type", expected.chars().map(u32::from)),
        );
        let mut state = BodyDedupeState::new();
        state
            .apply(&observation.validate())
            .expect("scalar text applies");
        assert_eq!(
            state.iter().next().expect("row exists").record_type(),
            expected
        );
    }

    let long = "x".repeat(1_000_000);
    let valid = observation(
        "apple_retain_complete_one_row",
        BUNDLE,
        &digest('e'),
        &digest('a'),
        |row, _| set_text(row, "record_type", long.chars().map(u32::from)),
    );
    let mut state = BodyDedupeState::new();
    state
        .apply(&valid.validate())
        .expect("megabyte-scale text applies");
    assert_eq!(state.iter().next().unwrap().record_type(), long);

    let baseline = observation(
        "apple_retain_complete_one_row",
        BUNDLE,
        &digest('f'),
        &digest('a'),
        |_, _| {},
    );
    state.apply(&baseline.validate()).expect("baseline applies");
    let before: Vec<BodyDedupeRow> = state.iter().cloned().collect();
    let invalid = observation(
        "apple_retain_complete_one_row",
        BUNDLE,
        &digest('a'),
        &digest('a'),
        |row, _| {
            set_text(
                row,
                "record_type",
                std::iter::repeat_n(u32::from(b'x'), 1_000_000).chain([0xd800]),
            );
        },
    );
    assert_eq!(
        state.apply(&invalid.validate()).unwrap_err().field(),
        BodyDedupeErrorField::RecordType
    );
    assert_eq!(state.iter().cloned().collect::<Vec<_>>(), before);
}

#[test]
fn error_rendering_is_bounded_and_has_no_source() {
    let validated = invalid_observation("record_type", &digest('f')).validate();
    let mut state = BodyDedupeState::new();
    let error = state.apply(&validated).expect_err("invalid text refuses");
    let expected = format!("body-dedupe[{BUNDLE}]#E1 invalid_text: record_type");
    assert_eq!(error.to_string(), expected);
    assert_eq!(format!("{error:?}"), expected);
    assert!(expected.is_ascii());
    assert!(expected.len() <= 256);
    assert!(!expected.contains("Some(") && !expected.contains("None"));
    assert!(Error::source(&error).is_none());
}
