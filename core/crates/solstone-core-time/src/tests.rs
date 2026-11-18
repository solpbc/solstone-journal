// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Golden values: each expected string is what the journal wrote before it
//! moved to jiff, captured once from that implementation.

use jiff::Timestamp;
use jiff::civil::date;
use jiff::tz::{Offset, TimeZone};

use crate::*;

fn ts(second: i64, nanosecond: i32) -> Timestamp {
    Timestamp::new(second, nanosecond).unwrap()
}

fn off(seconds: i32) -> Offset {
    Offset::from_seconds(seconds).unwrap()
}

#[test]
fn rfc3339_forms() {
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Secs, false),
        "1970-01-01T00:00:00+00:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Secs, true),
        "1970-01-01T00:00:00Z"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Millis, false),
        "1970-01-01T00:00:00.000+00:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Millis, true),
        "1970-01-01T00:00:00.000Z"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Micros, false),
        "1970-01-01T00:00:00.000000+00:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Micros, true),
        "1970-01-01T00:00:00.000000Z"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Nanos, false),
        "1970-01-01T00:00:00.000000000+00:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::Nanos, true),
        "1970-01-01T00:00:00.000000000Z"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::AutoSi, false),
        "1970-01-01T00:00:00+00:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(0), Precision::AutoSi, true),
        "1970-01-01T00:00:00Z"
    );
    assert_eq!(
        offset_display(&at_offset(ts(0, 0), off(0))),
        "1970-01-01 00:00:00 +00:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Secs, false),
        "1969-12-31T17:00:00-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Secs, true),
        "1969-12-31T17:00:00-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Millis, false),
        "1969-12-31T17:00:00.000-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Millis, true),
        "1969-12-31T17:00:00.000-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Micros, false),
        "1969-12-31T17:00:00.000000-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Micros, true),
        "1969-12-31T17:00:00.000000-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Nanos, false),
        "1969-12-31T17:00:00.000000000-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::Nanos, true),
        "1969-12-31T17:00:00.000000000-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::AutoSi, false),
        "1969-12-31T17:00:00-07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-25200), Precision::AutoSi, true),
        "1969-12-31T17:00:00-07:00"
    );
    assert_eq!(
        offset_display(&at_offset(ts(0, 0), off(-25200))),
        "1969-12-31 17:00:00 -07:00"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Secs, false),
        "1970-01-01T05:30:00+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Secs, true),
        "1970-01-01T05:30:00+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Millis, false),
        "1970-01-01T05:30:00.000+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Millis, true),
        "1970-01-01T05:30:00.000+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Micros, false),
        "1970-01-01T05:30:00.000000+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Micros, true),
        "1970-01-01T05:30:00.000000+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Nanos, false),
        "1970-01-01T05:30:00.000000000+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::Nanos, true),
        "1970-01-01T05:30:00.000000000+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::AutoSi, false),
        "1970-01-01T05:30:00+05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(19800), Precision::AutoSi, true),
        "1970-01-01T05:30:00+05:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(0, 0), off(19800))),
        "1970-01-01 05:30:00 +05:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Secs, false),
        "1969-12-31T20:30:00-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Secs, true),
        "1969-12-31T20:30:00-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Millis, false),
        "1969-12-31T20:30:00.000-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Millis, true),
        "1969-12-31T20:30:00.000-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Micros, false),
        "1969-12-31T20:30:00.000000-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Micros, true),
        "1969-12-31T20:30:00.000000-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Nanos, false),
        "1969-12-31T20:30:00.000000000-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::Nanos, true),
        "1969-12-31T20:30:00.000000000-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::AutoSi, false),
        "1969-12-31T20:30:00-03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(-12600), Precision::AutoSi, true),
        "1969-12-31T20:30:00-03:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(0, 0), off(-12600))),
        "1969-12-31 20:30:00 -03:30"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Secs, false),
        "1970-01-01T12:45:00+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Secs, true),
        "1970-01-01T12:45:00+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Millis, false),
        "1970-01-01T12:45:00.000+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Millis, true),
        "1970-01-01T12:45:00.000+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Micros, false),
        "1970-01-01T12:45:00.000000+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Micros, true),
        "1970-01-01T12:45:00.000000+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Nanos, false),
        "1970-01-01T12:45:00.000000000+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::Nanos, true),
        "1970-01-01T12:45:00.000000000+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::AutoSi, false),
        "1970-01-01T12:45:00+12:45"
    );
    assert_eq!(
        rfc3339(ts(0, 0), off(45900), Precision::AutoSi, true),
        "1970-01-01T12:45:00+12:45"
    );
    assert_eq!(
        offset_display(&at_offset(ts(0, 0), off(45900))),
        "1970-01-01 12:45:00 +12:45"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Secs, false),
        "2026-11-23T20:00:00+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Secs, true),
        "2026-11-23T20:00:00Z"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Millis, false),
        "2026-11-23T20:00:00.120+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Millis, true),
        "2026-11-23T20:00:00.120Z"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Micros, false),
        "2026-11-23T20:00:00.120000+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Micros, true),
        "2026-11-23T20:00:00.120000Z"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Nanos, false),
        "2026-11-23T20:00:00.120000000+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::Nanos, true),
        "2026-11-23T20:00:00.120000000Z"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::AutoSi, false),
        "2026-11-23T20:00:00.120+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(0), Precision::AutoSi, true),
        "2026-11-23T20:00:00.120Z"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795464000, 120000000), off(0))),
        "2026-11-23 20:00:00.120 +00:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Secs,
            false
        ),
        "2026-11-23T13:00:00-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Secs,
            true
        ),
        "2026-11-23T13:00:00-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Millis,
            false
        ),
        "2026-11-23T13:00:00.120-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Millis,
            true
        ),
        "2026-11-23T13:00:00.120-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Micros,
            false
        ),
        "2026-11-23T13:00:00.120000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Micros,
            true
        ),
        "2026-11-23T13:00:00.120000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Nanos,
            false
        ),
        "2026-11-23T13:00:00.120000000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::Nanos,
            true
        ),
        "2026-11-23T13:00:00.120000000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::AutoSi,
            false
        ),
        "2026-11-23T13:00:00.120-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-25200),
            Precision::AutoSi,
            true
        ),
        "2026-11-23T13:00:00.120-07:00"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795464000, 120000000), off(-25200))),
        "2026-11-23 13:00:00.120 -07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::Secs,
            false
        ),
        "2026-11-24T01:30:00+05:30"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(19800), Precision::Secs, true),
        "2026-11-24T01:30:00+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::Millis,
            false
        ),
        "2026-11-24T01:30:00.120+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::Millis,
            true
        ),
        "2026-11-24T01:30:00.120+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::Micros,
            false
        ),
        "2026-11-24T01:30:00.120000+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::Micros,
            true
        ),
        "2026-11-24T01:30:00.120000+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::Nanos,
            false
        ),
        "2026-11-24T01:30:00.120000000+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::Nanos,
            true
        ),
        "2026-11-24T01:30:00.120000000+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::AutoSi,
            false
        ),
        "2026-11-24T01:30:00.120+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(19800),
            Precision::AutoSi,
            true
        ),
        "2026-11-24T01:30:00.120+05:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795464000, 120000000), off(19800))),
        "2026-11-24 01:30:00.120 +05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Secs,
            false
        ),
        "2026-11-23T16:30:00-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Secs,
            true
        ),
        "2026-11-23T16:30:00-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Millis,
            false
        ),
        "2026-11-23T16:30:00.120-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Millis,
            true
        ),
        "2026-11-23T16:30:00.120-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Micros,
            false
        ),
        "2026-11-23T16:30:00.120000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Micros,
            true
        ),
        "2026-11-23T16:30:00.120000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Nanos,
            false
        ),
        "2026-11-23T16:30:00.120000000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::Nanos,
            true
        ),
        "2026-11-23T16:30:00.120000000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::AutoSi,
            false
        ),
        "2026-11-23T16:30:00.120-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(-12600),
            Precision::AutoSi,
            true
        ),
        "2026-11-23T16:30:00.120-03:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795464000, 120000000), off(-12600))),
        "2026-11-23 16:30:00.120 -03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::Secs,
            false
        ),
        "2026-11-24T08:45:00+12:45"
    );
    assert_eq!(
        rfc3339(ts(1795464000, 120000000), off(45900), Precision::Secs, true),
        "2026-11-24T08:45:00+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::Millis,
            false
        ),
        "2026-11-24T08:45:00.120+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::Millis,
            true
        ),
        "2026-11-24T08:45:00.120+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::Micros,
            false
        ),
        "2026-11-24T08:45:00.120000+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::Micros,
            true
        ),
        "2026-11-24T08:45:00.120000+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::Nanos,
            false
        ),
        "2026-11-24T08:45:00.120000000+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::Nanos,
            true
        ),
        "2026-11-24T08:45:00.120000000+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::AutoSi,
            false
        ),
        "2026-11-24T08:45:00.120+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795464000, 120000000),
            off(45900),
            Precision::AutoSi,
            true
        ),
        "2026-11-24T08:45:00.120+12:45"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795464000, 120000000), off(45900))),
        "2026-11-24 08:45:00.120 +12:45"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Secs, false),
        "2026-11-24T07:59:59+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Secs, true),
        "2026-11-24T07:59:59Z"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Millis, false),
        "2026-11-24T07:59:59.123+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Millis, true),
        "2026-11-24T07:59:59.123Z"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Micros, false),
        "2026-11-24T07:59:59.123456+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Micros, true),
        "2026-11-24T07:59:59.123456Z"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Nanos, false),
        "2026-11-24T07:59:59.123456000+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::Nanos, true),
        "2026-11-24T07:59:59.123456000Z"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::AutoSi, false),
        "2026-11-24T07:59:59.123456+00:00"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(0), Precision::AutoSi, true),
        "2026-11-24T07:59:59.123456Z"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795507199, 123456000), off(0))),
        "2026-11-24 07:59:59.123456 +00:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Secs,
            false
        ),
        "2026-11-24T00:59:59-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Secs,
            true
        ),
        "2026-11-24T00:59:59-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Millis,
            false
        ),
        "2026-11-24T00:59:59.123-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Millis,
            true
        ),
        "2026-11-24T00:59:59.123-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Micros,
            false
        ),
        "2026-11-24T00:59:59.123456-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Micros,
            true
        ),
        "2026-11-24T00:59:59.123456-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Nanos,
            false
        ),
        "2026-11-24T00:59:59.123456000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::Nanos,
            true
        ),
        "2026-11-24T00:59:59.123456000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::AutoSi,
            false
        ),
        "2026-11-24T00:59:59.123456-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-25200),
            Precision::AutoSi,
            true
        ),
        "2026-11-24T00:59:59.123456-07:00"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795507199, 123456000), off(-25200))),
        "2026-11-24 00:59:59.123456 -07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::Secs,
            false
        ),
        "2026-11-24T13:29:59+05:30"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(19800), Precision::Secs, true),
        "2026-11-24T13:29:59+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::Millis,
            false
        ),
        "2026-11-24T13:29:59.123+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::Millis,
            true
        ),
        "2026-11-24T13:29:59.123+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::Micros,
            false
        ),
        "2026-11-24T13:29:59.123456+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::Micros,
            true
        ),
        "2026-11-24T13:29:59.123456+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::Nanos,
            false
        ),
        "2026-11-24T13:29:59.123456000+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::Nanos,
            true
        ),
        "2026-11-24T13:29:59.123456000+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::AutoSi,
            false
        ),
        "2026-11-24T13:29:59.123456+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(19800),
            Precision::AutoSi,
            true
        ),
        "2026-11-24T13:29:59.123456+05:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795507199, 123456000), off(19800))),
        "2026-11-24 13:29:59.123456 +05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Secs,
            false
        ),
        "2026-11-24T04:29:59-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Secs,
            true
        ),
        "2026-11-24T04:29:59-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Millis,
            false
        ),
        "2026-11-24T04:29:59.123-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Millis,
            true
        ),
        "2026-11-24T04:29:59.123-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Micros,
            false
        ),
        "2026-11-24T04:29:59.123456-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Micros,
            true
        ),
        "2026-11-24T04:29:59.123456-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Nanos,
            false
        ),
        "2026-11-24T04:29:59.123456000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::Nanos,
            true
        ),
        "2026-11-24T04:29:59.123456000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::AutoSi,
            false
        ),
        "2026-11-24T04:29:59.123456-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(-12600),
            Precision::AutoSi,
            true
        ),
        "2026-11-24T04:29:59.123456-03:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795507199, 123456000), off(-12600))),
        "2026-11-24 04:29:59.123456 -03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::Secs,
            false
        ),
        "2026-11-24T20:44:59+12:45"
    );
    assert_eq!(
        rfc3339(ts(1795507199, 123456000), off(45900), Precision::Secs, true),
        "2026-11-24T20:44:59+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::Millis,
            false
        ),
        "2026-11-24T20:44:59.123+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::Millis,
            true
        ),
        "2026-11-24T20:44:59.123+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::Micros,
            false
        ),
        "2026-11-24T20:44:59.123456+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::Micros,
            true
        ),
        "2026-11-24T20:44:59.123456+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::Nanos,
            false
        ),
        "2026-11-24T20:44:59.123456000+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::Nanos,
            true
        ),
        "2026-11-24T20:44:59.123456000+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::AutoSi,
            false
        ),
        "2026-11-24T20:44:59.123456+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1795507199, 123456000),
            off(45900),
            Precision::AutoSi,
            true
        ),
        "2026-11-24T20:44:59.123456+12:45"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1795507199, 123456000), off(45900))),
        "2026-11-24 20:44:59.123456 +12:45"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Secs, false),
        "2000-02-29T00:00:00+00:00"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Secs, true),
        "2000-02-29T00:00:00Z"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Millis, false),
        "2000-02-29T00:00:00.123+00:00"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Millis, true),
        "2000-02-29T00:00:00.123Z"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Micros, false),
        "2000-02-29T00:00:00.123456+00:00"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Micros, true),
        "2000-02-29T00:00:00.123456Z"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Nanos, false),
        "2000-02-29T00:00:00.123456789+00:00"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::Nanos, true),
        "2000-02-29T00:00:00.123456789Z"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::AutoSi, false),
        "2000-02-29T00:00:00.123456789+00:00"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(0), Precision::AutoSi, true),
        "2000-02-29T00:00:00.123456789Z"
    );
    assert_eq!(
        offset_display(&at_offset(ts(951782400, 123456789), off(0))),
        "2000-02-29 00:00:00.123456789 +00:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::Secs,
            false
        ),
        "2000-02-28T17:00:00-07:00"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(-25200), Precision::Secs, true),
        "2000-02-28T17:00:00-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::Millis,
            false
        ),
        "2000-02-28T17:00:00.123-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::Millis,
            true
        ),
        "2000-02-28T17:00:00.123-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::Micros,
            false
        ),
        "2000-02-28T17:00:00.123456-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::Micros,
            true
        ),
        "2000-02-28T17:00:00.123456-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::Nanos,
            false
        ),
        "2000-02-28T17:00:00.123456789-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::Nanos,
            true
        ),
        "2000-02-28T17:00:00.123456789-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::AutoSi,
            false
        ),
        "2000-02-28T17:00:00.123456789-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-25200),
            Precision::AutoSi,
            true
        ),
        "2000-02-28T17:00:00.123456789-07:00"
    );
    assert_eq!(
        offset_display(&at_offset(ts(951782400, 123456789), off(-25200))),
        "2000-02-28 17:00:00.123456789 -07:00"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(19800), Precision::Secs, false),
        "2000-02-29T05:30:00+05:30"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(19800), Precision::Secs, true),
        "2000-02-29T05:30:00+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(19800),
            Precision::Millis,
            false
        ),
        "2000-02-29T05:30:00.123+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(19800),
            Precision::Millis,
            true
        ),
        "2000-02-29T05:30:00.123+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(19800),
            Precision::Micros,
            false
        ),
        "2000-02-29T05:30:00.123456+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(19800),
            Precision::Micros,
            true
        ),
        "2000-02-29T05:30:00.123456+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(19800),
            Precision::Nanos,
            false
        ),
        "2000-02-29T05:30:00.123456789+05:30"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(19800), Precision::Nanos, true),
        "2000-02-29T05:30:00.123456789+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(19800),
            Precision::AutoSi,
            false
        ),
        "2000-02-29T05:30:00.123456789+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(19800),
            Precision::AutoSi,
            true
        ),
        "2000-02-29T05:30:00.123456789+05:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(951782400, 123456789), off(19800))),
        "2000-02-29 05:30:00.123456789 +05:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::Secs,
            false
        ),
        "2000-02-28T20:30:00-03:30"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(-12600), Precision::Secs, true),
        "2000-02-28T20:30:00-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::Millis,
            false
        ),
        "2000-02-28T20:30:00.123-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::Millis,
            true
        ),
        "2000-02-28T20:30:00.123-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::Micros,
            false
        ),
        "2000-02-28T20:30:00.123456-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::Micros,
            true
        ),
        "2000-02-28T20:30:00.123456-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::Nanos,
            false
        ),
        "2000-02-28T20:30:00.123456789-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::Nanos,
            true
        ),
        "2000-02-28T20:30:00.123456789-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::AutoSi,
            false
        ),
        "2000-02-28T20:30:00.123456789-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(-12600),
            Precision::AutoSi,
            true
        ),
        "2000-02-28T20:30:00.123456789-03:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(951782400, 123456789), off(-12600))),
        "2000-02-28 20:30:00.123456789 -03:30"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(45900), Precision::Secs, false),
        "2000-02-29T12:45:00+12:45"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(45900), Precision::Secs, true),
        "2000-02-29T12:45:00+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(45900),
            Precision::Millis,
            false
        ),
        "2000-02-29T12:45:00.123+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(45900),
            Precision::Millis,
            true
        ),
        "2000-02-29T12:45:00.123+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(45900),
            Precision::Micros,
            false
        ),
        "2000-02-29T12:45:00.123456+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(45900),
            Precision::Micros,
            true
        ),
        "2000-02-29T12:45:00.123456+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(45900),
            Precision::Nanos,
            false
        ),
        "2000-02-29T12:45:00.123456789+12:45"
    );
    assert_eq!(
        rfc3339(ts(951782400, 123456789), off(45900), Precision::Nanos, true),
        "2000-02-29T12:45:00.123456789+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(45900),
            Precision::AutoSi,
            false
        ),
        "2000-02-29T12:45:00.123456789+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(951782400, 123456789),
            off(45900),
            Precision::AutoSi,
            true
        ),
        "2000-02-29T12:45:00.123456789+12:45"
    );
    assert_eq!(
        offset_display(&at_offset(ts(951782400, 123456789), off(45900))),
        "2000-02-29 12:45:00.123456789 +12:45"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Secs, false),
        "2001-09-09T01:46:40+00:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Secs, true),
        "2001-09-09T01:46:40Z"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Millis, false),
        "2001-09-09T01:46:40.005+00:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Millis, true),
        "2001-09-09T01:46:40.005Z"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Micros, false),
        "2001-09-09T01:46:40.005000+00:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Micros, true),
        "2001-09-09T01:46:40.005000Z"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Nanos, false),
        "2001-09-09T01:46:40.005000000+00:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::Nanos, true),
        "2001-09-09T01:46:40.005000000Z"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::AutoSi, false),
        "2001-09-09T01:46:40.005+00:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(0), Precision::AutoSi, true),
        "2001-09-09T01:46:40.005Z"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1000000000, 5000000), off(0))),
        "2001-09-09 01:46:40.005 +00:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(-25200), Precision::Secs, false),
        "2001-09-08T18:46:40-07:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(-25200), Precision::Secs, true),
        "2001-09-08T18:46:40-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-25200),
            Precision::Millis,
            false
        ),
        "2001-09-08T18:46:40.005-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-25200),
            Precision::Millis,
            true
        ),
        "2001-09-08T18:46:40.005-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-25200),
            Precision::Micros,
            false
        ),
        "2001-09-08T18:46:40.005000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-25200),
            Precision::Micros,
            true
        ),
        "2001-09-08T18:46:40.005000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-25200),
            Precision::Nanos,
            false
        ),
        "2001-09-08T18:46:40.005000000-07:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(-25200), Precision::Nanos, true),
        "2001-09-08T18:46:40.005000000-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-25200),
            Precision::AutoSi,
            false
        ),
        "2001-09-08T18:46:40.005-07:00"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-25200),
            Precision::AutoSi,
            true
        ),
        "2001-09-08T18:46:40.005-07:00"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1000000000, 5000000), off(-25200))),
        "2001-09-08 18:46:40.005 -07:00"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(19800), Precision::Secs, false),
        "2001-09-09T07:16:40+05:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(19800), Precision::Secs, true),
        "2001-09-09T07:16:40+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(19800),
            Precision::Millis,
            false
        ),
        "2001-09-09T07:16:40.005+05:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(19800), Precision::Millis, true),
        "2001-09-09T07:16:40.005+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(19800),
            Precision::Micros,
            false
        ),
        "2001-09-09T07:16:40.005000+05:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(19800), Precision::Micros, true),
        "2001-09-09T07:16:40.005000+05:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(19800), Precision::Nanos, false),
        "2001-09-09T07:16:40.005000000+05:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(19800), Precision::Nanos, true),
        "2001-09-09T07:16:40.005000000+05:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(19800),
            Precision::AutoSi,
            false
        ),
        "2001-09-09T07:16:40.005+05:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(19800), Precision::AutoSi, true),
        "2001-09-09T07:16:40.005+05:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1000000000, 5000000), off(19800))),
        "2001-09-09 07:16:40.005 +05:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(-12600), Precision::Secs, false),
        "2001-09-08T22:16:40-03:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(-12600), Precision::Secs, true),
        "2001-09-08T22:16:40-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-12600),
            Precision::Millis,
            false
        ),
        "2001-09-08T22:16:40.005-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-12600),
            Precision::Millis,
            true
        ),
        "2001-09-08T22:16:40.005-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-12600),
            Precision::Micros,
            false
        ),
        "2001-09-08T22:16:40.005000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-12600),
            Precision::Micros,
            true
        ),
        "2001-09-08T22:16:40.005000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-12600),
            Precision::Nanos,
            false
        ),
        "2001-09-08T22:16:40.005000000-03:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(-12600), Precision::Nanos, true),
        "2001-09-08T22:16:40.005000000-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-12600),
            Precision::AutoSi,
            false
        ),
        "2001-09-08T22:16:40.005-03:30"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(-12600),
            Precision::AutoSi,
            true
        ),
        "2001-09-08T22:16:40.005-03:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1000000000, 5000000), off(-12600))),
        "2001-09-08 22:16:40.005 -03:30"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(45900), Precision::Secs, false),
        "2001-09-09T14:31:40+12:45"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(45900), Precision::Secs, true),
        "2001-09-09T14:31:40+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(45900),
            Precision::Millis,
            false
        ),
        "2001-09-09T14:31:40.005+12:45"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(45900), Precision::Millis, true),
        "2001-09-09T14:31:40.005+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(45900),
            Precision::Micros,
            false
        ),
        "2001-09-09T14:31:40.005000+12:45"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(45900), Precision::Micros, true),
        "2001-09-09T14:31:40.005000+12:45"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(45900), Precision::Nanos, false),
        "2001-09-09T14:31:40.005000000+12:45"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(45900), Precision::Nanos, true),
        "2001-09-09T14:31:40.005000000+12:45"
    );
    assert_eq!(
        rfc3339(
            ts(1000000000, 5000000),
            off(45900),
            Precision::AutoSi,
            false
        ),
        "2001-09-09T14:31:40.005+12:45"
    );
    assert_eq!(
        rfc3339(ts(1000000000, 5000000), off(45900), Precision::AutoSi, true),
        "2001-09-09T14:31:40.005+12:45"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1000000000, 5000000), off(45900))),
        "2001-09-09 14:31:40.005 +12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Secs, false),
        "1969-12-31T23:59:59+00:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Secs, true),
        "1969-12-31T23:59:59Z"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Millis, false),
        "1969-12-31T23:59:59.999+00:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Millis, true),
        "1969-12-31T23:59:59.999Z"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Micros, false),
        "1969-12-31T23:59:59.999999+00:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Micros, true),
        "1969-12-31T23:59:59.999999Z"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Nanos, false),
        "1969-12-31T23:59:59.999999999+00:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::Nanos, true),
        "1969-12-31T23:59:59.999999999Z"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::AutoSi, false),
        "1969-12-31T23:59:59.999999999+00:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(0), Precision::AutoSi, true),
        "1969-12-31T23:59:59.999999999Z"
    );
    assert_eq!(
        offset_display(&at_offset(ts(-1, 999999999), off(0))),
        "1969-12-31 23:59:59.999999999 +00:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Secs, false),
        "1969-12-31T16:59:59-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Secs, true),
        "1969-12-31T16:59:59-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Millis, false),
        "1969-12-31T16:59:59.999-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Millis, true),
        "1969-12-31T16:59:59.999-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Micros, false),
        "1969-12-31T16:59:59.999999-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Micros, true),
        "1969-12-31T16:59:59.999999-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Nanos, false),
        "1969-12-31T16:59:59.999999999-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::Nanos, true),
        "1969-12-31T16:59:59.999999999-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::AutoSi, false),
        "1969-12-31T16:59:59.999999999-07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-25200), Precision::AutoSi, true),
        "1969-12-31T16:59:59.999999999-07:00"
    );
    assert_eq!(
        offset_display(&at_offset(ts(-1, 999999999), off(-25200))),
        "1969-12-31 16:59:59.999999999 -07:00"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Secs, false),
        "1970-01-01T05:29:59+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Secs, true),
        "1970-01-01T05:29:59+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Millis, false),
        "1970-01-01T05:29:59.999+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Millis, true),
        "1970-01-01T05:29:59.999+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Micros, false),
        "1970-01-01T05:29:59.999999+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Micros, true),
        "1970-01-01T05:29:59.999999+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Nanos, false),
        "1970-01-01T05:29:59.999999999+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::Nanos, true),
        "1970-01-01T05:29:59.999999999+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::AutoSi, false),
        "1970-01-01T05:29:59.999999999+05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(19800), Precision::AutoSi, true),
        "1970-01-01T05:29:59.999999999+05:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(-1, 999999999), off(19800))),
        "1970-01-01 05:29:59.999999999 +05:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Secs, false),
        "1969-12-31T20:29:59-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Secs, true),
        "1969-12-31T20:29:59-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Millis, false),
        "1969-12-31T20:29:59.999-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Millis, true),
        "1969-12-31T20:29:59.999-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Micros, false),
        "1969-12-31T20:29:59.999999-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Micros, true),
        "1969-12-31T20:29:59.999999-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Nanos, false),
        "1969-12-31T20:29:59.999999999-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::Nanos, true),
        "1969-12-31T20:29:59.999999999-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::AutoSi, false),
        "1969-12-31T20:29:59.999999999-03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(-12600), Precision::AutoSi, true),
        "1969-12-31T20:29:59.999999999-03:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(-1, 999999999), off(-12600))),
        "1969-12-31 20:29:59.999999999 -03:30"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Secs, false),
        "1970-01-01T12:44:59+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Secs, true),
        "1970-01-01T12:44:59+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Millis, false),
        "1970-01-01T12:44:59.999+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Millis, true),
        "1970-01-01T12:44:59.999+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Micros, false),
        "1970-01-01T12:44:59.999999+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Micros, true),
        "1970-01-01T12:44:59.999999+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Nanos, false),
        "1970-01-01T12:44:59.999999999+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::Nanos, true),
        "1970-01-01T12:44:59.999999999+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::AutoSi, false),
        "1970-01-01T12:44:59.999999999+12:45"
    );
    assert_eq!(
        rfc3339(ts(-1, 999999999), off(45900), Precision::AutoSi, true),
        "1970-01-01T12:44:59.999999999+12:45"
    );
    assert_eq!(
        offset_display(&at_offset(ts(-1, 999999999), off(45900))),
        "1970-01-01 12:44:59.999999999 +12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Secs, false),
        "2024-03-31T16:00:00+00:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Secs, true),
        "2024-03-31T16:00:00Z"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Millis, false),
        "2024-03-31T16:00:00.000+00:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Millis, true),
        "2024-03-31T16:00:00.000Z"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Micros, false),
        "2024-03-31T16:00:00.000000+00:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Micros, true),
        "2024-03-31T16:00:00.000000Z"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Nanos, false),
        "2024-03-31T16:00:00.000000001+00:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::Nanos, true),
        "2024-03-31T16:00:00.000000001Z"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::AutoSi, false),
        "2024-03-31T16:00:00.000000001+00:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(0), Precision::AutoSi, true),
        "2024-03-31T16:00:00.000000001Z"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1711900800, 1), off(0))),
        "2024-03-31 16:00:00.000000001 +00:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Secs, false),
        "2024-03-31T09:00:00-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Secs, true),
        "2024-03-31T09:00:00-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Millis, false),
        "2024-03-31T09:00:00.000-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Millis, true),
        "2024-03-31T09:00:00.000-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Micros, false),
        "2024-03-31T09:00:00.000000-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Micros, true),
        "2024-03-31T09:00:00.000000-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Nanos, false),
        "2024-03-31T09:00:00.000000001-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::Nanos, true),
        "2024-03-31T09:00:00.000000001-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::AutoSi, false),
        "2024-03-31T09:00:00.000000001-07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-25200), Precision::AutoSi, true),
        "2024-03-31T09:00:00.000000001-07:00"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1711900800, 1), off(-25200))),
        "2024-03-31 09:00:00.000000001 -07:00"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Secs, false),
        "2024-03-31T21:30:00+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Secs, true),
        "2024-03-31T21:30:00+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Millis, false),
        "2024-03-31T21:30:00.000+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Millis, true),
        "2024-03-31T21:30:00.000+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Micros, false),
        "2024-03-31T21:30:00.000000+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Micros, true),
        "2024-03-31T21:30:00.000000+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Nanos, false),
        "2024-03-31T21:30:00.000000001+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::Nanos, true),
        "2024-03-31T21:30:00.000000001+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::AutoSi, false),
        "2024-03-31T21:30:00.000000001+05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(19800), Precision::AutoSi, true),
        "2024-03-31T21:30:00.000000001+05:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1711900800, 1), off(19800))),
        "2024-03-31 21:30:00.000000001 +05:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Secs, false),
        "2024-03-31T12:30:00-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Secs, true),
        "2024-03-31T12:30:00-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Millis, false),
        "2024-03-31T12:30:00.000-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Millis, true),
        "2024-03-31T12:30:00.000-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Micros, false),
        "2024-03-31T12:30:00.000000-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Micros, true),
        "2024-03-31T12:30:00.000000-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Nanos, false),
        "2024-03-31T12:30:00.000000001-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::Nanos, true),
        "2024-03-31T12:30:00.000000001-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::AutoSi, false),
        "2024-03-31T12:30:00.000000001-03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(-12600), Precision::AutoSi, true),
        "2024-03-31T12:30:00.000000001-03:30"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1711900800, 1), off(-12600))),
        "2024-03-31 12:30:00.000000001 -03:30"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Secs, false),
        "2024-04-01T04:45:00+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Secs, true),
        "2024-04-01T04:45:00+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Millis, false),
        "2024-04-01T04:45:00.000+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Millis, true),
        "2024-04-01T04:45:00.000+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Micros, false),
        "2024-04-01T04:45:00.000000+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Micros, true),
        "2024-04-01T04:45:00.000000+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Nanos, false),
        "2024-04-01T04:45:00.000000001+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::Nanos, true),
        "2024-04-01T04:45:00.000000001+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::AutoSi, false),
        "2024-04-01T04:45:00.000000001+12:45"
    );
    assert_eq!(
        rfc3339(ts(1711900800, 1), off(45900), Precision::AutoSi, true),
        "2024-04-01T04:45:00.000000001+12:45"
    );
    assert_eq!(
        offset_display(&at_offset(ts(1711900800, 1), off(45900))),
        "2024-04-01 04:45:00.000000001 +12:45"
    );
}

