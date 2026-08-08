// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::calendar::{days_in_month, is_valid_year_month, parse_ascii_digits};
use crate::{BodyCalendarError, BodyCalendarField, BodyMonth, BodyString};

const BODY_DAY_LENGTH: usize = 8;

/// A validated native body calendar day.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyDay(Box<str>);

impl BodyDay {
    /// Builds a calendar day from its exact ASCII wire spelling.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BodyCalendarError> {
        if bytes.len() != BODY_DAY_LENGTH {
            return Err(invalid_format());
        }
        let Some(year) = parse_ascii_digits(&bytes[..4]) else {
            return Err(invalid_format());
        };
        let Some(month) = parse_ascii_digits(&bytes[4..6]) else {
            return Err(invalid_format());
        };
        let Some(day) = parse_ascii_digits(&bytes[6..]) else {
            return Err(invalid_format());
        };
        let month = u8::try_from(month).map_err(|_| invalid_format())?;
        let day = u8::try_from(day).map_err(|_| invalid_format())?;
        if !is_valid_year_month(year, month) || !(1..=days_in_month(year, month)).contains(&day) {
            return Err(invalid_format());
        }
        let value = std::str::from_utf8(bytes).map_err(|_| invalid_format())?;
        Ok(Self(value.into()))
    }

    /// Builds a calendar day from a decoded body string.
    pub fn from_body_string(value: &BodyString) -> Result<Self, BodyCalendarError> {
        if value.code_points().len() != BODY_DAY_LENGTH {
            return Err(invalid_format());
        }
        let mut bytes = Vec::with_capacity(BODY_DAY_LENGTH);
        for code_point in value.code_points() {
            if *code_point > 0x7f {
                return Err(invalid_format());
            }
            bytes.push(*code_point as u8);
        }
        Self::from_bytes(&bytes)
    }

    /// Returns the exact validated wire spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns this calendar day as a decoded body string.
    pub fn to_body_string(&self) -> BodyString {
        BodyString::from_code_points(self.0.bytes().map(u32::from).collect())
            .expect("validated BodyDay contains only ASCII code points")
    }

    /// Returns the calendar month containing this day.
    pub fn month(&self) -> BodyMonth {
        let bytes = self.0.as_bytes();
        let month = [
            bytes[0], bytes[1], bytes[2], bytes[3], b'-', bytes[4], bytes[5],
        ];
        BodyMonth::from_bytes(&month).expect("validated BodyDay contains a valid BodyMonth")
    }
}

impl TryFrom<&[u8]> for BodyDay {
    type Error = BodyCalendarError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes)
    }
}

impl TryFrom<&BodyString> for BodyDay {
    type Error = BodyCalendarError;

    fn try_from(value: &BodyString) -> Result<Self, Self::Error> {
        Self::from_body_string(value)
    }
}

fn invalid_format() -> BodyCalendarError {
    BodyCalendarError::InvalidFormat(BodyCalendarField::Day)
}
