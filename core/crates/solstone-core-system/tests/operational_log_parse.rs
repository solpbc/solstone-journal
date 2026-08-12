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
fn fixture_raw_sha256_is_pinned() {
    assert_eq!(fixture::raw_sha256(), fixture::FIXTURE_SHA256);
    let fixture = fixture::fixture();
    assert_eq!(fixture.source.path, "solstone/think/logs_cli.py");
    assert_eq!(
        fixture.source.sha256,
        "f2ce46d928dc7c1a2922b8060e95c26b610cfe4eae250370571dc532ceed7a7f"
    );
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
            fixture::SinceCase::Outcome { input, outcome } => {
                assert_eq!(
                    parse_health_log_since(input, now),
                    Ok(datetime(outcome)),
                    "since fixture case {index}"
                );
            }
            fixture::SinceCase::Error {
                input,
                error,
                error_type,
            } => {
                let actual = parse_health_log_since(input, now).expect_err("error fixture");
                match (error_type.as_str(), &actual) {
                    (
                        "ArgumentTypeError",
                        HealthLogSinceError::InvalidTime {
                            input: actual_input,
                        },
                    ) => {
                        assert_eq!(actual_input, input, "since fixture case {index}");
                    }
                    (
                        "OverflowError",
                        HealthLogSinceError::RelativeOverflow {
                            input: actual_input,
                        },
                    ) => {
                        assert_eq!(actual_input, input, "since fixture case {index}");
                    }
                    _ => panic!("since fixture case {index} returned wrong error: {actual:?}"),
                }
                assert_eq!(actual.to_string(), *error, "since fixture case {index}");
            }
        }
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
fn counts_timestamp_indices_as_unicode_scalars() {
    let parsed = parse_health_log_row("2026-02-09🐍10:00:00 [echo:stdout] scalar")
        .expect("astral separator consumes one scalar");
    assert_eq!(parsed.timestamp, datetime("2026-02-09T10:00:00"));
    assert_eq!(parsed.message, "scalar");
}
