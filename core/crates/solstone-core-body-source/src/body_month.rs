// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::calendar::{is_valid_year_month, parse_ascii_digits};
use crate::{BodyCalendarError, BodyCalendarField, BodyString};

const BODY_MONTH_LENGTH: usize = 7;

/// A validated native body calendar month.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyMonth(Box<str>);

impl BodyMonth {
    /// Builds a calendar month from its exact ASCII wire spelling.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BodyCalendarError> {
        if bytes.len() != BODY_MONTH_LENGTH || bytes[4] != b'-' {
            return Err(invalid_format());
        }
        let Some(year) = parse_ascii_digits(&bytes[..4]) else {
            return Err(invalid_format());
        };
        let Some(month) = parse_ascii_digits(&bytes[5..]) else {
            return Err(invalid_format());
        };
        let month = u8::try_from(month).map_err(|_| invalid_format())?;
        if !is_valid_year_month(year, month) {
            return Err(invalid_format());
        }
        let value = std::str::from_utf8(bytes).map_err(|_| invalid_format())?;
        Ok(Self(value.into()))
    }

    /// Builds a calendar month from a decoded body string.
    pub fn from_body_string(value: &BodyString) -> Result<Self, BodyCalendarError> {
        if value.code_points().len() != BODY_MONTH_LENGTH
            || value.code_points()[4] != u32::from(b'-')
        {
            return Err(invalid_format());
        }
        let mut bytes = Vec::with_capacity(BODY_MONTH_LENGTH);
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

    /// Returns this calendar month as a decoded body string.
    pub fn to_body_string(&self) -> BodyString {
        BodyString::from_code_points(self.0.bytes().map(u32::from).collect())
            .expect("validated BodyMonth contains only ASCII code points")
    }
}

impl TryFrom<&[u8]> for BodyMonth {
    type Error = BodyCalendarError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes)
    }
}

impl TryFrom<&BodyString> for BodyMonth {
    type Error = BodyCalendarError;

    fn try_from(value: &BodyString) -> Result<Self, Self::Error> {
        Self::from_body_string(value)
    }
}

fn invalid_format() -> BodyCalendarError {
    BodyCalendarError::InvalidFormat(BodyCalendarField::Month)
}