#[test]
fn utc_and_civil_forms() {
    assert_eq!(utc_rfc3339(ts(0, 0)), "1970-01-01T00:00:00+00:00");
    assert_eq!(utc_iso(ts(0, 0)), "1970-01-01T00:00:00Z");
    assert_eq!(utc_display(ts(0, 0)), "1970-01-01 00:00:00 UTC");
    assert_eq!(
        civil_iso(Offset::UTC.to_datetime(ts(0, 0))),
        "1970-01-01T00:00:00"
    );
    assert_eq!(
        civil_display(Offset::UTC.to_datetime(ts(0, 0))),
        "1970-01-01 00:00:00"
    );
    assert_eq!(
        time_display(Offset::UTC.to_datetime(ts(0, 0)).time()),
        "00:00:00"
    );
    assert_eq!(
        strftime_civil(Offset::UTC.to_datetime(ts(0, 0)), "%Y-%m-%dT%H:%M:%S%.f"),
        "1970-01-01T00:00:00"
    );
    assert_eq!(
        strftime_civil(Offset::UTC.to_datetime(ts(0, 0)), "%Y-%m-%dT%H:%M:%S%.fZ"),
        "1970-01-01T00:00:00Z"
    );
    assert_eq!(
        utc_rfc3339(ts(1795464000, 120000000)),
        "2026-11-23T20:00:00.120+00:00"
    );
    assert_eq!(
        utc_iso(ts(1795464000, 120000000)),
        "2026-11-23T20:00:00.120Z"
    );
    assert_eq!(
        utc_display(ts(1795464000, 120000000)),
        "2026-11-23 20:00:00.120 UTC"
    );
    assert_eq!(
        civil_iso(Offset::UTC.to_datetime(ts(1795464000, 120000000))),
        "2026-11-23T20:00:00.120"
    );
    assert_eq!(
        civil_display(Offset::UTC.to_datetime(ts(1795464000, 120000000))),
        "2026-11-23 20:00:00.120"
    );
    assert_eq!(
        time_display(Offset::UTC.to_datetime(ts(1795464000, 120000000)).time()),
        "20:00:00.120"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1795464000, 120000000)),
            "%Y-%m-%dT%H:%M:%S%.f"
        ),
        "2026-11-23T20:00:00.120"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1795464000, 120000000)),
            "%Y-%m-%dT%H:%M:%S%.fZ"
        ),
        "2026-11-23T20:00:00.120Z"
    );
    assert_eq!(
        utc_rfc3339(ts(1795507199, 123456000)),
        "2026-11-24T07:59:59.123456+00:00"
    );
    assert_eq!(
        utc_iso(ts(1795507199, 123456000)),
        "2026-11-24T07:59:59.123456Z"
    );
    assert_eq!(
        utc_display(ts(1795507199, 123456000)),
        "2026-11-24 07:59:59.123456 UTC"
    );
    assert_eq!(
        civil_iso(Offset::UTC.to_datetime(ts(1795507199, 123456000))),
        "2026-11-24T07:59:59.123456"
    );
    assert_eq!(
        civil_display(Offset::UTC.to_datetime(ts(1795507199, 123456000))),
        "2026-11-24 07:59:59.123456"
    );
    assert_eq!(
        time_display(Offset::UTC.to_datetime(ts(1795507199, 123456000)).time()),
        "07:59:59.123456"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1795507199, 123456000)),
            "%Y-%m-%dT%H:%M:%S%.f"
        ),
        "2026-11-24T07:59:59.123456"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1795507199, 123456000)),
            "%Y-%m-%dT%H:%M:%S%.fZ"
        ),
        "2026-11-24T07:59:59.123456Z"
    );
    assert_eq!(
        utc_rfc3339(ts(951782400, 123456789)),
        "2000-02-29T00:00:00.123456789+00:00"
    );
    assert_eq!(
        utc_iso(ts(951782400, 123456789)),
        "2000-02-29T00:00:00.123456789Z"
    );
    assert_eq!(
        utc_display(ts(951782400, 123456789)),
        "2000-02-29 00:00:00.123456789 UTC"
    );
    assert_eq!(
        civil_iso(Offset::UTC.to_datetime(ts(951782400, 123456789))),
        "2000-02-29T00:00:00.123456789"
    );
    assert_eq!(
        civil_display(Offset::UTC.to_datetime(ts(951782400, 123456789))),
        "2000-02-29 00:00:00.123456789"
    );
    assert_eq!(
        time_display(Offset::UTC.to_datetime(ts(951782400, 123456789)).time()),
        "00:00:00.123456789"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(951782400, 123456789)),
            "%Y-%m-%dT%H:%M:%S%.f"
        ),
        "2000-02-29T00:00:00.123456789"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(951782400, 123456789)),
            "%Y-%m-%dT%H:%M:%S%.fZ"
        ),
        "2000-02-29T00:00:00.123456789Z"
    );
    assert_eq!(
        utc_rfc3339(ts(1000000000, 5000000)),
        "2001-09-09T01:46:40.005+00:00"
    );
    assert_eq!(utc_iso(ts(1000000000, 5000000)), "2001-09-09T01:46:40.005Z");
    assert_eq!(
        utc_display(ts(1000000000, 5000000)),
        "2001-09-09 01:46:40.005 UTC"
    );
    assert_eq!(
        civil_iso(Offset::UTC.to_datetime(ts(1000000000, 5000000))),
        "2001-09-09T01:46:40.005"
    );
    assert_eq!(
        civil_display(Offset::UTC.to_datetime(ts(1000000000, 5000000))),
        "2001-09-09 01:46:40.005"
    );
    assert_eq!(
        time_display(Offset::UTC.to_datetime(ts(1000000000, 5000000)).time()),
        "01:46:40.005"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1000000000, 5000000)),
            "%Y-%m-%dT%H:%M:%S%.f"
        ),
        "2001-09-09T01:46:40.005"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1000000000, 5000000)),
            "%Y-%m-%dT%H:%M:%S%.fZ"
        ),
        "2001-09-09T01:46:40.005Z"
    );
    assert_eq!(
        utc_rfc3339(ts(-1, 999999999)),
        "1969-12-31T23:59:59.999999999+00:00"
    );
    assert_eq!(utc_iso(ts(-1, 999999999)), "1969-12-31T23:59:59.999999999Z");
    assert_eq!(
        utc_display(ts(-1, 999999999)),
        "1969-12-31 23:59:59.999999999 UTC"
    );
    assert_eq!(
        civil_iso(Offset::UTC.to_datetime(ts(-1, 999999999))),
        "1969-12-31T23:59:59.999999999"
    );
    assert_eq!(
        civil_display(Offset::UTC.to_datetime(ts(-1, 999999999))),
        "1969-12-31 23:59:59.999999999"
    );
    assert_eq!(
        time_display(Offset::UTC.to_datetime(ts(-1, 999999999)).time()),
        "23:59:59.999999999"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(-1, 999999999)),
            "%Y-%m-%dT%H:%M:%S%.f"
        ),
        "1969-12-31T23:59:59.999999999"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(-1, 999999999)),
            "%Y-%m-%dT%H:%M:%S%.fZ"
        ),
        "1969-12-31T23:59:59.999999999Z"
    );
    assert_eq!(
        utc_rfc3339(ts(1711900800, 1)),
        "2024-03-31T16:00:00.000000001+00:00"
    );
    assert_eq!(utc_iso(ts(1711900800, 1)), "2024-03-31T16:00:00.000000001Z");
    assert_eq!(
        utc_display(ts(1711900800, 1)),
        "2024-03-31 16:00:00.000000001 UTC"
    );
    assert_eq!(
        civil_iso(Offset::UTC.to_datetime(ts(1711900800, 1))),
        "2024-03-31T16:00:00.000000001"
    );
    assert_eq!(
        civil_display(Offset::UTC.to_datetime(ts(1711900800, 1))),
        "2024-03-31 16:00:00.000000001"
    );
    assert_eq!(
        time_display(Offset::UTC.to_datetime(ts(1711900800, 1)).time()),
        "16:00:00.000000001"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1711900800, 1)),
            "%Y-%m-%dT%H:%M:%S%.f"
        ),
        "2024-03-31T16:00:00.000000001"
    );
    assert_eq!(
        strftime_civil(
            Offset::UTC.to_datetime(ts(1711900800, 1)),
            "%Y-%m-%dT%H:%M:%S%.fZ"
        ),
        "2024-03-31T16:00:00.000000001Z"
    );
}

