// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal's timestamp text forms, on jiff.
//!
//! Every byte form the journal writes or serves, and every form it accepts,
//! has one function here. They keep the exact text the journal has always
//! written, so files, receipts and API responses read the same whatever library
//! computes the instant. jiff's own `Display`, `FromStr` and serde forms differ
//! from these in small ways (trailing fraction zeros, `Z` versus `+00:00`,
//! leniency), so code that writes or reads a stored or served timestamp goes
//! through this crate rather than through jiff's defaults.
//!
//! Local wall-clock resolution ([`from_local`]) keeps the journal's existing
//! answers in daylight-saving gaps (no time) and folds (both times).

use std::fmt::Write as _;

pub use jiff;

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone};
use jiff::{Timestamp, ZonedDateTime};

mod parse;
pub mod serde;

pub use parse::{
    ParseError, parse_civil_datetime, parse_datetime, parse_day, parse_rfc3339, parse_timestamp,
    strptime_fixed,
};

/// How many fraction digits a written timestamp carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Precision {
    /// No fraction.
    Secs,
    /// Three digits, truncated.
    Millis,
    /// Six digits, truncated.
    Micros,
    /// Nine digits.
    Nanos,
    /// The fewest of none, three, six or nine digits that hold the value
    /// exactly.
    AutoSi,
}

/// RFC 3339 text for `ts` at `offset`: `YYYY-MM-DDTHH:MM:SS[.fraction]` and
/// then `Z` (only when `use_z` and the offset is zero) or `±HH:MM`.
#[must_use]
pub fn rfc3339(ts: Timestamp, offset: Offset, precision: Precision, use_z: bool) -> String {
    let wall = offset.to_datetime(ts);
    let mut out = String::with_capacity(35);
    push_civil(&mut out, wall, 'T', precision);
    push_offset(&mut out, offset, use_z);
    out
}

/// `rfc3339` at the offset `zoned` carries.
#[must_use]
pub fn zoned_rfc3339(zoned: &ZonedDateTime, precision: Precision, use_z: bool) -> String {
    rfc3339(zoned.timestamp(), zoned.offset(), precision, use_z)
}

/// A UTC instant as `…+00:00` with the [`Precision::AutoSi`] fraction: the form
/// the journal writes for "an instant, as RFC 3339" (chrono's `to_rfc3339`).
#[must_use]
pub fn utc_rfc3339(ts: Timestamp) -> String {
    rfc3339(ts, Offset::UTC, Precision::AutoSi, false)
}

/// A UTC instant as `…Z` with the [`Precision::AutoSi`] fraction: the journal's
/// serialized-instant form.
#[must_use]
pub fn utc_iso(ts: Timestamp) -> String {
    rfc3339(ts, Offset::UTC, Precision::AutoSi, true)
}

/// A UTC instant as `…Z` at `precision`.
#[must_use]
pub fn utc_z(ts: Timestamp, precision: Precision) -> String {
    rfc3339(ts, Offset::UTC, precision, true)
}

/// A wall-clock date and time as `YYYY-MM-DDTHH:MM:SS[.fraction]`, AutoSi
/// fraction: the serialized form of a zone-less date-time.
#[must_use]
pub fn civil_iso(dt: DateTime) -> String {
    let mut out = String::with_capacity(29);
    push_civil(&mut out, dt, 'T', Precision::AutoSi);
    out
}

/// A wall-clock date and time with a space separator and AutoSi fraction:
/// `YYYY-MM-DD HH:MM:SS[.fraction]`.
#[must_use]
pub fn civil_display(dt: DateTime) -> String {
    let mut out = String::with_capacity(29);
    push_civil(&mut out, dt, ' ', Precision::AutoSi);
    out
}

/// A time of day as `HH:MM:SS[.fraction]`, AutoSi fraction.
#[must_use]
pub fn time_display(time: Time) -> String {
    let mut out = String::with_capacity(18);
    let _ = write!(
        out,
        "{:02}:{:02}:{:02}",
        time.hour(),
        time.minute(),
        time.second()
    );
    push_fraction(&mut out, time.subsec_nanosecond(), Precision::AutoSi);
    out
}

