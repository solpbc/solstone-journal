// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "../src/operational_log_parse/fixture.rs"]
mod fixture;

use std::collections::HashMap;

use chrono::{NaiveDateTime, TimeDelta};
use solstone_core_system::operational_log_parse::{
    HealthLogSinceError, ParsedHealthLogRow, parse_health_log_row, parse_health_log_since,
};

fn datetime(value: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").expect("fixture datetime")
}

#[test]
fn parses_all_frozen_rows() {
    for (index, case) in fixture::fixture().rows.iter().enumerate() {
        let actual = parse_health_log_row(&case.input);
        let expected = case.outcome.as_ref().map(|outcome| ParsedHealthLogRow {
            timestamp: datetime(&outcome.timestamp),
            service: outcome.service.clone(),
            stream: outcome.stream.clone(),
            message: outcome.message.clone(),
            raw: outcome.raw.clone(),
        });
        assert_eq!(actual, expected, "row fixture case {index}");
    }
}

#[test]
fn parses_all_frozen_since_cases() {
    let fixture = fixture::fixture();
    let now = datetime(&fixture.runtime.fixed_now);
    for (index, case) in fixture.since.iter().enumerate() {
        match case {
            fixture::SinceCase::Outcome(case) => {
                assert_eq!(
                    parse_health_log_since(&case.input, now),
                    Ok(datetime(&case.outcome)),
                    "since fixture case {index}"
                );
            }
            fixture::SinceCase::Error(case) => {
                let actual = parse_health_log_since(&case.input, now).expect_err("error fixture");
                match (case.error_type.as_str(), &actual) {
                    (
                        "ArgumentTypeError",
                        HealthLogSinceError::InvalidTime {
                            input: actual_input,
                        },
                    ) => {
                        assert_eq!(actual_input, &case.input, "since fixture case {index}");
                    }
                    (
                        "OverflowError",
                        HealthLogSinceError::RelativeOverflow {
                            input: actual_input,
                        },
                    ) => {
                        assert_eq!(actual_input, &case.input, "since fixture case {index}");
                    }
                    _ => panic!("since fixture case {index} returned wrong error: {actual:?}"),
                }
                assert_eq!(actual.to_string(), case.error, "since fixture case {index}");
            }
        }
    }
}

#[test]
fn fixture_schema_rejects_unknown_fields_at_every_shape() {
    let original = serde_json::from_str::<serde_json::Value>(include_str!(
        "../../../fixtures/health_logs_reference.json"
    ))
    .expect("fixture JSON");
    let mutations = [
        vec![],
        vec!["source"],
        vec!["runtime"],
        vec!["rows", "0"],
        vec!["rows", "0", "outcome"],
        vec!["since", "0"],
        vec!["since", "18"],
        vec!["regex", "0"],
        vec!["regex", "35"],
        vec!["unicode_contract"],
    ];

    for path in mutations {
        let mut value = original.clone();
        let mut cursor = &mut value;
        for component in &path {
            cursor = if let Ok(index) = component.parse::<usize>() {
                &mut cursor.as_array_mut().expect("array path")[index]
            } else {
                cursor
                    .as_object_mut()
                    .expect("object path")
                    .get_mut(*component)
                    .expect("fixture path")
            };
        }
        cursor
            .as_object_mut()
            .expect("mutation target object")
            .insert("unknown_field".to_owned(), serde_json::json!(true));
        let encoded = serde_json::to_string(&value).expect("mutated fixture JSON");
        assert!(
            fixture::parse_json(&encoded).is_err(),
            "unknown field accepted at {path:?}"
        );
    }
}

#[test]
fn unicode_contract_is_exhaustive() {
    let contract = &fixture::fixture().unicode_contract;
    assert_eq!(contract.whitespace_codepoints.len(), 29);
    assert_eq!(contract.decimal_codepoints.len(), 760);
    assert_eq!(contract.decimal_zero_codepoints.len(), 76);

    let now = datetime("2026-08-12T17:45:30");
    for &scalar in &contract.whitespace_codepoints {
        let scalar = char::from_u32(scalar).expect("fixture whitespace scalar");
        assert_eq!(
            parse_health_log_since(&format!("{scalar}0m{scalar}"), now),
            Ok(now),
            "whitespace U+{:04X}",
            scalar as u32
        );
    }

    let digits = contract
        .decimal_codepoints
        .iter()
        .copied()
        .collect::<HashMap<_, _>>();
    for &(scalar, value) in &contract.decimal_codepoints {
        let scalar = char::from_u32(scalar).expect("fixture decimal scalar");
        assert_eq!(
            parse_health_log_since(&format!("{scalar}m"), now),
            Ok(now - TimeDelta::try_minutes(i64::from(value)).expect("minute delta")),
            "decimal U+{:04X}",
            scalar as u32
        );
    }
    for &zero in &contract.decimal_zero_codepoints {
        for value in 0..10_u32 {
            assert_eq!(
                digits.get(&(zero + value)),
                Some(&(value as u8)),
                "zero U+{zero:04X}"
            );
        }
    }
}

#[test]
fn rejects_invalid_iso_weeks_and_accepts_valid_boundary() {
    for input in [
        "2021-W53-1T00:00:00 [echo:stdout] bad week 53",
        "2021-W00-1T00:00:00 [echo:stdout] bad week 00",
    ] {
        assert_eq!(parse_health_log_row(input), None, "{input}");
    }
    let parsed =
        parse_health_log_row("2020-W53-1T00:00:00 [echo:stdout] valid").expect("valid week 53");
    assert_eq!(parsed.timestamp, datetime("2020-12-28T00:00:00"));
}

#[test]
fn accepts_only_midnight_for_hour_24() {
    let accepted = parse_health_log_row("2026-02-09T24:00:00 [echo:stdout] accepted")
        .expect("24:00:00 is valid");
    assert_eq!(accepted.timestamp, datetime("2026-02-10T00:00:00"));
    assert_eq!(
        parse_health_log_row("2026-02-09T24:00:01 [echo:stdout] rejected"),
        None
    );
}

#[test]
fn rejects_overwide_12_hour_fields() {
    let now = datetime(&fixture::fixture().runtime.fixed_now);
    for input in ["001PM", "001pm", "4:000pm"] {
        assert_eq!(
            parse_health_log_since(input, now),
            Err(HealthLogSinceError::InvalidTime {
                input: input.to_owned(),
            }),
            "{input}"
        );
    }
}

#[test]
fn accepts_one_or_two_digit_12_hour_fields() {
    let now = datetime(&fixture::fixture().runtime.fixed_now);
    for (input, expected) in [
        ("4pm", "2026-08-12T16:00:00"),
        ("04pm", "2026-08-12T16:00:00"),
        ("4:30pm", "2026-08-12T16:30:00"),
    ] {
        assert_eq!(
            parse_health_log_since(input, now),
            Ok(datetime(expected)),
            "{input}"
        );
    }
}

#[test]
fn counts_timestamp_indices_as_unicode_scalars() {
    let parsed = parse_health_log_row("2026-02-09🐍10:00:00 [echo:stdout] scalar")
        .expect("astral separator consumes one scalar");
    assert_eq!(parsed.timestamp, datetime("2026-02-09T10:00:00"));
    assert_eq!(parsed.message, "scalar");
}
