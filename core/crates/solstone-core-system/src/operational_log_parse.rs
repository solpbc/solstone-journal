// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure parsers for the journal host's operational-log display format.

mod fixture;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Weekday};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHealthLogRow {
    pub timestamp: NaiveDateTime,
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

fn parse_time(chars: &[char], date: NaiveDate) -> Option<NaiveDateTime> {
    if chars[13] != ':' || chars[16] != ':' {
        return None;
    }
    let hour = ascii_number(&chars[11..13])?;
    let minute = ascii_number(&chars[14..16])?;
    let second = ascii_number(&chars[17..19])?;
    if hour == 24 {
        return (minute == 0 && second == 0)
            .then(|| date.succ_opt()?.and_hms_opt(0, 0, 0))
            .flatten();
    }
    date.and_hms_opt(hour, minute, second)
}

fn parse_timestamp(prefix: &str) -> Option<NaiveDateTime> {
    let chars = prefix.chars().collect::<Vec<_>>();
    if chars.len() != 19 {
        return None;
    }
    let date = if chars[4] == '-' && chars[7] == '-' {
        NaiveDate::from_ymd_opt(
            ascii_number(&chars[0..4])? as i32,
            ascii_number(&chars[5..7])?,
            ascii_number(&chars[8..10])?,
        )?
    } else if chars[4] == '-' && chars[5] == 'W' && chars[8] == '-' {
        let weekday = match ascii_number(&chars[9..10])? {
            1 => Weekday::Mon,
            2 => Weekday::Tue,
            3 => Weekday::Wed,
            4 => Weekday::Thu,
            5 => Weekday::Fri,
            6 => Weekday::Sat,
            7 => Weekday::Sun,
            _ => return None,
        };
        NaiveDate::from_isoywd_opt(
            ascii_number(&chars[0..4])? as i32,
            ascii_number(&chars[6..8])?,
            weekday,
        )?
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
    supplied_local_now: NaiveDateTime,
    digits: &HashMap<char, u8>,
) -> Option<Result<NaiveDateTime, HealthLogSinceError>> {
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
        'm' => TimeDelta::try_minutes(amount),
        'h' => TimeDelta::try_hours(amount),
        'd' => TimeDelta::try_days(amount),
        _ => unreachable!("unit was matched above"),
    };
    let Some(duration) = duration else {
        return Some(Err(overflow()));
    };
    Some(
        supplied_local_now
            .checked_sub_signed(duration)
            .ok_or_else(overflow),
    )
}

fn parse_12_hour_time(value: &str, format: &str, has_minutes: bool) -> Option<NaiveTime> {
    if let Ok(time) = NaiveTime::parse_from_str(value, format) {
        return Some(time);
    }
    let meridiem = value.get(value.len().checked_sub(2)?..)?;
    if !matches!(meridiem, "AM" | "PM") {
        return None;
    }
    let clock = &value[..value.len() - 2];
    let (hour, minute) = if has_minutes {
        let (hour, minute) = clock.split_once(':')?;
        (
            ascii_number(&hour.chars().collect::<Vec<_>>())?,
            ascii_number(&minute.chars().collect::<Vec<_>>())?,
        )
    } else {
        (ascii_number(&clock.chars().collect::<Vec<_>>())?, 0)
    };
    let hour = match (hour, meridiem) {
        (1..=11, "AM") => hour,
        (12, "AM") => 0,
        (1..=11, "PM") => hour + 12,
        (12, "PM") => 12,
        _ => return None,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

pub fn parse_health_log_since(
    spec: &str,
    supplied_local_now: NaiveDateTime,
) -> Result<NaiveDateTime, HealthLogSinceError> {
    let lookup = unicode_lookup();
    let spec = trim(spec, &lookup.whitespace);
    if let Some(result) = parse_relative(spec, supplied_local_now, &lookup.digits) {
        return result;
    }
    let upper = spec.to_uppercase();
    let time = parse_12_hour_time(&upper, "%I:%M%p", true)
        .or_else(|| parse_12_hour_time(&upper, "%I%p", false))
        .or_else(|| NaiveTime::parse_from_str(&upper, "%H:%M").ok());
    if let Some(time) = time {
        return Ok(supplied_local_now.date().and_time(time));
    }
    Err(HealthLogSinceError::InvalidTime {
        input: spec.to_owned(),
    })
}
