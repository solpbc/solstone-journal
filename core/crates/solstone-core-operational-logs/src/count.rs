// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible parsing for the `health logs -c` count argument.

use solstone_core_system_health::decimal_digit_value;

const MAX_PYTHON_INT_DIGITS: usize = 4300;

const PYTHON_INT_WHITESPACE_RANGES: &[(u32, u32)] = &[
    (0x0009, 0x000d),
    (0x0020, 0x0020),
    (0x0085, 0x0085),
    (0x00a0, 0x00a0),
    (0x1680, 0x1680),
    (0x2000, 0x200a),
    (0x2028, 0x2029),
    (0x202f, 0x202f),
    (0x205f, 0x205f),
    (0x3000, 0x3000),
];

/// A parsed count whose magnitude may exceed machine range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedCount {
    Value(i64),
    SaturatedPositive,
    SaturatedNegative,
}

/// A Python `int(str)` compatibility failure for a count argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CountParseError {
    Empty,
    Invalid,
    TooManyDigits { digit_count: usize },
}

/// Expose the pinned Python `str.isspace()` subset for fixture verification.
#[doc(hidden)]
#[must_use]
pub fn python_int_whitespace_ranges() -> &'static [(u32, u32)] {
    PYTHON_INT_WHITESPACE_RANGES
}

/// Parse one count with CPython's Unicode integer text grammar.
pub fn parse_health_log_count(input: &str) -> Result<ParsedCount, CountParseError> {
    let input = input.trim_matches(python_int_whitespace);
    if input.is_empty() {
        return Err(CountParseError::Empty);
    }

    let (negative, body) = if let Some(body) = input.strip_prefix('-') {
        (true, body)
    } else if let Some(body) = input.strip_prefix('+') {
        (false, body)
    } else {
        (false, input)
    };
    if body.is_empty() {
        return Err(CountParseError::Invalid);
    }

    let mut digits = 0_usize;
    let mut previous_was_digit = false;
    let mut scalars = body.chars().peekable();
    while let Some(scalar) = scalars.next() {
        if scalar == '_' {
            if !previous_was_digit
                || scalars
                    .peek()
                    .is_none_or(|next| decimal_digit_value(*next).is_none())
            {
                return Err(CountParseError::Invalid);
            }
            previous_was_digit = false;
            continue;
        }
        if decimal_digit_value(scalar).is_none() {
            return Err(CountParseError::Invalid);
        }
        digits += 1;
        previous_was_digit = true;
    }
    if digits == 0 || !previous_was_digit {
        return Err(CountParseError::Invalid);
    }
    if digits > MAX_PYTHON_INT_DIGITS {
        return Err(CountParseError::TooManyDigits {
            digit_count: digits,
        });
    }

    let magnitude = body
        .chars()
        .filter_map(decimal_digit_value)
        .fold(0_u64, |value, digit| {
            value.saturating_mul(10).saturating_add(u64::from(digit))
        });
    if magnitude == 0 {
        return Ok(ParsedCount::Value(0));
    }
    if !negative {
        return match i64::try_from(magnitude) {
            Ok(value) => Ok(ParsedCount::Value(value)),
            Err(_) => Ok(ParsedCount::SaturatedPositive),
        };
    }
    let min_magnitude = i64::MIN.unsigned_abs();
    if magnitude == min_magnitude {
        Ok(ParsedCount::Value(i64::MIN))
    } else if let Ok(value) = i64::try_from(magnitude) {
        Ok(ParsedCount::Value(-value))
    } else {
        Ok(ParsedCount::SaturatedNegative)
    }
}

