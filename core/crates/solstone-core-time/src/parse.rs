// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading the timestamp forms the journal accepts, with the same acceptance
//! the journal has always had: no more lenient (a validation that refused a
//! form still refuses it) and no stricter (anything already stored still reads).

use std::fmt;

use jiff::civil::{Date, DateTime, Time};
use jiff::fmt::strtime;
use jiff::tz::Offset;
use jiff::{Timestamp, ZonedDateTime};

use crate::at_offset;

/// A timestamp text the journal does not accept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    reason: &'static str,
}

impl ParseError {
    const fn new(reason: &'static str) -> Self {
        Self { reason }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for ParseError {}

/// Strict RFC 3339: `YYYY-MM-DD`, then `T`, `t` or a space, then
/// `HH:MM:SS[.fraction]`, then `Z`, `z` or `±HH:MM`. Nothing before or after.
///
/// # Errors
/// When `text` is not that form or names no real date and time.
pub fn parse_rfc3339(text: &str) -> Result<ZonedDateTime, ParseError> {
    parse_instant(text, Grammar::RFC3339)
}

/// An instant with its offset as the journal has always read stored and
/// received timestamps: [`parse_rfc3339`], plus surrounding whitespace, a
/// `±HHMM` offset and a signed year.
///
/// # Errors
/// When `text` is not one of those forms or names no real date and time.
pub fn parse_datetime(text: &str) -> Result<ZonedDateTime, ParseError> {
    parse_instant(text, Grammar::LENIENT)
}

/// A zone-less date and time: `YYYY-MM-DDTHH:MM:SS[.fraction]`, read as the
/// journal has always read stored ones (surrounding whitespace, a signed year,
/// one-digit fields and whitespace between fields).
///
/// # Errors
/// When `text` is not that form or names no real date and time.
pub fn parse_civil_datetime(text: &str) -> Result<DateTime, ParseError> {
    let mut cursor = Cursor::new(text.trim());
    cursor.loose = true;
    let date = cursor.date(true)?;
    cursor.skip_space();
    if !cursor.eat(b'T') {
        return Err(ParseError::new(
            "expected `T` between the date and the time",
        ));
    }
    let (time, _) = cursor.time()?;
    cursor.end()?;
    Ok(date.to_datetime(time))
}

/// A compact day key: exactly eight ASCII digits naming a real date
/// (`YYYYMMDD`). Anything else, a sign or whitespace included, is `None`.
#[must_use]
pub fn parse_day(text: &str) -> Option<Date> {
    let bytes = text.as_bytes();
    if bytes.len() != 8 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| {
        bytes[range]
            .iter()
            .fold(0_i32, |value, digit| value * 10 + i32::from(digit - b'0'))
    };
    let year = i16::try_from(number(0..4)).ok()?;
    let month = i8::try_from(number(4..6)).ok()?;
    let day = i8::try_from(number(6..8)).ok()?;
    Date::new(year, month, day).ok()
}

/// `strptime` for a format that ends with a numeric offset (`%z` or `%:z`),
/// reading the offset as the journal always has: optional whitespace, `+`,
/// `-` or `\u{2212}`, two hour digits, any run of colons or whitespace, two
/// minute digits, and nothing after. An offset of 24 hours or more is refused.
///
/// # Errors
/// When `text` does not match `format`, or carries no such offset.
pub fn strptime_fixed(format: &str, text: &str) -> Result<ZonedDateTime, ParseError> {
    let refused = ParseError::new("text does not match the format");
    let prefix = format
        .strip_suffix("%:z")
        .or_else(|| format.strip_suffix("%z"))
        .ok_or(refused.clone())?;
    // The prefix's own trailing whitespace is optional before the offset.
    let prefix = prefix.trim_end();
    let (parsed, used) =
        strtime::BrokenDownTime::parse_prefix(prefix, text).map_err(|_| refused.clone())?;
    let offset = offset_suffix(text.get(used..).ok_or(refused.clone())?).ok_or(refused.clone())?;
    let wall = parsed.to_datetime().map_err(|_| refused.clone())?;
    let ts = offset.to_timestamp(wall).map_err(|_| refused)?;
    Ok(at_offset(ts, offset))
}

fn offset_suffix(text: &str) -> Option<Offset> {
    let text = text.trim_start();
    let (negative, rest) = if let Some(rest) = text.strip_prefix('+') {
        (false, rest)
    } else if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else {
        (true, text.strip_prefix('\u{2212}')?)
    };
    let two_digits = |text: &str| -> Option<i32> {
        match text.as_bytes() {
            [tens @ b'0'..=b'9', ones @ b'0'..=b'9', ..] => {
                Some(i32::from(tens - b'0') * 10 + i32::from(ones - b'0'))
            }
            _ => None,
        }
    };
    let hours = two_digits(rest)?;
    let rest = rest[2..].trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let minutes = two_digits(rest)?;
    if rest.len() != 2 || minutes > 59 {
        return None;
    }
    let seconds = hours * 3600 + minutes * 60;
    if seconds >= 86_400 {
        return None;
    }
    Offset::from_seconds(if negative { -seconds } else { seconds }).ok()
}

#[derive(Clone, Copy)]
struct Grammar {
    trim: bool,
    compact_offset: bool,
    signed_year: bool,
    /// Fields of one or two digits (a year of one or more), with whitespace
    /// allowed between fields.
    loose: bool,
}

impl Grammar {
    const RFC3339: Self = Self {
        trim: false,
        compact_offset: false,
        signed_year: false,
        loose: false,
    };
    const LENIENT: Self = Self {
        trim: true,
        compact_offset: true,
        signed_year: true,
        loose: true,
    };
}

fn parse_instant(text: &str, grammar: Grammar) -> Result<ZonedDateTime, ParseError> {
    let text = if grammar.trim { text.trim() } else { text };
    let mut cursor = Cursor::new(text);
    cursor.loose = grammar.loose;
    let date = cursor.date(grammar.signed_year)?;
    let spaced = cursor.skip_space();
    match cursor.peek() {
        Some(b'T' | b't') => cursor.at += 1,
        Some(b' ') => cursor.at += 1,
        _ if spaced => {}
        _ => {
            return Err(ParseError::new(
                "expected `T` between the date and the time",
            ));
        }
    }
    let (time, leap) = cursor.time()?;
    cursor.skip_space();
    let offset = cursor.offset(grammar.compact_offset)?;
    cursor.end()?;
    let ts = offset
        .to_timestamp(date.to_datetime(time))
        .map_err(|_| ParseError::new("instant out of range"))?;
    // A leap second (`:60`) reads as the instant one second after `:59`.
    let ts = if leap {
        ts.checked_add(jiff::SignedDuration::from_secs(1))
            .map_err(|_| ParseError::new("instant out of range"))?
    } else {
        ts
    };
    Ok(at_offset(ts, offset))
}

/// The instant `text` names, for callers that keep only the instant.
///
/// # Errors
/// As [`parse_datetime`].
pub fn parse_timestamp(text: &str) -> Result<Timestamp, ParseError> {
    parse_datetime(text).map(|zoned| zoned.timestamp())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    loose: bool,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
            loose: false,
        }
    }

