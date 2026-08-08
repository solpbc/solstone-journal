// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const DAYS_IN_MONTH: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// Parses an ASCII decimal slice without truncation.
pub(crate) fn parse_ascii_digits(bytes: &[u8]) -> Option<u16> {
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

/// Returns whether a year and month are within the supported calendar range.
pub(crate) const fn is_valid_year_month(year: u16, month: u8) -> bool {
    year >= 1 && year <= 9999 && month >= 1 && month <= 12
}

/// Returns whether a year is a proleptic-Gregorian leap year.
pub(crate) const fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// Returns the number of days in a validated calendar month.
pub(crate) fn days_in_month(year: u16, month: u8) -> u8 {
    debug_assert!(is_valid_year_month(year, month));
    if month == 2 && is_leap_year(year) {
        29
    } else {
        DAYS_IN_MONTH[usize::from(month - 1)]
    }
}
