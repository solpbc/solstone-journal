// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure parsers for the journal host's operational-log display format.

mod fixture;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use jiff::SignedDuration;
use jiff::civil::{Date, DateTime, ISOWeekDate, Time, Weekday};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHealthLogRow {
    pub timestamp: DateTime,
    pub service: String,
    pub stream: String,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HealthLogSinceError {
    #[error("Python int too large to convert to C int")]
    RelativeOverflow { input: String },
    #[error("Invalid time: '{input}'. Use e.g., 30m, 2h, 1d, 4pm, 16:00")]
    InvalidTime { input: String },
}

struct UnicodeLookup {
    whitespace: HashSet<char>,
    digits: HashMap<char, u8>,
}

static UNICODE_LOOKUP: OnceLock<UnicodeLookup> = OnceLock::new();

fn unicode_lookup() -> &'static UnicodeLookup {
    UNICODE_LOOKUP.get_or_init(|| {
        let contract = &fixture::fixture().unicode_contract;
        let whitespace = contract
            .whitespace_codepoints
            .iter()
            .map(|&value| char::from_u32(value).expect("valid fixture whitespace scalar"))
            .collect();
        let digits = contract
            .decimal_codepoints
            .iter()
            .map(|&(scalar, value)| {
                (
                    char::from_u32(scalar).expect("valid fixture decimal scalar"),
                    value,
                )
            })
            .collect();
        UnicodeLookup { whitespace, digits }
    })
}

fn scalar_offsets(value: &str) -> Vec<usize> {
    let mut offsets = value
        .char_indices()
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    offsets.push(value.len());
    offsets
}

fn trim_end<'a>(value: &'a str, whitespace: &HashSet<char>) -> &'a str {
    let offsets = scalar_offsets(value);
    let mut end = offsets.len() - 1;
    while end > 0 {
        let scalar = value[offsets[end - 1]..offsets[end]]
            .chars()
            .next()
            .expect("scalar range is nonempty");
        if !whitespace.contains(&scalar) {
            break;
        }
        end -= 1;
    }
    &value[..offsets[end]]
}

fn trim<'a>(value: &'a str, whitespace: &HashSet<char>) -> &'a str {
    let offsets = scalar_offsets(value);
    let mut start = 0;
    let mut end = offsets.len() - 1;
    while start < end {
        let scalar = value[offsets[start]..offsets[start + 1]]
            .chars()
            .next()
            .expect("scalar range is nonempty");
        if !whitespace.contains(&scalar) {
            break;
        }
        start += 1;
    }
    while start < end {
        let scalar = value[offsets[end - 1]..offsets[end]]
            .chars()
            .next()
            .expect("scalar range is nonempty");
        if !whitespace.contains(&scalar) {
            break;
        }
        end -= 1;
    }
    &value[offsets[start]..offsets[end]]
}

fn scalar_at(value: &str, offsets: &[usize], index: usize) -> char {
    value[offsets[index]..offsets[index + 1]]
        .chars()
        .next()
        .expect("scalar range is nonempty")
}

fn ascii_number(chars: &[char]) -> Option<u32> {
    chars.iter().try_fold(0_u32, |value, digit| {
        digit
            .is_ascii_digit()
            .then(|| *digit as u32 - '0' as u32)
            .and_then(|digit| value.checked_mul(10)?.checked_add(digit))
    })
}

/// A parsed field as jiff's narrow field type; a value that does not fit is
/// out of range for every field it fills.
fn field<T: TryFrom<u32>>(chars: &[char]) -> Option<T> {
    T::try_from(ascii_number(chars)?).ok()
}

fn parse_time(chars: &[char], date: Date) -> Option<DateTime> {
    if chars[13] != ':' || chars[16] != ':' {
        return None;
    }
    let hour = field::<i8>(&chars[11..13])?;
    let minute = field::<i8>(&chars[14..16])?;
    let second = field::<i8>(&chars[17..19])?;
    if hour == 24 {
        return (minute == 0 && second == 0)
            .then(|| Some(date.tomorrow().ok()?.at(0, 0, 0, 0)))
            .flatten();
    }
    Some(date.to_datetime(Time::new(hour, minute, second, 0).ok()?))
}

fn parse_timestamp(prefix: &str) -> Option<DateTime> {
    let chars = prefix.chars().collect::<Vec<_>>();
    if chars.len() != 19 {
        return None;
    }
    let date = if chars[4] == '-' && chars[7] == '-' {
        Date::new(
            field(&chars[0..4])?,
            field(&chars[5..7])?,
            field(&chars[8..10])?,
        )
        .ok()?
    } else if chars[4] == '-' && chars[5] == 'W' && chars[8] == '-' {
        let weekday = match ascii_number(&chars[9..10])? {
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            6 => Weekday::Saturday,
            7 => Weekday::Sunday,
            _ => return None,
        };
        ISOWeekDate::new(field(&chars[0..4])?, field(&chars[6..8])?, weekday)
            .ok()?
            .date()
    } else {
        return None;
    };
    parse_time(&chars, date)
}