#[test]
fn named_zone_display() {
    assert_eq!(
        abbreviation_display(
            &ts(1784160000, 0).to_zoned_datetime(TimeZone::get("America/Denver").unwrap())
        ),
        "2026-07-15 18:00:00 MDT"
    );
    assert_eq!(
        abbreviation_display(
            &ts(1795464000, 0).to_zoned_datetime(TimeZone::get("America/Denver").unwrap())
        ),
        "2026-11-23 13:00:00 MST"
    );
    assert_eq!(
        abbreviation_display(
            &ts(1784160000, 0).to_zoned_datetime(TimeZone::get("Asia/Kolkata").unwrap())
        ),
        "2026-07-16 05:30:00 IST"
    );
    assert_eq!(
        abbreviation_display(
            &ts(1795464000, 0).to_zoned_datetime(TimeZone::get("Asia/Kolkata").unwrap())
        ),
        "2026-11-24 01:30:00 IST"
    );
    assert_eq!(
        abbreviation_display(
            &ts(1784160000, 0).to_zoned_datetime(TimeZone::get("Europe/London").unwrap())
        ),
        "2026-07-16 01:00:00 BST"
    );
    assert_eq!(
        abbreviation_display(
            &ts(1795464000, 0).to_zoned_datetime(TimeZone::get("Europe/London").unwrap())
        ),
        "2026-11-23 20:00:00 GMT"
    );
    assert_eq!(
        abbreviation_display(
            &ts(1784160000, 0).to_zoned_datetime(TimeZone::get("Australia/Lord_Howe").unwrap())
        ),
        "2026-07-16 10:30:00 +1030"
    );
    assert_eq!(
        abbreviation_display(
            &ts(1795464000, 0).to_zoned_datetime(TimeZone::get("Australia/Lord_Howe").unwrap())
        ),
        "2026-11-24 07:00:00 +11"
    );
}

