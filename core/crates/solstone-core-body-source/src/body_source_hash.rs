// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Ordering;

use crate::{BodyDay, BodySourceFamily, BodySourceHashError, BodyString};

const HASH_LENGTH: usize = 64;
const WINDOW_PREFIX: &[u8] = b"#window:";

/// A validated native body-source hash bound to its source family.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BodySourceHash {
    family: BodySourceFamily,
    spelling: Box<str>,
}

impl BodySourceHash {
    /// Builds a source hash from its exact ASCII wire spelling for a source family.
    pub fn from_bytes_for_family(
        bytes: &[u8],
        family: &BodySourceFamily,
    ) -> Result<Self, BodySourceHashError> {
        if bytes.len() < HASH_LENGTH || !is_valid_hash(&bytes[..HASH_LENGTH]) {
            return Err(invalid_format());
        }

        let suffix = &bytes[HASH_LENGTH..];
        if suffix.is_empty() {
            return Self::from_validated_bytes(bytes, family);
        }
        if !matches!(family, BodySourceFamily::AppleHealth) || !suffix.starts_with(WINDOW_PREFIX) {
            return Err(invalid_format());
        }

        let (start, end) = parse_window_bounds(&suffix[WINDOW_PREFIX.len()..])?;
        if matches!((&start, &end), (WindowBound::Open, WindowBound::Open)) {
            return Err(invalid_format());
        }
        if let (WindowBound::Day(start), WindowBound::Day(end)) = (&start, &end)
            && start > end
        {
            return Err(invalid_format());
        }

        Self::from_validated_bytes(bytes, family)
    }

    /// Builds a source hash from a decoded body string for a source family.
    pub fn from_body_string_for_family(
        text: &BodyString,
        family: &BodySourceFamily,
    ) -> Result<Self, BodySourceHashError> {
        let mut bytes = Vec::with_capacity(text.code_points().len());
        for code_point in text.code_points() {
            if *code_point > 0x7f {
                return Err(invalid_format());
            }
            bytes.push(*code_point as u8);
        }
        Self::from_bytes_for_family(&bytes, family)
    }

    /// Returns the exact validated wire spelling.
    pub fn as_str(&self) -> &str {
        &self.spelling
    }

    /// Returns this source hash as a decoded body string.
    pub fn to_body_string(&self) -> BodyString {
        BodyString::from_code_points(self.spelling.bytes().map(u32::from).collect())
            .expect("validated BodySourceHash contains only ASCII code points")
    }

    /// Returns the source family this hash is bound to.
    pub fn family(&self) -> BodySourceFamily {
        self.family
    }

    /// Returns whether this source identity's validated selection window includes a day.
    ///
    /// Plain hashes have no day bound. Apple window bounds are inclusive, and an open
    /// side is unbounded.
    pub fn includes_day(&self, day: &BodyDay) -> bool {
        let suffix = &self.spelling.as_bytes()[HASH_LENGTH..];
        if suffix.is_empty() {
            return true;
        }

        let window = suffix
            .strip_prefix(WINDOW_PREFIX)
            .expect("validated windowed BodySourceHash has the window prefix");
        let (start, end) =
            parse_window_bounds(window).expect("validated BodySourceHash has valid window bounds");
        let starts_before_or_on = match start {
            WindowBound::Open => true,
            WindowBound::Day(start) => start <= *day,
        };
        let ends_after_or_on = match end {
            WindowBound::Open => true,
            WindowBound::Day(end) => *day <= end,
        };
        starts_before_or_on && ends_after_or_on
    }

    fn from_validated_bytes(
        bytes: &[u8],
        family: &BodySourceFamily,
    ) -> Result<Self, BodySourceHashError> {
        let spelling = std::str::from_utf8(bytes).map_err(|_| invalid_format())?;
        Ok(Self {
            family: *family,
            spelling: spelling.into(),
        })
    }
}

impl Ord for BodySourceHash {
    fn cmp(&self, other: &Self) -> Ordering {
        self.family
            .as_str()
            .cmp(other.family.as_str())
            .then_with(|| self.spelling.as_bytes().cmp(other.spelling.as_bytes()))
    }
}

impl PartialOrd for BodySourceHash {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

enum WindowBound {
    Open,
    Day(BodyDay),
}

fn is_valid_hash(bytes: &[u8]) -> bool {
    bytes.len() == HASH_LENGTH
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn parse_window_bounds(bytes: &[u8]) -> Result<(WindowBound, WindowBound), BodySourceHashError> {
    let mut bounds = bytes.split(|byte| *byte == b':');
    let Some(start) = bounds.next() else {
        return Err(invalid_format());
    };
    let Some(end) = bounds.next() else {
        return Err(invalid_format());
    };
    if bounds.next().is_some() {
        return Err(invalid_format());
    }
    Ok((parse_window_bound(start)?, parse_window_bound(end)?))
}

fn parse_window_bound(bytes: &[u8]) -> Result<WindowBound, BodySourceHashError> {
    if bytes == b"open" {
        Ok(WindowBound::Open)
    } else {
        BodyDay::from_bytes(bytes)
            .map(WindowBound::Day)
            .map_err(|_| invalid_format())
    }
}

fn invalid_format() -> BodySourceHashError {
    BodySourceHashError::InvalidFormat
}