/// A UTC instant for people and logs: `YYYY-MM-DD HH:MM:SS[.fraction] UTC`.
#[must_use]
pub fn utc_display(ts: Timestamp) -> String {
    let mut out = civil_display(Offset::UTC.to_datetime(ts));
    out.push_str(" UTC");
    out
}

/// A zoned time for people and logs, with its numeric offset:
/// `YYYY-MM-DD HH:MM:SS[.fraction] ±HH:MM`.
#[must_use]
pub fn offset_display(zoned: &ZonedDateTime) -> String {
    let mut out = civil_display(zoned.datetime());
    out.push(' ');
    let seconds = zoned.offset().seconds();
    let sign = if seconds < 0 { '-' } else { '+' };
    let magnitude = seconds.unsigned_abs();
    let _ = write!(
        out,
        "{sign}{:02}:{:02}",
        magnitude / 3600,
        magnitude / 60 % 60
    );
    if !magnitude.is_multiple_of(60) {
        let _ = write!(out, ":{:02}", magnitude % 60);
    }
    out
}

/// A zoned time for people and logs, with its zone abbreviation:
/// `YYYY-MM-DD HH:MM:SS[.fraction] MST`.
#[must_use]
pub fn abbreviation_display(zoned: &ZonedDateTime) -> String {
    let mut out = civil_display(zoned.datetime());
    out.push(' ');
    out.push_str(&abbreviation(zoned));
    out
}

/// The zone abbreviation in effect at `zoned` (`MST`, `CET`, `+0530`).
#[must_use]
pub fn abbreviation(zoned: &ZonedDateTime) -> String {
    zoned
        .time_zone()
        .to_offset_info(zoned.timestamp())
        .abbreviation()
        .to_owned()
}

/// A compact day key, `YYYYMMDD`.
#[must_use]
pub fn day_key(date: Date) -> String {
    let year = date.year();
    let sign = if year < 0 { "-" } else { "" };
    format!(
        "{sign}{:04}{:02}{:02}",
        year.unsigned_abs(),
        date.month(),
        date.day()
    )
}

/// `strftime` for a zoned time, where `%.f` writes the AutoSi fraction with its
/// dot (or nothing for a whole second), as the journal's formats expect. Every
/// other directive is jiff's, which writes the same text for every directive
/// the journal uses.
#[must_use]
pub fn strftime_zoned(zoned: &ZonedDateTime, format: &str) -> String {
    let format = expand_auto_fraction(format, zoned.time().subsec_nanosecond());
    zoned.strftime(&format).to_string()
}

/// [`strftime_zoned`] for a wall-clock date and time.
#[must_use]
pub fn strftime_civil(dt: DateTime, format: &str) -> String {
    let format = expand_auto_fraction(format, dt.subsec_nanosecond());
    dt.strftime(&format).to_string()
}

/// A wall-clock time resolved in a zone, with every answer a daylight-saving
/// change allows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalResult {
    /// Exactly one instant has this wall-clock time.
    Single(ZonedDateTime),
    /// The clocks were set back: the earlier and the later instant.
    Ambiguous(ZonedDateTime, ZonedDateTime),
    /// The clocks skipped this wall-clock time.
    None,
}

impl LocalResult {
    /// The instant when there is exactly one.
    #[must_use]
    pub fn single(self) -> Option<ZonedDateTime> {
        match self {
            Self::Single(zoned) => Some(zoned),
            _ => None,
        }
    }

    /// The only or earlier instant; none in a gap.
    #[must_use]
    pub fn earliest(self) -> Option<ZonedDateTime> {
        match self {
            Self::Single(zoned) | Self::Ambiguous(zoned, _) => Some(zoned),
            Self::None => None,
        }
    }

    /// The only or later instant; none in a gap.
    #[must_use]
    pub fn latest(self) -> Option<ZonedDateTime> {
        match self {
            Self::Single(zoned) | Self::Ambiguous(_, zoned) => Some(zoned),
            Self::None => None,
        }
    }
}