#[test]
fn local_gaps_and_folds() {
    let show =
        |z: Option<jiff::ZonedDateTime>| z.map(|z| zoned_rfc3339(&z, Precision::AutoSi, false));
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 3, 8).at(2, 30, 0, 0)
            )
            .single()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 3, 8).at(2, 30, 0, 0)
            )
            .earliest()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 3, 8).at(2, 30, 0, 0)
            )
            .latest()
        ),
        None
    );
    let show =
        |z: Option<jiff::ZonedDateTime>| z.map(|z| zoned_rfc3339(&z, Precision::AutoSi, false));
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 11, 1).at(1, 30, 0, 0)
            )
            .single()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 11, 1).at(1, 30, 0, 0)
            )
            .earliest()
        ),
        Some("2026-11-01T01:30:00-06:00".to_owned())
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 11, 1).at(1, 30, 0, 0)
            )
            .latest()
        ),
        Some("2026-11-01T01:30:00-07:00".to_owned())
    );
    let show =
        |z: Option<jiff::ZonedDateTime>| z.map(|z| zoned_rfc3339(&z, Precision::AutoSi, false));
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 7, 1).at(12, 0, 0, 0)
            )
            .single()
        ),
        Some("2026-07-01T12:00:00-06:00".to_owned())
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 7, 1).at(12, 0, 0, 0)
            )
            .earliest()
        ),
        Some("2026-07-01T12:00:00-06:00".to_owned())
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("America/Denver").unwrap(),
                date(2026, 7, 1).at(12, 0, 0, 0)
            )
            .latest()
        ),
        Some("2026-07-01T12:00:00-06:00".to_owned())
    );
    let show =
        |z: Option<jiff::ZonedDateTime>| z.map(|z| zoned_rfc3339(&z, Precision::AutoSi, false));
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Europe/London").unwrap(),
                date(2026, 3, 29).at(1, 15, 0, 0)
            )
            .single()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Europe/London").unwrap(),
                date(2026, 3, 29).at(1, 15, 0, 0)
            )
            .earliest()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Europe/London").unwrap(),
                date(2026, 3, 29).at(1, 15, 0, 0)
            )
            .latest()
        ),
        None
    );
    let show =
        |z: Option<jiff::ZonedDateTime>| z.map(|z| zoned_rfc3339(&z, Precision::AutoSi, false));
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Europe/London").unwrap(),
                date(2026, 10, 25).at(1, 15, 0, 0)
            )
            .single()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Europe/London").unwrap(),
                date(2026, 10, 25).at(1, 15, 0, 0)
            )
            .earliest()
        ),
        Some("2026-10-25T01:15:00+01:00".to_owned())
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Europe/London").unwrap(),
                date(2026, 10, 25).at(1, 15, 0, 0)
            )
            .latest()
        ),
        Some("2026-10-25T01:15:00+00:00".to_owned())
    );
    let show =
        |z: Option<jiff::ZonedDateTime>| z.map(|z| zoned_rfc3339(&z, Precision::AutoSi, false));
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Australia/Lord_Howe").unwrap(),
                date(2026, 4, 5).at(1, 45, 0, 0)
            )
            .single()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Australia/Lord_Howe").unwrap(),
                date(2026, 4, 5).at(1, 45, 0, 0)
            )
            .earliest()
        ),
        Some("2026-04-05T01:45:00+11:00".to_owned())
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Australia/Lord_Howe").unwrap(),
                date(2026, 4, 5).at(1, 45, 0, 0)
            )
            .latest()
        ),
        Some("2026-04-05T01:45:00+10:30".to_owned())
    );
    let show =
        |z: Option<jiff::ZonedDateTime>| z.map(|z| zoned_rfc3339(&z, Precision::AutoSi, false));
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Australia/Lord_Howe").unwrap(),
                date(2026, 10, 4).at(2, 15, 0, 0)
            )
            .single()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Australia/Lord_Howe").unwrap(),
                date(2026, 10, 4).at(2, 15, 0, 0)
            )
            .earliest()
        ),
        None
    );
    assert_eq!(
        show(
            from_local(
                &TimeZone::get("Australia/Lord_Howe").unwrap(),
                date(2026, 10, 4).at(2, 15, 0, 0)
            )
            .latest()
        ),
        None
    );
}