pub fn parse_health_log_row(input: &str) -> Option<ParsedHealthLogRow> {
    let lookup = unicode_lookup();
    let stripped = trim_end(input, &lookup.whitespace);
    let offsets = scalar_offsets(stripped);
    let count = offsets.len() - 1;
    if count < 20 {
        return None;
    }
    let timestamp = parse_timestamp(&stripped[..offsets[19]])?;
    let open = (19..count).find(|&index| scalar_at(stripped, &offsets, index) == '[')?;
    let close = ((open + 1)..count).find(|&index| scalar_at(stripped, &offsets, index) == ']')?;
    let bracket = &stripped[offsets[open + 1]..offsets[close]];
    let (service, stream) = bracket.rsplit_once(':')?;
    let message_start = close + 2;
    let message = if message_start <= count {
        stripped[offsets[message_start]..].to_owned()
    } else {
        String::new()
    };
    Some(ParsedHealthLogRow {
        timestamp,
        service: service.to_owned(),
        stream: stream.to_owned(),
        message,
        raw: stripped.to_owned(),
    })
}

fn parse_relative(
    spec: &str,
    supplied_local_now: DateTime,
    digits: &HashMap<char, u8>,
) -> Option<Result<DateTime, HealthLogSinceError>> {
    let chars = spec.chars().collect::<Vec<_>>();
    let (&unit, number) = chars.split_last()?;
    if number.is_empty() || !matches!(unit, 'm' | 'h' | 'd') {
        return None;
    }
    let mut amount = 0_u64;
    for scalar in number {
        let &digit = digits.get(scalar)?;
        let Some(next) = amount
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(digit)))
        else {
            return Some(Err(HealthLogSinceError::RelativeOverflow {
                input: spec.to_owned(),
            }));
        };
        amount = next;
    }
    let overflow = || HealthLogSinceError::RelativeOverflow {
        input: spec.to_owned(),
    };
    let Ok(amount) = i64::try_from(amount) else {
        return Some(Err(overflow()));
    };
    let duration = match unit {
        'm' => SignedDuration::try_from_mins(amount),
        'h' => SignedDuration::try_from_hours(amount),
        'd' => amount
            .checked_mul(24)
            .and_then(SignedDuration::try_from_hours),
        _ => unreachable!("unit was matched above"),
    };
    let Some(duration) = duration else {
        return Some(Err(overflow()));
    };
    Some(
        supplied_local_now
            .checked_sub(duration)
            .map_err(|_| overflow()),
    )
}

fn parse_12_hour_time(value: &str, format: &str, has_minutes: bool) -> Option<Time> {
    if let Ok(time) = Time::strptime(format, value) {
        return Some(time);
    }
    let meridiem = value.get(value.len().checked_sub(2)?..)?;
    if !matches!(meridiem, "AM" | "PM") {
        return None;
    }
    let clock = &value[..value.len() - 2];
    let (hour, minute) = if has_minutes {
        let (hour, minute) = clock.split_once(':')?;
        if !(1..=2).contains(&hour.len()) || !(1..=2).contains(&minute.len()) {
            return None;
        }
        (
            ascii_number(&hour.chars().collect::<Vec<_>>())?,
            ascii_number(&minute.chars().collect::<Vec<_>>())?,
        )
    } else {
        if !(1..=2).contains(&clock.len()) {
            return None;
        }
        (ascii_number(&clock.chars().collect::<Vec<_>>())?, 0)
    };
    let hour = match (hour, meridiem) {
        (1..=11, "AM") => hour,
        (12, "AM") => 0,
        (1..=11, "PM") => hour + 12,
        (12, "PM") => 12,
        _ => return None,
    };
    Time::new(i8::try_from(hour).ok()?, i8::try_from(minute).ok()?, 0, 0).ok()
}

pub fn parse_health_log_since(
    spec: &str,
    supplied_local_now: DateTime,
) -> Result<DateTime, HealthLogSinceError> {
    let lookup = unicode_lookup();
    let spec = trim(spec, &lookup.whitespace);
    if let Some(result) = parse_relative(spec, supplied_local_now, &lookup.digits) {
        return result;
    }
    let upper = spec.to_uppercase();
    let time = parse_12_hour_time(&upper, "%I:%M%p", true)
        .or_else(|| parse_12_hour_time(&upper, "%I%p", false))
        .or_else(|| Time::strptime("%H:%M", &upper).ok());
    if let Some(time) = time {
        return Ok(supplied_local_now.date().to_datetime(time));
    }
    Err(HealthLogSinceError::InvalidTime {
        input: spec.to_owned(),
    })
}
