// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Timestamp values and `--auto` state used by import resolution.

use std::fmt;

use regex::Regex;

/// A timestamp which has passed the import timestamp validators.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Timestamp(String);

impl Timestamp {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Calendar-day half of the stamp (`YYYYMMDD`).
    #[must_use]
    pub fn day(&self) -> &str {
        &self.0[..8]
    }

    /// Time-of-day half in the `HH:MM:SS` form transcript import expects.
    ///
    /// The stamp itself stays `YYYYMMDD_HHMMSS`. Segment and entry clocks are a
    /// different contract; callers convert here rather than teaching the
    /// transcript parser a second format.
    #[must_use]
    pub fn clock(&self) -> String {
        format!("{}:{}:{}", &self.0[9..11], &self.0[11..13], &self.0[13..15])
    }
}

/// A deterministic or model detector's timestamp answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectedTimestamp {
    pub timestamp: Timestamp,
}

impl DetectedTimestamp {
    #[must_use]
    pub fn new(timestamp: Timestamp) -> Self {
        Self { timestamp }
    }
}

/// A non-empty model guidance value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NonEmptyGuidance(String);

impl NonEmptyGuidance {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The four states of the CLI's `--auto` option.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutoTimestamp {
    Absent,
    Bare,
    Guidance(NonEmptyGuidance),
    EmptyGuidance,
}

impl AutoTimestamp {
    /// Converts absent, bare, textual, and empty textual CLI states.
    #[must_use]
    pub fn from_raw(raw: Option<Option<&str>>) -> Self {
        match raw {
            None => Self::Absent,
            Some(None) => Self::Bare,
            Some(Some("")) => Self::EmptyGuidance,
            Some(Some(value)) => Self::Guidance(NonEmptyGuidance(value.to_owned())),
        }
    }

    #[must_use]
    pub const fn adopts(&self) -> bool {
        matches!(self, Self::Bare | Self::Guidance(_))
    }

    #[must_use]
    pub fn guidance(&self) -> Option<&str> {
        match self {
            Self::Absent | Self::Bare => None,
            Self::Guidance(value) => Some(value.as_str()),
            Self::EmptyGuidance => Some(""),
        }
    }
}

/// The validator which rejected a timestamp.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimestampError {
    Shape,
    Calendar { message: String },
}

impl fmt::Display for TimestampError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shape => formatter.write_str("timestamp must be YYYYMMDD_HHMMSS format"),
            Self::Calendar { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for TimestampError {}

/// Validates the same shape and calendar rules as the Python resolver.
pub fn validate_timestamp(raw: &str) -> Result<Timestamp, TimestampError> {
    let shape = Regex::new(r"^\d{8}_\d{6}$").expect("literal timestamp regex is valid");
    if !shape.is_match(raw) {
        return Err(TimestampError::Shape);
    }

    let format_message = || TimestampError::Calendar {
        message: format!("time data '{raw}' does not match format '%Y%m%d_%H%M%S'"),
    };
    if !raw.is_ascii() {
        return Err(format_message());
    }
    let parse = |range: std::ops::Range<usize>| raw[range].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        parse(0..4),
        parse(4..6),
        parse(6..8),
        parse(9..11),
        parse(11..13),
        parse(13..15),
    ) else {
        return Err(format_message());
    };
    if year == 0 || month == 0 || month > 12 || hour > 23 || minute > 59 || second > 59 {
        return Err(format_message());
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => unreachable!(),
    };
    if day == 0 || day > days {
        return Err(TimestampError::Calendar {
            message: "day is out of range for month".to_owned(),
        });
    }
    Ok(Timestamp(raw.to_owned()))
}
