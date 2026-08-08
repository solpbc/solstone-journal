// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{BodyDay, BodyMonth, BodyString};

const DAYS_IN_MONTH: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DAY_ANCHORS: &[&[u8]] = &[
    b"00010101",
    b"99991231",
    b"20000229",
    b"19000228",
    b"20000228",
    b"20240229",
    b"20240228",
];
const MONTH_ANCHORS: &[&[u8]] = &[b"0001-01", b"9999-12", b"1900-02", b"2000-02", b"2024-02"];

fn body_string(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("test code points are valid")
}

fn parse_ascii_digits(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0_u16, |value, byte| {
        let digit = byte.checked_sub(b'0')?;
        if digit > 9 {
            return None;
        }
        value.checked_mul(10)?.checked_add(u16::from(digit))
    })
}

fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    if month == 2 && is_leap_year(year) {
        29
    } else {
        DAYS_IN_MONTH[usize::from(month - 1)]
    }
}

fn valid_month_values(year: u16, month: u8) -> bool {
    (1..=9999).contains(&year) && (1..=12).contains(&month)
}

fn valid_day_bytes(bytes: &[u8]) -> bool {
    if bytes.len() != 8 {
        return false;
    }
    let Some(year) = parse_ascii_digits(&bytes[..4]) else {
        return false;
    };
    let Some(month) = parse_ascii_digits(&bytes[4..6]) else {
        return false;
    };
    let Some(day) = parse_ascii_digits(&bytes[6..]) else {
        return false;
    };
    let Ok(month) = u8::try_from(month) else {
        return false;
    };
    let Ok(day) = u8::try_from(day) else {
        return false;
    };
    valid_month_values(year, month) && (1..=days_in_month(year, month)).contains(&day)
}

fn valid_month_bytes(bytes: &[u8]) -> bool {
    if bytes.len() != 7 || bytes[4] != b'-' {
        return false;
    }
    let Some(year) = parse_ascii_digits(&bytes[..4]) else {
        return false;
    };
    let Some(month) = parse_ascii_digits(&bytes[5..]) else {
        return false;
    };
    let Ok(month) = u8::try_from(month) else {
        return false;
    };
    valid_month_values(year, month)
}

fn valid_body_string(code_points: &[u32], valid_bytes: impl Fn(&[u8]) -> bool) -> bool {
    if code_points.iter().any(|code_point| *code_point > 0x7f) {
        return false;
    }
    let Some(bytes) = code_points
        .iter()
        .copied()
        .map(u8::try_from)
        .collect::<Result<Vec<_>, _>>()
        .ok()
    else {
        return false;
    };
    valid_bytes(&bytes)
}

fn assert_day_bytes(bytes: &[u8]) {
    let expected = valid_day_bytes(bytes);
    let from_bytes = catch_unwind(AssertUnwindSafe(|| BodyDay::from_bytes(bytes)));
    assert!(from_bytes.is_ok());
    assert_eq!(
        from_bytes.expect("constructor did not panic").is_ok(),
        expected
    );

    let code_points = bytes.iter().copied().map(u32::from).collect();
    assert_day_code_points(code_points);
}

fn assert_month_bytes(bytes: &[u8]) {
    let expected = valid_month_bytes(bytes);
    let from_bytes = catch_unwind(AssertUnwindSafe(|| BodyMonth::from_bytes(bytes)));
    assert!(from_bytes.is_ok());
    assert_eq!(
        from_bytes.expect("constructor did not panic").is_ok(),
        expected
    );

    let code_points = bytes.iter().copied().map(u32::from).collect();
    assert_month_code_points(code_points);
}

fn assert_day_code_points(code_points: Vec<u32>) {
    let expected = valid_body_string(&code_points, valid_day_bytes);
    let value = body_string(code_points);
    let from_body_string = catch_unwind(AssertUnwindSafe(|| BodyDay::from_body_string(&value)));
    assert!(from_body_string.is_ok());
    assert_eq!(
        from_body_string.expect("constructor did not panic").is_ok(),
        expected
    );
}

fn assert_month_code_points(code_points: Vec<u32>) {
    let expected = valid_body_string(&code_points, valid_month_bytes);
    let value = body_string(code_points);
    let from_body_string = catch_unwind(AssertUnwindSafe(|| BodyMonth::from_body_string(&value)));
    assert!(from_body_string.is_ok());
    assert_eq!(
        from_body_string.expect("constructor did not panic").is_ok(),
        expected
    );
}

fn fuzz_day_anchor(anchor: &[u8]) {
    for position in 0..anchor.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut substituted = anchor.to_vec();
            substituted[position] = byte;
            assert_day_bytes(&substituted);
        }
    }
    for position in 0..=anchor.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut inserted = anchor.to_vec();
            inserted.insert(position, byte);
            assert_day_bytes(&inserted);
        }
    }
    for position in 0..anchor.len() {
        let mut deleted = anchor.to_vec();
        deleted.remove(position);
        assert_day_bytes(&deleted);
    }
    for length in 0..anchor.len() {
        assert_day_bytes(&anchor[..length]);
    }
}

fn fuzz_month_anchor(anchor: &[u8]) {
    for position in 0..anchor.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut substituted = anchor.to_vec();
            substituted[position] = byte;
            assert_month_bytes(&substituted);
        }
    }
    for position in 0..=anchor.len() {
        for byte in u8::MIN..=u8::MAX {
            let mut inserted = anchor.to_vec();
            inserted.insert(position, byte);
            assert_month_bytes(&inserted);
        }
    }
    for position in 0..anchor.len() {
        let mut deleted = anchor.to_vec();
        deleted.remove(position);
        assert_month_bytes(&deleted);
    }
    for length in 0..anchor.len() {
        assert_month_bytes(&anchor[..length]);
    }
}

#[test]
fn calendar_validation_is_exhaustive_over_boundary_wire_mutations() {
    for anchor in DAY_ANCHORS {
        fuzz_day_anchor(anchor);
    }
    for anchor in MONTH_ANCHORS {
        fuzz_month_anchor(anchor);
    }

    let mut oversized_day = DAY_ANCHORS[0].to_vec();
    oversized_day.extend(vec![b'x'; 1_048_576]);
    assert_day_bytes(&oversized_day);
    let mut oversized_month = MONTH_ANCHORS[0].to_vec();
    oversized_month.extend(vec![b'x'; 1_048_576]);
    assert_month_bytes(&oversized_month);

    assert_day_bytes(&[b'2', 0xff, 0xfe, b'4', b'0', b'2', b'2', b'9']);
    assert_month_bytes(&[b'2', 0xff, 0xfe, b'4', b'-', b'0', b'2']);

    for code_point in [0x0100, 0x2603, 0x1f600, 0xd800, 0xdfff] {
        let mut day_points: Vec<u32> = DAY_ANCHORS[0].iter().copied().map(u32::from).collect();
        day_points[0] = code_point;
        assert_day_code_points(day_points);

        let mut month_points: Vec<u32> = MONTH_ANCHORS[0].iter().copied().map(u32::from).collect();
        month_points[0] = code_point;
        assert_month_code_points(month_points);
    }
}
