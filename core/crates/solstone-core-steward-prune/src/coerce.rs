// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::syntax::{Extended, Value, decode_string};
use crate::unicode::{is_python_whitespace, nd_digit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Coercion {
    Aged,
    Kept { compatibility: bool },
    NumericOverflow,
}

pub(crate) fn coerce(value: Value<'_>, cutoff: i64) -> Coercion {
    match value {
        Value::Null | Value::Container => Coercion::Kept {
            compatibility: false,
        },
        Value::Bool(value) => compare_i64(i64::from(value), cutoff),
        Value::Extended(Extended::Nan) => Coercion::Kept {
            compatibility: true,
        },
        Value::Extended(Extended::PositiveInfinity | Extended::NegativeInfinity) => {
            Coercion::NumericOverflow
        }
        Value::Number(raw) => coerce_number(raw, cutoff),
        Value::String(raw) => coerce_string(raw, cutoff),
    }
}

fn compare_i64(value: i64, cutoff: i64) -> Coercion {
    if value < cutoff {
        Coercion::Aged
    } else {
        Coercion::Kept {
            compatibility: false,
        }
    }
}

fn coerce_number(raw: &[u8], cutoff: i64) -> Coercion {
    if !raw.contains(&b'.') && !raw.contains(&b'e') && !raw.contains(&b'E') {
        return compare_ascii_integer(raw, cutoff);
    }
    let text = core::str::from_utf8(raw).expect("JSON number is ASCII");
    let value = match text.parse::<f64>() {
        Ok(value) if value.is_finite() => value,
        _ => return Coercion::NumericOverflow,
    };
    let truncated = value.trunc();
    if !truncated.is_finite() {
        return Coercion::NumericOverflow;
    }
    if truncated < cutoff as f64 {
        Coercion::Aged
    } else {
        Coercion::Kept {
            compatibility: false,
        }
    }
}

fn compare_ascii_integer(raw: &[u8], cutoff: i64) -> Coercion {
    let (negative, digits) = match raw.first() {
        Some(b'-') => (true, &raw[1..]),
        _ => (false, raw),
    };
    let digits = trim_zeroes(digits);
    if digits.is_empty() {
        return compare_i64(0, cutoff);
    }
    if negative {
        return if cutoff > 0 {
            Coercion::Aged
        } else {
            compare_decimal(false, digits, cutoff)
        };
    }
    compare_decimal(true, digits, cutoff)
}

fn compare_decimal(positive: bool, digits: &[u8], cutoff: i64) -> Coercion {
    let cutoff_text = cutoff.unsigned_abs().to_string();
    let relation = digits
        .len()
        .cmp(&cutoff_text.len())
        .then_with(|| digits.cmp(cutoff_text.as_bytes()));
    let less = match (positive, cutoff.is_negative()) {
        (true, true) => false,
        (false, false) => true,
        (true, false) => relation.is_lt(),
        (false, true) => relation.is_gt(),
    };
    if less {
        Coercion::Aged
    } else {
        Coercion::Kept {
            compatibility: false,
        }
    }
}

fn trim_zeroes(digits: &[u8]) -> &[u8] {
    let index = digits
        .iter()
        .position(|digit| *digit != b'0')
        .unwrap_or(digits.len());
    &digits[index..]
}

fn coerce_string(raw: &[u8], cutoff: i64) -> Coercion {
    let Some(points) = decode_string(raw) else {
        return Coercion::Kept {
            compatibility: false,
        };
    };
    let mut start = 0;
    let mut end = points.len();
    while start < end && is_python_whitespace(points[start]) {
        start += 1;
    }
    while end > start && is_python_whitespace(points[end - 1]) {
        end -= 1;
    }
    if start == end {
        return Coercion::Kept {
            compatibility: false,
        };
    }
    let mut index = start;
    let negative = match points[index] {
        0x2d => {
            index += 1;
            true
        }
        0x2b => {
            index += 1;
            false
        }
        _ => false,
    };
    let mut digits = Vec::new();
    let mut previous_was_digit = false;
    while index < end {
        if let Some(digit) = nd_digit(points[index]) {
            digits.push(b'0' + digit);
            previous_was_digit = true;
        } else if points[index] == u32::from(b'_') {
            let next_is_digit = points
                .get(index + 1)
                .and_then(|point| nd_digit(*point))
                .is_some();
            if !previous_was_digit || !next_is_digit {
                return Coercion::Kept {
                    compatibility: false,
                };
            }
            previous_was_digit = false;
        } else {
            return Coercion::Kept {
                compatibility: false,
            };
        }
        index += 1;
    }
    if digits.is_empty() || !previous_was_digit || digits.len() > 4300 {
        return Coercion::Kept {
            compatibility: false,
        };
    }
    let digits = trim_zeroes(&digits);
    if digits.is_empty() {
        return compare_i64(0, cutoff);
    }
    if negative {
        if cutoff > 0 {
            Coercion::Aged
        } else {
            compare_decimal(false, digits, cutoff)
        }
    } else {
        compare_decimal(true, digits, cutoff)
    }
}