/// Resolve the wall-clock `dt` in `zone`.
#[must_use]
pub fn from_local(zone: &TimeZone, dt: DateTime) -> LocalResult {
    let at = |offset: Offset| {
        offset
            .to_timestamp(dt)
            .ok()
            .map(|ts| ts.to_zoned_datetime(zone.clone()))
    };
    match zone.to_ambiguous_zoned_datetime(dt).offset() {
        AmbiguousOffset::Unambiguous { offset } => {
            at(offset).map_or(LocalResult::None, LocalResult::Single)
        }
        AmbiguousOffset::Gap { .. } => LocalResult::None,
        AmbiguousOffset::Fold { before, after } => match (at(before), at(after)) {
            (Some(earlier), Some(later)) => LocalResult::Ambiguous(earlier, later),
            (Some(only), None) | (None, Some(only)) => LocalResult::Single(only),
            (None, None) => LocalResult::None,
        },
    }
}

/// The IANA zone `name` names, exactly as the time zone database writes it
/// (`America/Denver`, not `america/denver`).
///
/// The operating system's database answers first, so the journal keeps the
/// same clock as the computer it runs on. A name the system lacks, such as an
/// older alias (`Asia/Calcutta`, `Europe/Kiev`, `US/Mountain`) that some
/// systems no longer install but browsers still report, comes from the bundled
/// copy.
#[must_use]
pub fn zone(name: &str) -> Option<TimeZone> {
    let exact = |zone: TimeZone| (zone.iana_name() == Some(name)).then_some(zone);
    TimeZone::get(name).ok().and_then(exact).or_else(|| {
        jiff::tz::TimeZoneDatabase::bundled()
            .get(name)
            .ok()
            .and_then(exact)
    })
}

/// A fixed-offset zoned time: the instant `ts` read at `offset`.
#[must_use]
pub fn at_offset(ts: Timestamp, offset: Offset) -> ZonedDateTime {
    ts.to_zoned_datetime(TimeZone::fixed(offset))
}

fn push_civil(out: &mut String, dt: DateTime, separator: char, precision: Precision) {
    let year = dt.year();
    if year < 0 {
        out.push('-');
    }
    let _ = write!(
        out,
        "{:04}-{:02}-{:02}{separator}{:02}:{:02}:{:02}",
        year.unsigned_abs(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    );
    push_fraction(out, dt.subsec_nanosecond(), precision);
}

fn push_fraction(out: &mut String, nanos: i32, precision: Precision) {
    let nanos = nanos.max(0);
    match precision {
        Precision::Secs => {}
        Precision::Millis => {
            let _ = write!(out, ".{:03}", nanos / 1_000_000);
        }
        Precision::Micros => {
            let _ = write!(out, ".{:06}", nanos / 1_000);
        }
        Precision::Nanos => {
            let _ = write!(out, ".{nanos:09}");
        }
        Precision::AutoSi => {
            if nanos == 0 {
            } else if nanos % 1_000_000 == 0 {
                let _ = write!(out, ".{:03}", nanos / 1_000_000);
            } else if nanos % 1_000 == 0 {
                let _ = write!(out, ".{:06}", nanos / 1_000);
            } else {
                let _ = write!(out, ".{nanos:09}");
            }
        }
    }
}

fn push_offset(out: &mut String, offset: Offset, use_z: bool) {
    let seconds = offset.seconds();
    if use_z && seconds == 0 {
        out.push('Z');
        return;
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let magnitude = seconds.unsigned_abs();
    // Whole minutes, as RFC 3339 writes them; a seconds part rounds to the
    // nearest minute.
    let minutes = (magnitude + 30) / 60;
    let _ = write!(out, "{sign}{:02}:{:02}", minutes / 60, minutes % 60);
}

fn expand_auto_fraction(format: &str, nanos: i32) -> String {
    if !format.contains("%.f") {
        return format.to_owned();
    }
    let mut fraction = String::new();
    push_fraction(&mut fraction, nanos, Precision::AutoSi);
    let mut out = String::with_capacity(format.len() + fraction.len());
    let mut rest = format;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(after) = tail.strip_prefix("%%") {
            out.push_str("%%");
            rest = after;
        } else if let Some(after) = tail.strip_prefix("%.f") {
            out.push_str(&fraction);
            rest = after;
        } else {
            out.push('%');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