#[test]
fn parsing_accepts_what_the_journal_always_accepted() {
    assert_eq!(
        parse_rfc3339("2026-11-17T20:00:00Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794945600000000000, 0))
    );
    assert_eq!(
        parse_datetime("2026-11-17T20:00:00Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794945600000000000, 0))
    );
    assert_eq!(
        parse_rfc3339("2026-11-17t20:00:00.120z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794945600120000000, 0))
    );
    assert_eq!(
        parse_datetime("2026-11-17t20:00:00.120z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794945600120000000, 0))
    );
    assert_eq!(
        parse_rfc3339("2026-11-17 20:00:00+05:30")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794925800000000000, 19800))
    );
    assert_eq!(
        parse_datetime("2026-11-17 20:00:00+05:30")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794925800000000000, 19800))
    );
    assert_eq!(
        parse_rfc3339("2026-11-17T20:00:00+0530")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime("2026-11-17T20:00:00+0530")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794925800000000000, 19800))
    );
    assert_eq!(
        parse_rfc3339(" 2026-11-17T20:00:00Z ")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime(" 2026-11-17T20:00:00Z ")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794945600000000000, 0))
    );
    assert_eq!(
        parse_rfc3339("2026-1-7T2:00:00Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime("2026-1-7T2:00:00Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1767751200000000000, 0))
    );
    assert_eq!(
        parse_rfc3339("2026-11-17T20:00:00.1234567891-07:00")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794970800123456789, -25200))
    );
    assert_eq!(
        parse_datetime("2026-11-17T20:00:00.1234567891-07:00")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        Some((1794970800123456789, -25200))
    );
    assert_eq!(
        parse_rfc3339("2026-11-17T20:00Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime("2026-11-17T20:00Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_rfc3339("2026-11-17T20:00:00")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime("2026-11-17T20:00:00")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_rfc3339("2026-11-17T20:00:00+24:00")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime("2026-11-17T20:00:00+24:00")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_rfc3339("20261117T200000Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime("20261117T200000Z")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_rfc3339("2026-11-17T20:00:00Z[UTC]")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_datetime("2026-11-17T20:00:00Z[UTC]")
            .ok()
            .map(|z| (z.timestamp().as_nanosecond(), z.offset().seconds())),
        None
    );
    assert_eq!(
        parse_civil_datetime("2026-11-17T20:00:00")
            .ok()
            .map(civil_iso)
            .as_deref(),
        Some("2026-11-17T20:00:00")
    );
    assert_eq!(
        parse_civil_datetime("2026-11-17T20:00:00.120")
            .ok()
            .map(civil_iso)
            .as_deref(),
        Some("2026-11-17T20:00:00.120")
    );
    assert_eq!(
        parse_civil_datetime(" 2026-1-7T2:00:00 ")
            .ok()
            .map(civil_iso)
            .as_deref(),
        Some("2026-01-07T02:00:00")
    );
    assert_eq!(
        parse_civil_datetime("2026-11-17 20:00:00")
            .ok()
            .map(civil_iso)
            .as_deref(),
        None
    );
    assert_eq!(
        parse_civil_datetime("2026-11-17T20:00")
            .ok()
            .map(civil_iso)
            .as_deref(),
        None
    );
    assert_eq!(
        strptime_fixed("%Y-%m-%d %H:%M:%S %z", "2026-11-17 20:00:00 +0530")
            .ok()
            .map(|z| (z.timestamp().as_second(), z.offset().seconds())),
        Some((1794925800, 19800))
    );
    assert_eq!(
        strptime_fixed("%Y-%m-%d %H:%M:%S %z", "2026-11-17 20:00:00 +05:30")
            .ok()
            .map(|z| (z.timestamp().as_second(), z.offset().seconds())),
        Some((1794925800, 19800))
    );
    assert_eq!(
        strptime_fixed("%Y-%m-%d %H:%M:%S %z", "2026-11-17 20:00:00")
            .ok()
            .map(|z| (z.timestamp().as_second(), z.offset().seconds())),
        None
    );
}

#[test]
fn day_keys_are_eight_digits_naming_a_real_date() {
    assert_eq!(parse_day("20261117"), Some(date(2026, 11, 17)));
    assert_eq!(parse_day("20240229"), Some(date(2024, 2, 29)));
    for refused in [
        "20250229",
        "20261317",
        "20261100",
        "2026111",
        "202611170",
        "+20261117",
        " 20261117",
        "2026 117",
        "",
    ] {
        assert_eq!(parse_day(refused), None, "{refused:?}");
    }
    assert_eq!(day_key(date(2026, 1, 7)), "20260107");
    assert_eq!(day_key(date(-1, 12, 26)), "-00011226");
    assert_eq!(
        civil_iso(date(-1, 12, 26).at(1, 2, 3, 0)),
        "-0001-12-26T01:02:03"
    );
}

#[test]
fn serde_keeps_the_stored_forms() {
    #[derive(::serde::Serialize, ::serde::Deserialize, Debug, PartialEq)]
    struct Record {
        #[serde(with = "crate::serde::timestamp")]
        at: Timestamp,
        #[serde(with = "crate::serde::timestamp::option", default)]
        seen: Option<Timestamp>,
        #[serde(with = "crate::serde::zoned")]
        local: jiff::ZonedDateTime,
        #[serde(with = "crate::serde::civil")]
        wall: jiff::civil::DateTime,
    }
    let record = Record {
        at: ts(1_795_464_000, 120_000_000),
        seen: None,
        local: at_offset(ts(1_795_464_000, 0), off(19_800)),
        wall: date(2026, 11, 17).at(20, 0, 0, 120_000_000),
    };
    let text = serde_json::to_string(&record).unwrap();
    assert_eq!(
        text,
        r#"{"at":"2026-11-23T20:00:00.120Z","seen":null,"local":"2026-11-24T01:30:00+05:30","wall":"2026-11-17T20:00:00.120"}"#
    );
    assert_eq!(serde_json::from_str::<Record>(&text).unwrap(), record);
    let older = r#"{"at":"2026-11-23T20:00:00.12+00:00","local":"2026-11-24T01:30:00+0530","wall":"2026-11-17T20:00:00.12"}"#;
    assert_eq!(serde_json::from_str::<Record>(older).unwrap(), record);
}

#[test]
fn zone_names_are_exact_and_older_aliases_still_resolve() {
    let name = |text: &str| zone(text).and_then(|zone| zone.iana_name().map(str::to_owned));
    for text in [
        "America/Denver",
        "UTC",
        "Etc/GMT-9",
        "Asia/Calcutta",
        "Europe/Kiev",
        "US/Mountain",
    ] {
        assert_eq!(name(text).as_deref(), Some(text), "{text}");
    }
    for text in [
        "america/denver",
        "",
        "Mars/Olympus_Mons",
        "../etc",
        "/etc/localtime",
    ] {
        assert!(zone(text).is_none(), "{text}");
    }
    let calcutta = zone("Asia/Calcutta").unwrap();
    assert_eq!(calcutta.to_offset(ts(1_795_464_000, 0)).seconds(), 19_800);
}

#[test]
fn a_unicode_minus_reads_as_minus() {
    let at = |text: &str| {
        parse_rfc3339(text)
            .ok()
            .map(|z| (z.timestamp().as_second(), z.offset().seconds()))
    };
    assert_eq!(
        at("2026-11-17T20:00:00\u{2212}07:00"),
        Some((1_794_970_800, -25_200))
    );
    assert_eq!(
        parse_datetime("2026-11-17 20:00:00\u{2212}0700")
            .ok()
            .map(|z| z.timestamp().as_second()),
        Some(1_794_970_800)
    );
    let fixed = |text: &str| {
        strptime_fixed("%Y-%m-%d %H:%M:%S %z", text)
            .ok()
            .map(|z| z.offset().seconds())
    };
    assert_eq!(fixed("2026-11-17 20:00:00 \u{2212}07 : 00"), Some(-25_200));
    assert_eq!(fixed("2026-11-17 20:00:00 +24:00"), None);
    assert_eq!(fixed("2026-11-17 20:00:00 -07:00:00"), None);
}
