// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixture instants for the home tests.

use jiff::Timestamp;
use jiff::tz::Offset;

/// The UTC instant at a wall-clock date and time; panics on a field out of
/// range.
pub(crate) fn utc_at(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Timestamp {
    let field = |value: u32| i8::try_from(value).expect("field in range");
    let date = jiff::civil::date(
        i16::try_from(year).expect("year in range"),
        field(month),
        field(day),
    );
    Offset::UTC
        .to_timestamp(date.at(field(hour), field(minute), field(second), 0))
        .expect("instant in range")
}