fn python_int_whitespace(scalar: char) -> bool {
    let scalar = scalar as u32;
    PYTHON_INT_WHITESPACE_RANGES
        .binary_search_by(|(start, end)| {
            if scalar < *start {
                std::cmp::Ordering::Greater
            } else if scalar > *end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use solstone_core_system_health::decimal_digit_value;

    use super::*;

    const FIXTURE_JSON: &str = include_str!("../../../fixtures/health_text_reference.json");
    const FIXTURE_SHA256: &str = "b0c3ac7312aea7e017c5807c2f531b7463b8a416f78ca3a1d7c63cd6536f664d";

    static FIXTURE: OnceLock<HealthTextFixture> = OnceLock::new();

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct HealthTextFixture {
        schema: u64,
        runtime: Runtime,
        decimal_cases: Vec<(u32, u8, ValueResult, ValueResult)>,
        whitespace_cases: Vec<(u32, ValueResult)>,
        port_cases: serde_json::Value,
        provenance: serde_json::Value,
        scalar_cases: serde_json::Value,
        unsafe_unicode: serde_json::Value,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Runtime {
        int_max_str_digits: usize,
        executable_sha256: String,
        python: String,
        unicode: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ValueResult {
        kind: String,
        value: Option<String>,
    }

    fn fixture() -> &'static HealthTextFixture {
        FIXTURE.get_or_init(|| {
            assert_eq!(
                format!("{:x}", Sha256::digest(FIXTURE_JSON.as_bytes())),
                FIXTURE_SHA256,
                "health text fixture digest"
            );
            let fixture: HealthTextFixture =
                serde_json::from_str(FIXTURE_JSON).expect("health text fixture must be valid");
            assert_eq!(fixture.schema, 2);
            assert_eq!(fixture.runtime.int_max_str_digits, MAX_PYTHON_INT_DIGITS);
            assert_eq!(fixture.decimal_cases.len(), 760);
            assert_eq!(fixture.whitespace_cases.len(), 29);
            fixture
        })
    }

    fn value(input: &str) -> ParsedCount {
        parse_health_log_count(input).unwrap()
    }

    #[test]
    fn fixture_decimal_cases_match_the_shared_digit_table_and_parser() {
        for (scalar, digit, single, mixed) in &fixture().decimal_cases {
            let scalar = char::from_u32(*scalar).unwrap();
            assert_eq!(decimal_digit_value(scalar), Some(*digit));
            assert_eq!(single.kind, "value");
            assert_eq!(mixed.kind, "value");
            assert_eq!(
                value(&scalar.to_string()),
                ParsedCount::Value(single.value.as_deref().unwrap().parse().unwrap())
            );
            assert_eq!(
                value(&format!("1{scalar}2")),
                ParsedCount::Value(mixed.value.as_deref().unwrap().parse().unwrap())
            );
        }
    }

    #[test]
    fn fixture_whitespace_cases_match_the_dedicated_python_table() {
        let from_ranges = python_int_whitespace_ranges()
            .iter()
            .flat_map(|(start, end)| *start..=*end)
            .collect::<Vec<_>>();
        let from_fixture = fixture()
            .whitespace_cases
            .iter()
            .filter(|(_, result)| result.kind == "value")
            .map(|(scalar, _)| *scalar)
            .collect::<Vec<_>>();
        assert_eq!(from_ranges, from_fixture);
        for (scalar, expected) in &fixture().whitespace_cases {
            let scalar = char::from_u32(*scalar).unwrap();
            let input = format!("{scalar}12{scalar}");
            if expected.kind == "value" {
                assert_eq!(expected.value.as_deref(), Some("12"));
                assert_eq!(value(&input), ParsedCount::Value(12));
            } else {
                assert_eq!(expected.kind, "ValueError");
                assert_eq!(
                    parse_health_log_count(&input),
                    Err(CountParseError::Invalid)
                );
            }
        }
    }

    #[test]
    fn parses_signs_mixed_scripts_and_pep_515_underscores() {
        assert_eq!(value("+5"), ParsedCount::Value(5));
        assert_eq!(value("-5"), ParsedCount::Value(-5));
        assert_eq!(value("-0"), ParsedCount::Value(0));
        assert_eq!(value("+0"), ParsedCount::Value(0));
        assert_eq!(value("1٢"), ParsedCount::Value(12));
        assert_eq!(value("1_000"), ParsedCount::Value(1000));
        for input in ["_1000", "1000_", "1__000", "-_1000", "1_", "_"] {
            assert_eq!(parse_health_log_count(input), Err(CountParseError::Invalid));
        }
    }

    #[test]
    fn rejects_invalid_forms_before_applying_the_digit_limit() {
        for input in [
            "",
            " \u{001c}\u{3000}",
            "+",
            "-",
            "1 2",
            "+-1",
            "-+1",
            "nope",
        ] {
            assert!(matches!(
                parse_health_log_count(input),
                Err(CountParseError::Empty | CountParseError::Invalid)
            ));
        }
        for input in ["a".repeat(4301), format!("{}_", "1".repeat(4301))] {
            assert_eq!(
                parse_health_log_count(&input),
                Err(CountParseError::Invalid)
            );
        }
    }

    #[test]
    fn digit_limit_and_saturation_follow_cpython_boundaries() {
        let nines = "9".repeat(MAX_PYTHON_INT_DIGITS);
        assert_eq!(value(&nines), ParsedCount::SaturatedPositive);
        assert_eq!(value(&format!("-{nines}")), ParsedCount::SaturatedNegative);
        assert_eq!(
            value(&format!("{}12345", "0".repeat(MAX_PYTHON_INT_DIGITS - 5))),
            ParsedCount::Value(12345)
        );
        for input in [
            "0".repeat(MAX_PYTHON_INT_DIGITS) + "1",
            "1".repeat(MAX_PYTHON_INT_DIGITS + 1),
        ] {
            assert_eq!(
                parse_health_log_count(&input),
                Err(CountParseError::TooManyDigits {
                    digit_count: MAX_PYTHON_INT_DIGITS + 1
                })
            );
        }
    }

    #[test]
    fn i64_boundaries_are_exact() {
        assert_eq!(value(&i64::MAX.to_string()), ParsedCount::Value(i64::MAX));
        assert_eq!(value("9223372036854775808"), ParsedCount::SaturatedPositive);
        assert_eq!(value(&i64::MIN.to_string()), ParsedCount::Value(i64::MIN));
        assert_eq!(
            value("-9223372036854775807"),
            ParsedCount::Value(-9_223_372_036_854_775_807)
        );
        assert_eq!(
            value("-9223372036854775809"),
            ParsedCount::SaturatedNegative
        );
    }
}
