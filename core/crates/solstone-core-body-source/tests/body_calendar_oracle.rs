// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Ordering;

use solstone_core_body_source::{BodyDay, BodyMonth, BodyString};

const ORACLE_DAYS_IN_MONTH: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

fn body_string(bytes: &[u8]) -> BodyString {
    BodyString::from_code_points(bytes.iter().copied().map(u32::from).collect())
        .expect("byte values are valid body-string code points")
}

fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    if month == 2 && is_leap_year(year) {
        29
    } else {
        ORACLE_DAYS_IN_MONTH[usize::from(month - 1)]
    }
}

fn valid_month(year: u16, month: u8) -> bool {
    (1..=9999).contains(&year) && (1..=12).contains(&month)
}

fn valid_day(year: u16, month: u8, day: u8) -> bool {
    valid_month(year, month) && (1..=days_in_month(year, month)).contains(&day)
}

fn four_digits(value: u16) -> [u8; 4] {
    [
        b'0' + (value / 1000) as u8,
        b'0' + ((value / 100) % 10) as u8,
        b'0' + ((value / 10) % 10) as u8,
        b'0' + (value % 10) as u8,
    ]
}

fn two_digits(value: u8) -> [u8; 2] {
    [b'0' + value / 10, b'0' + value % 10]
}

fn day_bytes(year: u16, month: u8, day: u8) -> [u8; 8] {
    let year = four_digits(year);
    let month = two_digits(month);
    let day = two_digits(day);
    [
        year[0], year[1], year[2], year[3], month[0], month[1], day[0], day[1],
    ]
}

fn overflowing_day_bytes(month: u8, day: u8) -> [u8; 9] {
    let month = two_digits(month);
    let day = two_digits(day);
    [
        b'1', b'0', b'0', b'0', b'0', month[0], month[1], day[0], day[1],
    ]
}

fn month_bytes(year: u16, month: u8) -> [u8; 7] {
    let year = four_digits(year);
    let month = two_digits(month);
    [year[0], year[1], year[2], year[3], b'-', month[0], month[1]]
}

fn overflowing_month_bytes(month: u8) -> [u8; 8] {
    let month = two_digits(month);
    [b'1', b'0', b'0', b'0', b'0', b'-', month[0], month[1]]
}

fn check_day(
    bytes: &[u8],
    expected: bool,
    expected_month: Option<&[u8]>,
    previous_day: &mut Option<BodyDay>,
    previous_projected_month: &mut Option<BodyMonth>,
) {
    let from_bytes = BodyDay::from_bytes(bytes);
    let from_body_string = BodyDay::from_body_string(&body_string(bytes));
    assert_eq!(from_bytes.is_ok(), expected);
    assert_eq!(from_body_string.is_ok(), expected);
    assert_eq!(from_bytes.is_ok(), from_body_string.is_ok());

    if let Ok(day) = from_bytes {
        let projected_month = day.month();
        assert_eq!(
            projected_month.as_str().as_bytes(),
            expected_month.expect("valid day month")
        );
        if let Some(previous_day) = previous_day.as_ref() {
            let byte_order = previous_day
                .as_str()
                .as_bytes()
                .cmp(day.as_str().as_bytes());
            assert_eq!(previous_day.cmp(&day), byte_order);
            assert_eq!(byte_order, Ordering::Less);
        }
        if let Some(previous_month) = previous_projected_month.as_ref() {
            assert!(previous_month <= &projected_month);
        }
        *previous_day = Some(day);
        *previous_projected_month = Some(projected_month);
    }
}

fn check_month(bytes: &[u8], expected: bool, previous_month: &mut Option<BodyMonth>) {
    let from_bytes = BodyMonth::from_bytes(bytes);
    let from_body_string = BodyMonth::from_body_string(&body_string(bytes));
    assert_eq!(from_bytes.is_ok(), expected);
    assert_eq!(from_body_string.is_ok(), expected);
    assert_eq!(from_bytes.is_ok(), from_body_string.is_ok());

    if let Ok(month) = from_bytes {
        if let Some(previous_month) = previous_month.as_ref() {
            let byte_order = previous_month
                .as_str()
                .as_bytes()
                .cmp(month.as_str().as_bytes());
            assert_eq!(previous_month.cmp(&month), byte_order);
            assert_eq!(byte_order, Ordering::Less);
        }
        *previous_month = Some(month);
    }
}

#[test]
fn calendar_constructors_match_the_independent_gregorian_oracle() {
    let mut previous_day = None;
    let mut previous_projected_month = None;
    for year in 0_u16..=9999 {
        for month in 0_u8..=13 {
            for day in 0_u8..=32 {
                let bytes = day_bytes(year, month, day);
                let expected = valid_day(year, month, day);
                let expected_month = month_bytes(year, month);
                check_day(
                    &bytes,
                    expected,
                    expected.then_some(expected_month.as_slice()),
                    &mut previous_day,
                    &mut previous_projected_month,
                );
            }
        }
    }
    for month in 0_u8..=13 {
        for day in 0_u8..=32 {
            check_day(
                &overflowing_day_bytes(month, day),
                false,
                None,
                &mut previous_day,
                &mut previous_projected_month,
            );
        }
    }

    let mut previous_month = None;
    for year in 0_u16..=9999 {
        for month in 0_u8..=13 {
            let bytes = month_bytes(year, month);
            check_month(&bytes, valid_month(year, month), &mut previous_month);
        }
    }
    for month in 0_u8..=13 {
        check_month(&overflowing_month_bytes(month), false, &mut previous_month);
    }
}