    /// In a loose grammar, step over whitespace; whether any was there.
    fn skip_space(&mut self) -> bool {
        if !self.loose {
            return false;
        }
        let start = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.at += 1;
        }
        self.at > start
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        self.skip_space();
        if self.eat(byte) {
            Ok(())
        } else {
            Err(ParseError::new("unexpected character"))
        }
    }

    /// Exactly `count` digits, or in a loose grammar one to `count`.
    fn digits(&mut self, count: usize) -> Result<i32, ParseError> {
        self.skip_space();
        let mut value = 0_i32;
        let mut read = 0;
        while read < count {
            match self.peek() {
                Some(byte @ b'0'..=b'9') => {
                    value = value * 10 + i32::from(byte - b'0');
                    self.at += 1;
                    read += 1;
                }
                _ => break,
            }
        }
        if read == count || (self.loose && read > 0) {
            Ok(value)
        } else {
            Err(ParseError::new("expected a digit"))
        }
    }

    /// A year: four digits, or in a loose grammar any run of one or more.
    fn year_digits(&mut self) -> Result<i32, ParseError> {
        if !self.loose {
            return self.digits(4);
        }
        self.skip_space();
        let start = self.at;
        let mut value = 0_i32;
        while let Some(byte @ b'0'..=b'9') = self.peek() {
            value = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(i32::from(byte - b'0')))
                .ok_or(ParseError::new("year out of range"))?;
            self.at += 1;
        }
        if self.at == start {
            return Err(ParseError::new("expected a year"));
        }
        Ok(value)
    }

    fn date(&mut self, signed_year: bool) -> Result<Date, ParseError> {
        let year = if signed_year && matches!(self.peek(), Some(b'+' | b'-')) {
            let negative = self.next() == Some(b'-');
            let start = self.at;
            let mut value = 0_i32;
            while let Some(byte @ b'0'..=b'9') = self.peek() {
                value = value
                    .checked_mul(10)
                    .and_then(|value| value.checked_add(i32::from(byte - b'0')))
                    .ok_or(ParseError::new("year out of range"))?;
                self.at += 1;
            }
            if self.at - start < 4 {
                return Err(ParseError::new("expected a year"));
            }
            if negative { -value } else { value }
        } else {
            self.year_digits()?
        };
        self.expect(b'-')?;
        let month = self.digits(2)?;
        self.expect(b'-')?;
        let day = self.digits(2)?;
        let year = i16::try_from(year).map_err(|_| ParseError::new("year out of range"))?;
        Date::new(year, month as i8, day as i8).map_err(|_| ParseError::new("no such date"))
    }

    /// `HH:MM:SS[.fraction]`, and whether the second was a leap second.
    fn time(&mut self) -> Result<(Time, bool), ParseError> {
        let hour = self.digits(2)?;
        self.expect(b':')?;
        let minute = self.digits(2)?;
        self.expect(b':')?;
        let second = self.digits(2)?;
        let mut nanos = 0_i32;
        if self.eat(b'.') {
            let mut count = 0;
            while let Some(byte @ b'0'..=b'9') = self.peek() {
                if count < 9 {
                    nanos = nanos * 10 + i32::from(byte - b'0');
                }
                count += 1;
                self.at += 1;
            }
            if count == 0 {
                return Err(ParseError::new("expected a fraction digit"));
            }
            for _ in count..9 {
                nanos *= 10;
            }
        }
        let leap = second == 60;
        let second = if leap { 59 } else { second };
        let time = Time::new(hour as i8, minute as i8, second as i8, nanos)
            .map_err(|_| ParseError::new("no such time"))?;
        Ok((time, leap))
    }

    fn offset(&mut self, compact: bool) -> Result<Offset, ParseError> {
        // U+2212 MINUS SIGN reads as `-`, as it always has.
        if self.bytes.get(self.at..self.at + 3) == Some("\u{2212}".as_bytes()) {
            self.at += 3;
            return self.offset_after_sign(-1, compact);
        }
        let sign = match self.next() {
            Some(b'Z' | b'z') => return Ok(Offset::UTC),
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => return Err(ParseError::new("expected an offset")),
        };
        self.offset_after_sign(sign, compact)
    }

    fn offset_after_sign(&mut self, sign: i32, compact: bool) -> Result<Offset, ParseError> {
        // Offset fields are always two digits, loose grammar or not.
        let loose = std::mem::replace(&mut self.loose, false);
        let hours = self.digits(2);
        let colon = self.eat(b':');
        let minutes = if colon || compact {
            self.digits(2)
        } else {
            Err(ParseError::new("expected `:` in the offset"))
        };
        self.loose = loose;
        let (hours, minutes) = (hours?, minutes?);
        if hours > 23 || minutes > 59 {
            return Err(ParseError::new("offset minutes out of range"));
        }
        Offset::from_seconds(sign * (hours * 3600 + minutes * 60))
            .map_err(|_| ParseError::new("offset out of range"))
    }

    fn end(&self) -> Result<(), ParseError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(ParseError::new("unexpected text after the timestamp"))
        }
    }
}
