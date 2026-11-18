// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! RFC 5545 recurrence rules (`RRULE`): reading one, and expanding it from a
//! series' first start.
//!
//! The expansion follows the period-by-period method that python-dateutil's
//! `rrule` and the Rust `rrule` crate also use, so its answers, quirks
//! included, match theirs.
//!
//! Expansion works in the series' own wall clock. Each period of the rule's
//! frequency (a year, a month, a week from the week start, a day, an hour, a
//! minute or a second) gives candidate days, the `BY*` parts filter them, and
//! each day takes every time of day the rule allows. `BYSETPOS` then picks
//! from that period's candidates.
//!
//! Each wall-clock candidate becomes an instant in the series' zone. A time
//! that happens exactly once is that instant. A time the clocks skip or repeat
//! is read as that much time after the day's midnight, so a skipped time
//! lands as far past the change as it was written, and a repeated time is its
//! first occurrence when midnight is before the change. A day whose midnight
//! is itself skipped or repeated gives no candidate at such a time.
//!
//! Instances before the first start are not given, and do not count towards
//! `COUNT`. A series ends when its `COUNT` or `UNTIL` is reached, when its
//! year passes 9999, or after 100,000 periods in a row with nothing to give.
//!
//! Rules are read as written by calendar programs: names and values in any
//! case, ordinals on any `BYDAY`, and only the checks RFC 5545 requires of a
//! rule's shape (for example, no `BYWEEKNO` outside `YEARLY`). A rule the
//! reader or those checks refuse has no expansion.

use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
use std::fmt;

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ZonedDateTime};

/// Why a rule has no expansion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuleError;

impl fmt::Display for RuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the recurrence rule cannot be expanded")
    }
}

impl std::error::Error for RuleError {}

/// A rule's frequency, coarsest first.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Frequency {
    Yearly,
    Monthly,
    Weekly,
    Daily,
    Hourly,
    Minutely,
    Secondly,
}

/// A `BYDAY` entry: every such weekday, or the nth of the period (from the
/// end when negative). Weekdays count from Monday as 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ByDay {
    Every(u8),
    Nth(i16, u8),
}

impl ByDay {
    /// The order entries are sorted in: every weekday before ordinals, each
    /// by number then weekday.
    fn order(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Every(left), Self::Every(right)) => left.cmp(&right),
            (Self::Every(_), Self::Nth(..)) => Ordering::Less,
            (Self::Nth(..), Self::Every(_)) => Ordering::Greater,
            (Self::Nth(left_n, left), Self::Nth(right_n, right)) => {
                left_n.cmp(&right_n).then(left.cmp(&right))
            }
        }
    }
}

/// An `UNTIL` read from a rule's own text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Until {
    /// A time ending in `Z`.
    At(Timestamp),
    /// A date or a time with no `Z`, which names no instant in a series'
    /// zone; such a rule is refused when it is built.
    Unzoned,
}

/// A recurrence rule as read, before it meets a series' first start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rule {
    freq: Frequency,
    interval: u16,
    count: Option<u32>,
    until: Option<Until>,
    week_start: u8,
    by_set_pos: Vec<i32>,
    by_month: Vec<u8>,
    by_month_day: Vec<i8>,
    by_n_month_day: Vec<i8>,
    by_year_day: Vec<i16>,
    by_week_no: Vec<i8>,
    by_weekday: Vec<ByDay>,
    by_hour: Vec<u8>,
    by_minute: Vec<u8>,
    by_second: Vec<u8>,
}

impl Rule {
    /// Read a rule from an `RRULE` value such as `FREQ=WEEKLY;BYDAY=TU;COUNT=4`.
    ///
    /// Each part is `NAME=value`, separated by `;`, with no name twice (`BYDAY`
    /// and `BYWEEKDAY` are one name); empty parts are skipped. `FREQ` is
    /// required. A value with a leading `RRULE:` or `EXRULE:` reads the same;
    /// property parameters are refused.
    pub fn parse(text: &str) -> Result<Self, RuleError> {
        parts_to_rule(&rule_parts(rule_value(text)?)?)
    }

    /// The same rule ending at `until`, an instant, in place of any `UNTIL`
    /// its text gave.
    #[must_use]
    pub fn until(mut self, until: Timestamp) -> Self {
        self.until = Some(Until::At(until));
        self
    }

    /// The rule expanded from a series' first start, whose zone the series is
    /// expanded in. Refused when the rule's shape breaks RFC 5545 or its
    /// `UNTIL` is before `start`.
    pub fn build(self, start: &ZonedDateTime) -> Result<Recurrence, RuleError> {
        let rule = self.finalized(start);
        rule.validate(start)?;
        Ok(Recurrence {
            rule,
            start: start.clone(),
        })
    }

    /// Fill the parts a rule leaves to its first start, the way RFC 5545
    /// derives them, and put each list in order without repeats.
    fn finalized(mut self, start: &ZonedDateTime) -> Self {
        let mut by_month_day = Vec::new();
        for day in std::mem::take(&mut self.by_month_day) {
            match day.cmp(&0) {
                Ordering::Greater => by_month_day.push(day),
                Ordering::Less => self.by_n_month_day.push(day),
                Ordering::Equal => {}
            }
        }
        self.by_month_day = by_month_day;
        let month = u8::try_from(start.month()).expect("month is 1-12");
        let day = start.day();
        let hour = u8::try_from(start.hour()).expect("hour is 0-23");
        let minute = u8::try_from(start.minute()).expect("minute is 0-59");
        let second = u8::try_from(start.second()).expect("second is 0-59");
        if self.by_week_no.is_empty()
            && self.by_year_day.is_empty()
            && self.by_month_day.is_empty()
            && self.by_n_month_day.is_empty()
            && self.by_weekday.is_empty()
        {
            match self.freq {
                Frequency::Yearly => {
                    if self.by_month.is_empty() {
                        self.by_month = vec![month];
                    }
                    self.by_month_day = vec![day];
                }
                Frequency::Monthly => self.by_month_day = vec![day],
                Frequency::Weekly => {
                    self.by_weekday = vec![ByDay::Every(monday_zero(start.weekday()))];
                }
                _ => {}
            }
        }
        if self.by_hour.is_empty() && self.freq < Frequency::Hourly {
            self.by_hour = vec![hour];
        }
        if self.by_minute.is_empty() && self.freq < Frequency::Minutely {
            self.by_minute = vec![minute];
        }
        if self.by_second.is_empty() && self.freq < Frequency::Secondly {
            self.by_second = vec![second];
        }
        sort_unique(&mut self.by_hour);
        sort_unique(&mut self.by_minute);
        sort_unique(&mut self.by_second);
        sort_unique(&mut self.by_month);
        sort_unique(&mut self.by_month_day);
        sort_unique(&mut self.by_n_month_day);
        sort_unique(&mut self.by_year_day);
        sort_unique(&mut self.by_week_no);
        sort_unique(&mut self.by_set_pos);
        self.by_weekday
            .sort_unstable_by(|left, right| left.order(*right));
        self.by_weekday.dedup();
        self
    }

    /// RFC 5545's checks on a rule's shape.
    fn validate(&self, start: &ZonedDateTime) -> Result<(), RuleError> {
        let ok = |condition: bool| if condition { Ok(()) } else { Err(RuleError) };
        match self.until {
            Some(Until::Unzoned) => return Err(RuleError),
            Some(Until::At(until)) => ok(until >= start.timestamp())?,
            None => {}
        }
        ok(!self.by_set_pos.contains(&0))?;
        let set_pos_limit = match self.freq {
            Frequency::Yearly | Frequency::Daily => 366,
            Frequency::Monthly => 31,
            Frequency::Weekly => 53,
            Frequency::Hourly => 24,
            Frequency::Minutely | Frequency::Secondly => 60,
        };
        ok(self
            .by_set_pos
            .iter()
            .all(|pos| (-set_pos_limit..=set_pos_limit).contains(pos)))?;
        ok(self.by_set_pos.is_empty()
            || !(self.by_hour.is_empty()
                && self.by_minute.is_empty()
                && self.by_second.is_empty()
                && self.by_month_day.is_empty()
                && self.by_month.is_empty()
                && self.by_year_day.is_empty()
                && self.by_week_no.is_empty()
                && self.by_weekday.is_empty()))?;
        ok(self.by_month.iter().all(|month| (1..=12).contains(month)))?;
        ok(self.by_month_day.iter().all(|day| (-31..=31).contains(day)))?;
        ok(self.by_month_day.is_empty() || self.freq != Frequency::Weekly)?;
        ok(!self.by_year_day.contains(&0))?;
        ok(self.by_year_day.is_empty()
            || !matches!(
                self.freq,
                Frequency::Monthly | Frequency::Weekly | Frequency::Daily
            ))?;
        ok(!self.by_week_no.contains(&0))?;
        ok(self.by_week_no.is_empty() || self.freq == Frequency::Yearly)?;
        let (low, high) = match self.freq {
            Frequency::Yearly | Frequency::Daily => (-52, 53),
            Frequency::Monthly => (-4, 5),
            Frequency::Weekly => (-7, 8),
            Frequency::Hourly => (-3, 4),
            Frequency::Minutely | Frequency::Secondly => (-8, 9),
        };
        ok(self.by_weekday.iter().all(|day| match day {
            ByDay::Nth(number, _) => (low..=high).contains(number),
            ByDay::Every(_) => true,
        }))?;
        ok(self.by_hour.iter().all(|hour| *hour <= 23))?;
        ok(self.by_minute.iter().all(|minute| *minute <= 59))?;
        ok(self.by_second.iter().all(|second| *second <= 59))?;
        Ok(())
    }
}

fn sort_unique<T: Ord>(values: &mut Vec<T>) {
    values.sort_unstable();
    values.dedup();
}

fn monday_zero(weekday: jiff::civil::Weekday) -> u8 {
    u8::try_from(weekday.to_monday_zero_offset()).expect("weekday is 0-6")
}

// ---------------------------------------------------------------------------
// Reading a rule.

/// The rule text without a property name or parameters. A line that begins
/// with upper-case letters and then `:` or `;` names a property; with a `:`
/// the value is what follows the first `:`.
fn rule_value(text: &str) -> Result<&str, RuleError> {
    let named = line_property_name(text);
    if let Some(named) = named
        && !matches!(named, "RRULE" | "EXRULE" | "RDATE" | "EXDATE" | "DTSTART")
    {
        return Err(RuleError);
    }
    let named = named.unwrap_or("RRULE");
    if named == "RRULE" && !text.contains(':') {
        return Ok(text);
    }
    if let Some(after_name) = text.strip_prefix(named)
        && let Some(parameters) = after_name.strip_prefix(';')
        && let Some(colon) = parameters.find(':')
        && colon > 0
    {
        return Err(RuleError);
    }
    Ok(text.split_once(':').map_or("", |(_, value)| value))
}

/// The first line start followed by upper-case letters and then `:` or `;`.
fn line_property_name(text: &str) -> Option<&str> {
    let mut start = 0;
    loop {
        let line = &text[start..];
        let letters = line.bytes().take_while(u8::is_ascii_uppercase).count();
        if letters > 0 && matches!(line.as_bytes().get(letters), Some(b':' | b';')) {
            return Some(&line[..letters]);
        }
        start += line.find('\n')? + 1;
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Part {
    Freq,
    Until,
    Count,
    Interval,
    BySecond,
    ByMinute,
    ByHour,
    ByDay,
    ByMonthDay,
    ByYearDay,
    ByWeekNo,
    ByMonth,
    BySetPos,
    Wkst,
}

impl Part {
    fn from_name(name: &str) -> Result<Self, RuleError> {
        Ok(match name.to_uppercase().as_str() {
            "FREQ" => Self::Freq,
            "UNTIL" => Self::Until,
            "COUNT" => Self::Count,
            "INTERVAL" => Self::Interval,
            "BYSECOND" => Self::BySecond,
            "BYMINUTE" => Self::ByMinute,
            "BYHOUR" => Self::ByHour,
            "BYWEEKDAY" | "BYDAY" => Self::ByDay,
            "BYMONTHDAY" => Self::ByMonthDay,
            "BYYEARDAY" => Self::ByYearDay,
            "BYWEEKNO" => Self::ByWeekNo,
            "BYMONTH" => Self::ByMonth,
            "BYSETPOS" => Self::BySetPos,
            "WKST" => Self::Wkst,
            _ => return Err(RuleError),
        })
    }
}

fn rule_parts(value: &str) -> Result<Vec<(Part, &str)>, RuleError> {
    let mut parts: Vec<(Part, &str)> = Vec::new();
    for raw in value.split(';') {
        if raw.is_empty() {
            continue;
        }
        let (name, text) = raw.split_once('=').ok_or(RuleError)?;
        let part = Part::from_name(name)?;
        if parts.iter().any(|(seen, _)| *seen == part) {
            return Err(RuleError);
        }
        parts.push((part, text));
    }
    Ok(parts)
}

fn parts_to_rule(parts: &[(Part, &str)]) -> Result<Rule, RuleError> {
    let get = |wanted: Part| {
        parts
            .iter()
            .find(|(part, _)| *part == wanted)
            .map(|(_, text)| *text)
    };
    let freq = match get(Part::Freq).ok_or(RuleError)?.to_uppercase().as_str() {
        "YEARLY" => Frequency::Yearly,
        "MONTHLY" => Frequency::Monthly,
        "WEEKLY" => Frequency::Weekly,
        "DAILY" => Frequency::Daily,
        "HOURLY" => Frequency::Hourly,
        "MINUTELY" => Frequency::Minutely,
        "SECONDLY" => Frequency::Secondly,
        _ => return Err(RuleError),
    };
    let interval = get(Part::Interval)
        .map(str::parse::<u16>)
        .transpose()
        .map_err(|_| RuleError)?
        .unwrap_or(1);
    let count = get(Part::Count)
        .map(str::parse::<u32>)
        .transpose()
        .map_err(|_| RuleError)?;
    let until = get(Part::Until).map(rule_until).transpose()?;
    let week_start = get(Part::Wkst).map(weekday).transpose()?.unwrap_or(0);
    Ok(Rule {
        freq,
        interval,
        count,
        until,
        week_start,
        by_set_pos: number_list(get(Part::BySetPos), |_: i32| true)?,
        by_month: number_list(get(Part::ByMonth), |month: u8| (1..=12).contains(&month))?,
        by_month_day: number_list(get(Part::ByMonthDay), |day: i8| (-31..=31).contains(&day))?,
        by_n_month_day: Vec::new(),
        by_year_day: number_list(get(Part::ByYearDay), |day: i16| (-366..=366).contains(&day))?,
        by_week_no: number_list(get(Part::ByWeekNo), |week: i8| (-53..=53).contains(&week))?,
        by_weekday: get(Part::ByDay)
            .map(|text| text.split(',').map(by_day).collect::<Result<Vec<_>, _>>())
            .transpose()?
            .unwrap_or_default(),
        by_hour: number_list(get(Part::ByHour), |hour: u8| hour < 24)?,
        by_minute: number_list(get(Part::ByMinute), |minute: u8| minute < 60)?,
        by_second: number_list(get(Part::BySecond), |second: u8| second < 60)?,
    })
}

/// A comma-separated list of numbers, each accepted by `accept`; empty text
/// is an empty list.
fn number_list<T: std::str::FromStr + Ord + Copy>(
    text: Option<&str>,
    accept: impl Fn(T) -> bool,
) -> Result<Vec<T>, RuleError> {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut values = Vec::new();
    for item in text.split(',') {
        let value = item.parse::<T>().map_err(|_| RuleError)?;
        if !accept(value) {
            return Err(RuleError);
        }
        values.push(value);
    }
    sort_unique(&mut values);
    Ok(values)
}

/// A two-letter weekday in any case, Monday as 0.
fn weekday(text: &str) -> Result<u8, RuleError> {
    Ok(match text.to_uppercase().as_str() {
        "MO" => 0,
        "TU" => 1,
        "WE" => 2,
        "TH" => 3,
        "FR" => 4,
        "SA" => 5,
        "SU" => 6,
        _ => return Err(RuleError),
    })
}

/// A `BYDAY` entry: a weekday after an optional signed ordinal. An ordinal
/// that is not a number, or is zero, means every such weekday.
fn by_day(text: &str) -> Result<ByDay, RuleError> {
    let split = text.len().checked_sub(2).ok_or(RuleError)?;
    let (number, day) = (
        text.get(..split).ok_or(RuleError)?,
        text.get(split..).ok_or(RuleError)?,
    );
    let day = weekday(day)?;
    match number.parse::<i16>().unwrap_or_default() {
        0 => Ok(ByDay::Every(day)),
        number => Ok(ByDay::Nth(number, day)),
    }
}

/// An `UNTIL` in the rule's own text: `YYYYMMDD` or `YYYYMMDDTHHMMSS`, with
/// `Z` for a UTC time.
fn rule_until(text: &str) -> Result<Until, RuleError> {
    let line = text
        .split('\n')
        .find(|line| until_shape(line))
        .ok_or(RuleError)?;
    let number = |range: std::ops::Range<usize>| -> i64 {
        line[range].parse::<i64>().expect("shape checked")
    };
    let date = Date::new(
        i16::try_from(number(0..4)).map_err(|_| RuleError)?,
        i8::try_from(number(4..6)).map_err(|_| RuleError)?,
        i8::try_from(number(6..8)).map_err(|_| RuleError)?,
    )
    .map_err(|_| RuleError)?;
    if line.len() == 8 {
        return Ok(Until::Unzoned);
    }
    let time = Time::new(
        i8::try_from(number(9..11)).map_err(|_| RuleError)?,
        i8::try_from(number(11..13)).map_err(|_| RuleError)?,
        i8::try_from(number(13..15)).map_err(|_| RuleError)?,
        0,
    )
    .map_err(|_| RuleError)?;
    if !line.ends_with('Z') {
        return Ok(Until::Unzoned);
    }
    jiff::tz::Offset::UTC
        .to_timestamp(date.to_datetime(time))
        .map(Until::At)
        .map_err(|_| RuleError)
}

fn until_shape(line: &str) -> bool {
    let bytes = line.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    match bytes.len() {
        8 => digits(0..8),
        15 | 16 => {
            digits(0..8)
                && bytes[8] == b'T'
                && digits(9..15)
                && (bytes.len() == 15 || bytes[15] == b'Z')
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Calendar arithmetic, in days since 1970-01-01, for any year a rule reaches.

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn year_length(year: i32) -> u16 {
    if is_leap_year(year) { 366 } else { 365 }
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Weekday of a day number, Monday as 0.
fn weekday_of(days: i64) -> u16 {
    u16::try_from((days + 3).rem_euclid(7)).expect("0-6")
}

/// The date of a day number; none outside the years a civil date holds
/// (-9999 to 9999).
fn date_of_days(days: i64) -> Option<Date> {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Date::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
    )
    .ok()
}

fn month_starts(year: i32) -> [u16; 13] {
    if is_leap_year(year) {
        [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335, 366]
    } else {
        [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365]
    }
}

/// One year's view of its days, indexed from 0 for 1 January and running
/// seven days into the next year (those days count as January).
struct YearInfo {
    year: i32,
    length: u16,
    next_length: u16,
    first_day: i64,
    first_weekday: u16,
    month_starts: [u16; 13],
    /// Which days belong to a `BYWEEKNO` week, when the rule has any.
    week_numbers: Option<Vec<bool>>,
}

impl YearInfo {
    fn new(year: i32, rule: &Rule) -> Self {
        let first_day = days_from_civil(year, 1, 1);
        let first_weekday = weekday_of(first_day);
        let mut info = Self {
            year,
            length: year_length(year),
            next_length: year_length(year + 1),
            first_day,
            first_weekday,
            month_starts: month_starts(year),
            week_numbers: None,
        };
        if !rule.by_week_no.is_empty() {
            info.week_numbers = Some(info.week_number_days(rule));
        }
        info
    }

    fn month(&self, day: usize) -> u8 {
        if day >= usize::from(self.length) {
            return 1;
        }
        let index = self
            .month_starts
            .iter()
            .rposition(|start| usize::from(*start) <= day)
            .expect("day 0 starts January");
        u8::try_from(index + 1).expect("1-12")
    }

    fn month_day(&self, day: usize) -> i8 {
        let length = usize::from(self.length);
        if day >= length {
            return i8::try_from(day - length + 1).expect("1-7");
        }
        let month = usize::from(self.month(day));
        i8::try_from(day - usize::from(self.month_starts[month - 1]) + 1).expect("1-31")
    }

    fn negative_month_day(&self, day: usize) -> i8 {
        let length = usize::from(self.length);
        if day >= length {
            return i8::try_from(day - length).expect("0-6") - 31;
        }
        let month = usize::from(self.month(day));
        let end = i64::from(self.month_starts[month]);
        i8::try_from(i64::try_from(day).expect("small") - end).expect("-31 to -1")
    }

    fn weekday(&self, day: usize) -> u16 {
        let day = u16::try_from(day).expect("a year's days fit");
        (self.first_weekday + day) % 7
    }

    /// The days of the weeks `BYWEEKNO` names, with weeks starting on the
    /// rule's week start and week 1 the first with four days in the year.
    fn week_number_days(&self, rule: &Rule) -> Vec<bool> {
        let length = self.length;
        let mut days = vec![false; usize::from(length) + 7];
        let week_start = u16::from(rule.week_start);
        let mut first_week = (7 - self.first_weekday + week_start) % 7;
        let first_week_start = first_week;
        let extended_length = if first_week >= 4 {
            first_week = 0;
            let shift = (i32::from(self.first_weekday) - i32::from(week_start)).rem_euclid(7);
            length + u16::try_from(shift).expect("0-6")
        } else {
            length - first_week
        };
        let weeks = extended_length / 7 + (extended_length % 7) / 4;
        let mut mark_week = |from: u16| {
            for day in from..from + 7 {
                let day = usize::from(day);
                if let Some(slot) = days.get_mut(day) {
                    *slot = true;
                }
                if self.weekday(day + 1) == week_start {
                    break;
                }
            }
        };
        let weeks_i8 = i8::try_from(weeks).expect("52-54");
        for &number in &rule.by_week_no {
            let mut number = number;
            if number < 0 {
                number += weeks_i8 + 1;
            }
            if !(number > 0 && number <= weeks_i8) {
                continue;
            }
            let from = if number > 1 {
                let mut from = first_week + (u16::try_from(number).expect("positive") - 1) * 7;
                if first_week != first_week_start {
                    from -= 7 - first_week_start;
                }
                from
            } else {
                first_week
            };
            mark_week(from);
        }
        if rule.by_week_no.contains(&1) {
            let mut from = first_week + weeks * 7;
            if first_week != first_week_start {
                from -= 7 - first_week_start;
            }
            if from < length {
                mark_week(from);
            }
        }
        if first_week > 0 {
            let last_year_weeks = if rule.by_week_no.contains(&-1) {
                -1
            } else {
                let last_first_weekday = weekday_of(days_from_civil(self.year - 1, 1, 1));
                let last_first_week = (7 - last_first_weekday + week_start) % 7;
                let span = if last_first_week >= 4 {
                    let shift =
                        (i32::from(last_first_weekday) - i32::from(week_start)).rem_euclid(7);
                    year_length(self.year - 1) + u16::try_from(shift).expect("0-6")
                } else {
                    length - first_week
                };
                52 + i8::try_from((span % 7) / 4).expect("0-1")
            };
            if rule.by_week_no.contains(&last_year_weeks) {
                for day in days.iter_mut().take(usize::from(first_week)) {
                    *day = true;
                }
            }
        }
        days
    }

    /// The days `BYDAY` ordinals pick within the year (`YEARLY`, by month when
    /// `BYMONTH` is given) or within `month` (`MONTHLY`); empty for any other
    /// frequency, where ordinals pick nothing out.
    fn nth_weekday_days(&self, month: u8, rule: &Rule) -> Vec<bool> {
        let mut ranges = Vec::new();
        let month_range = |month: u8| {
            let month = usize::from(month);
            (
                u32::from(self.month_starts[month - 1]),
                u32::from(self.month_starts[month]) - 1,
            )
        };
        match rule.freq {
            Frequency::Yearly if rule.by_month.is_empty() => {
                ranges.push((0, u32::from(self.length) - 1));
            }
            Frequency::Yearly => {
                ranges.extend(rule.by_month.iter().map(|month| month_range(*month)))
            }
            Frequency::Monthly => ranges.push(month_range(month)),
            _ => {}
        }
        if ranges.is_empty() {
            return Vec::new();
        }
        let mut days = vec![false; usize::from(self.length)];
        let weekday_at = |day: u32| -> Option<i16> {
            let day = usize::try_from(day).ok()?;
            (day < 385 - usize::from(self.first_weekday))
                .then(|| i16::try_from(self.weekday(day)).expect("0-6"))
        };
        for (first, last) in ranges {
            for entry in &rule.by_weekday {
                let ByDay::Nth(number, weekday) = *entry else {
                    continue;
                };
                let weekday = i16::from(weekday);
                let picked = if number < 0 {
                    let number = u32::from(number.unsigned_abs());
                    let Some(last_week) = last.checked_sub((number - 1) * 7) else {
                        continue;
                    };
                    let Some(last_week_weekday) = weekday_at(last_week) else {
                        continue;
                    };
                    let back =
                        u32::try_from((last_week_weekday - weekday).rem_euclid(7)).expect("0-6");
                    let Some(day) = last_week.checked_sub(back) else {
                        continue;
                    };
                    day
                } else {
                    let first_week = first + (u32::try_from(number).expect("positive") - 1) * 7;
                    let Some(first_week_weekday) = weekday_at(first_week) else {
                        continue;
                    };
                    let ahead = u32::try_from((7 - first_week_weekday + weekday).rem_euclid(7))
                        .expect("0-6");
                    first_week + ahead
                };
                if first <= picked && picked <= last {
                    days[usize::try_from(picked).expect("in the year")] = true;
                }
            }
        }
        days
    }
}

// ---------------------------------------------------------------------------
// Stepping from period to period.

/// The wall-clock fields of the period being expanded. A monthly or yearly
/// step leaves the day as it was, even past the month's end.
#[derive(Clone, Copy, Debug)]
struct Counter {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

/// Stepping went past the years a rule may reach, or found no time of day
/// the rule can ever reach again.
struct StepEnd;

impl Counter {
    fn step(&mut self, rule: &Rule, empty_period: bool) -> Result<(), StepEnd> {
        let interval = u32::from(rule.interval);
        match rule.freq {
            Frequency::Yearly => {
                self.year += i32::from(rule.interval);
                check_year(self.year)?;
                self.fix_day()
            }
            Frequency::Monthly => self.add_months(interval),
            Frequency::Weekly => {
                let weekday = self.weekday();
                let week_start = u32::from(rule.week_start);
                self.day += if week_start > weekday {
                    interval * 7 - (weekday + 7 - week_start)
                } else {
                    interval * 7 - (weekday - week_start)
                };
                self.fix_day()
            }
            Frequency::Daily => self.add_days(interval),
            Frequency::Hourly => self.add_hours(interval, &rule.by_hour, empty_period),
            Frequency::Minutely => {
                self.add_minutes(interval, &rule.by_hour, &rule.by_minute, empty_period)
            }
            Frequency::Secondly => self.add_seconds(
                interval,
                &rule.by_hour,
                &rule.by_minute,
                &rule.by_second,
                empty_period,
            ),
        }
    }

    fn add_months(&mut self, months: u32) -> Result<(), StepEnd> {
        self.month += months;
        if self.month > 12 {
            let mut years = u16::try_from(self.month).map_err(|_| StepEnd)? / 12;
            self.month %= 12;
            if self.month == 0 {
                self.month = 12;
                years -= 1;
            }
            self.year += i32::from(years);
            check_year(self.year)?;
        }
        Ok(())
    }

    fn weekday(&self) -> u32 {
        let starts = month_starts(self.year);
        let day_of_year = u32::from(starts[self.month as usize - 1]) + self.day - 1;
        let first_weekday = u32::from(weekday_of(days_from_civil(self.year, 1, 1)));
        (day_of_year % 7 + first_weekday) % 7
    }

    fn days_in_month(&self) -> u32 {
        let starts = month_starts(self.year);
        u32::from(starts[self.month as usize] - starts[self.month as usize - 1])
    }

    fn fix_day(&mut self) -> Result<(), StepEnd> {
        if self.day <= 28 {
            return Ok(());
        }
        let mut days_in_month = self.days_in_month();
        while self.day > days_in_month {
            self.day -= days_in_month;
            self.add_months(1)?;
            days_in_month = self.days_in_month();
        }
        Ok(())
    }

    fn add_days(&mut self, days: u32) -> Result<(), StepEnd> {
        self.day += days;
        self.fix_day()
    }

    fn add_hours(&mut self, hours: u32, by_hour: &[u8], empty_period: bool) -> Result<(), StepEnd> {
        if empty_period {
            // Jump to the day's last step before stepping on.
            self.hour += ((23 - self.hour) / hours)
                .checked_mul(hours)
                .ok_or(StepEnd)?;
        }
        let mut seen = HashSet::new();
        loop {
            self.hour = self.hour.checked_add(hours).ok_or(StepEnd)?;
            let hour = u8::try_from(self.hour % 24).expect("0-23");
            if by_hour.is_empty() || by_hour.contains(&hour) {
                break;
            }
            if !seen.insert(hour) {
                return Err(StepEnd);
            }
        }
        let days = u16::try_from(self.hour / 24).map_err(|_| StepEnd)?;
        self.hour %= 24;
        if days > 0 {
            self.add_days(u32::from(days))
        } else {
            Ok(())
        }
    }

    fn add_minutes(
        &mut self,
        minutes: u32,
        by_hour: &[u8],
        by_minute: &[u8],
        empty_period: bool,
    ) -> Result<(), StepEnd> {
        if empty_period {
            let elapsed = self.hour * 60 + self.minute;
            self.minute += ((24 * 60 - 1 - elapsed) / minutes)
                .checked_mul(minutes)
                .ok_or(StepEnd)?;
        }
        let mut seen = HashSet::new();
        loop {
            self.minute += minutes;
            let hours = u16::try_from(self.minute / 60).map_err(|_| StepEnd)?;
            if hours > 0 {
                self.minute %= 60;
                self.add_hours(u32::from(hours), by_hour, empty_period)?;
            }
            let hour = u8::try_from(self.hour % 24).expect("0-23");
            let minute = u8::try_from(self.minute % 60).expect("0-59");
            if (by_hour.is_empty() || by_hour.contains(&hour))
                && (by_minute.is_empty() || by_minute.contains(&minute))
            {
                break;
            }
            if !seen.insert((hour, minute)) {
                return Err(StepEnd);
            }
        }
        Ok(())
    }

    fn add_seconds(
        &mut self,
        seconds: u32,
        by_hour: &[u8],
        by_minute: &[u8],
        by_second: &[u8],
        empty_period: bool,
    ) -> Result<(), StepEnd> {
        if empty_period {
            let elapsed = self.hour * 3600 + self.minute * 60 + self.second;
            self.second += ((24 * 60 * 60 - 1 - elapsed) / seconds)
                .checked_mul(seconds)
                .ok_or(StepEnd)?;
        }
        let mut seen = HashSet::new();
        loop {
            self.second += seconds;
            let minutes = u16::try_from(self.second / 60).map_err(|_| StepEnd)?;
            if minutes > 0 {
                self.second %= 60;
                self.add_minutes(u32::from(minutes), by_hour, by_minute, empty_period)?;
            }
            let hour = u8::try_from(self.hour % 24).expect("0-23");
            let minute = u8::try_from(self.minute % 60).expect("0-59");
            let second = u8::try_from(self.second % 60).expect("0-59");
            if (by_hour.is_empty() || by_hour.contains(&hour))
                && (by_minute.is_empty() || by_minute.contains(&minute))
                && (by_second.is_empty() || by_second.contains(&second))
            {
                break;
            }
            if !seen.insert((hour, minute, second)) {
                return Err(StepEnd);
            }
        }
        Ok(())
    }
}

fn check_year(year: i32) -> Result<(), StepEnd> {
    if (-10_000..=10_000).contains(&year) {
        Ok(())
    } else {
        Err(StepEnd)
    }
}

// ---------------------------------------------------------------------------
// Expansion.

/// Periods in a row that give no instance before a series is taken to end.
const MAX_EMPTY_PERIODS: u32 = 100_000;

/// A rule built from a series' first start.
#[derive(Clone, Debug)]
pub struct Recurrence {
    rule: Rule,
    start: ZonedDateTime,
}

impl Recurrence {
    /// Every instance from the first start on, in the order the rule gives
    /// them: period by period, and within a period by day and then time of
    /// day (by instant where `BYSETPOS` picks).
    #[must_use]
    pub fn iter(&self) -> Occurrences<'_> {
        let start = &self.start;
        let counter = Counter {
            year: i32::from(start.year()),
            month: u32::from(start.month().unsigned_abs()),
            day: u32::from(start.day().unsigned_abs()),
            hour: u32::from(start.hour().unsigned_abs()),
            minute: u32::from(start.minute().unsigned_abs()),
            second: u32::from(start.second().unsigned_abs()),
        };
        let mut occurrences = Occurrences {
            rule: &self.rule,
            zone: start.time_zone().clone(),
            first: start.timestamp(),
            year: YearInfo::new(counter.year, &self.rule),
            nth: None,
            counter,
            times: Vec::new(),
            buffer: VecDeque::new(),
            finished: false,
            count: self.rule.count,
        };
        occurrences.times = occurrences.first_times(
            u8::try_from(counter.hour).expect("0-23"),
            u8::try_from(counter.minute).expect("0-59"),
            u8::try_from(counter.second).expect("0-59"),
        );
        occurrences.refresh_nth();
        occurrences
    }
}

/// The instances of a [`Recurrence`].
pub struct Occurrences<'a> {
    rule: &'a Rule,
    zone: TimeZone,
    first: Timestamp,
    year: YearInfo,
    /// The days `BYDAY` ordinals pick, for the year and month they were
    /// worked out for.
    nth: Option<(i32, u32, Vec<bool>)>,
    counter: Counter,
    times: Vec<Time>,
    buffer: VecDeque<ZonedDateTime>,
    finished: bool,
    count: Option<u32>,
}

impl Iterator for Occurrences<'_> {
    type Item = ZonedDateTime;

    fn next(&mut self) -> Option<ZonedDateTime> {
        if let Some(next) = self.buffer.pop_front() {
            return Some(next);
        }
        if self.finished {
            return None;
        }
        self.finished = self.fill();
        if self.buffer.is_empty() {
            self.finished = true;
        }
        self.buffer.pop_front()
    }
}

impl Occurrences<'_> {
    /// The times of day the first period may give. For an hourly or finer
    /// rule, none when the first start is not itself a time the rule allows.
    fn first_times(&self, hour: u8, minute: u8, second: u8) -> Vec<Time> {
        let rule = self.rule;
        if rule.freq >= Frequency::Hourly {
            let wrong_hour = !rule.by_hour.is_empty() && !rule.by_hour.contains(&hour);
            let wrong_minute = rule.freq >= Frequency::Minutely
                && !rule.by_minute.is_empty()
                && !rule.by_minute.contains(&minute);
            let wrong_second = rule.freq >= Frequency::Secondly
                && !rule.by_second.is_empty()
                && !rule.by_second.contains(&second);
            if wrong_hour || wrong_minute || wrong_second {
                return Vec::new();
            }
            return self.period_times(hour, minute, second);
        }
        let mut times = Vec::new();
        for hour in &rule.by_hour {
            for minute in &rule.by_minute {
                for second in &rule.by_second {
                    times.push(time_of(*hour, *minute, *second));
                }
            }
        }
        times
    }

    /// The times of day an hourly or finer period gives.
    fn period_times(&self, hour: u8, minute: u8, second: u8) -> Vec<Time> {
        let rule = self.rule;
        match rule.freq {
            Frequency::Hourly => rule
                .by_minute
                .iter()
                .flat_map(|minute| {
                    rule.by_second
                        .iter()
                        .map(move |second| time_of(hour, *minute, *second))
                })
                .collect(),
            Frequency::Minutely => rule
                .by_second
                .iter()
                .map(|second| time_of(hour, minute, *second))
                .collect(),
            _ => vec![time_of(hour, minute, second)],
        }
    }

    fn refresh_nth(&mut self) {
        let has_nth = self
            .rule
            .by_weekday
            .iter()
            .any(|day| matches!(day, ByDay::Nth(..)));
        let (year, month) = (self.counter.year, self.counter.month);
        if has_nth && !matches!(&self.nth, Some((y, m, _)) if *y == year && *m == month) {
            let month = u8::try_from(month).expect("1-12");
            self.nth = Some((
                year,
                self.counter.month,
                self.year.nth_weekday_days(month, self.rule),
            ));
        }
    }

    /// Move the year's view to the counter's year and month.
    fn rebuild(&mut self) {
        if self.year.year != self.counter.year {
            self.year = YearInfo::new(self.counter.year, self.rule);
        }
        self.refresh_nth();
    }

    /// The period's candidate days, each a day index in the year's view.
    fn period_days(&self) -> Vec<usize> {
        let Counter {
            year, month, day, ..
        } = self.counter;
        let info = &self.year;
        let day_index = |month: u32, day: u32| {
            usize::from(info.month_starts[month as usize - 1]) + day as usize - 1
        };
        let mut days: Vec<usize> = match self.rule.freq {
            Frequency::Yearly => (0..usize::from(info.length)).collect(),
            Frequency::Monthly => {
                let month = month as usize;
                (usize::from(info.month_starts[month - 1])..usize::from(info.month_starts[month]))
                    .collect()
            }
            Frequency::Weekly => {
                let limit = usize::from(info.length) + 7;
                let week_start = u16::from(self.rule.week_start);
                let mut index = day_index(month, day);
                let mut days = Vec::new();
                for _ in 0..7 {
                    if index >= limit {
                        break;
                    }
                    days.push(index);
                    index += 1;
                    if info.weekday(index) == week_start {
                        break;
                    }
                }
                days
            }
            _ => vec![day_index(month, day)],
        };
        debug_assert_eq!(info.year, year);
        days.retain(|day| !self.filtered(*day));
        days
    }

    /// Whether the `BY*` parts leave `day` out.
    fn filtered(&self, day: usize) -> bool {
        let rule = self.rule;
        let info = &self.year;
        if !rule.by_month.is_empty() && !rule.by_month.contains(&info.month(day)) {
            return true;
        }
        if !rule.by_week_no.is_empty()
            && info.week_numbers.as_ref().is_some_and(|weeks| !weeks[day])
        {
            return true;
        }
        let every = rule.by_weekday.iter().filter_map(|entry| match entry {
            ByDay::Every(weekday) => Some(u16::from(*weekday)),
            ByDay::Nth(..) => None,
        });
        let mut every = every.peekable();
        if every.peek().is_some() && !every.any(|weekday| weekday == info.weekday(day)) {
            return true;
        }
        if let Some((_, _, nth)) = &self.nth
            && !nth.is_empty()
            && !nth[day]
        {
            return true;
        }
        if !(rule.by_month_day.is_empty() && rule.by_n_month_day.is_empty())
            && !rule.by_month_day.contains(&info.month_day(day))
            && !rule.by_n_month_day.contains(&info.negative_month_day(day))
        {
            return true;
        }
        if !rule.by_year_day.is_empty() {
            let Ok(day) = i16::try_from(day) else {
                return true;
            };
            let length = i16::try_from(info.length).expect("365-366");
            let next_length = i16::try_from(info.next_length).expect("365-366");
            let (forward, backward) = if day < length {
                (day + 1, day - length)
            } else {
                (day + 1 - length, day - next_length - length)
            };
            if !rule.by_year_day.contains(&forward) && !rule.by_year_day.contains(&backward) {
                return true;
            }
        }
        false
    }

    /// The instant a wall-clock candidate names in the series' zone; see the
    /// module notes for times the clocks skip or repeat.
    fn instant(&self, date: Date, time: Time) -> Option<ZonedDateTime> {
        if let Some(single) =
            solstone_core_time::from_local(&self.zone, date.to_datetime(time)).single()
        {
            return Some(single);
        }
        let midnight =
            solstone_core_time::from_local(&self.zone, date.to_datetime(Time::midnight()))
                .single()?;
        let since_midnight = i64::from(time.hour()) * 3600
            + i64::from(time.minute()) * 60
            + i64::from(time.second());
        midnight
            .checked_add(SignedDuration::from_secs(since_midnight))
            .ok()
    }

    /// Offer one candidate; true once the series has ended.
    fn offer(&mut self, candidate: ZonedDateTime) -> bool {
        if let Some(Until::At(until)) = self.rule.until
            && candidate.timestamp() > until
        {
            return true;
        }
        if candidate.timestamp() >= self.first {
            self.buffer.push_back(candidate);
            if let Some(count) = self.count.as_mut() {
                *count -= 1;
                if *count == 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Expand periods until one gives an instance; true once the series has
    /// ended.
    fn fill(&mut self) -> bool {
        if self.count == Some(0) || self.rule.interval == 0 {
            return true;
        }
        let mut periods = 0_u32;
        while self.buffer.is_empty() {
            // A rule that gives nothing for this many periods in a row gives
            // nothing ever after (it can only be one whose BYSETPOS picks past
            // every period's candidates), so the series ends here rather than
            // stepping on to the year 10000.
            periods += 1;
            if periods >= MAX_EMPTY_PERIODS {
                return true;
            }
            let days = self.period_days();
            if self.offer_period(&days) {
                return true;
            }
            if self.counter.step(self.rule, days.is_empty()).is_err() {
                return true;
            }
            if self.rule.freq >= Frequency::Hourly {
                self.times = self.period_times(
                    u8::try_from(self.counter.hour).expect("0-23"),
                    u8::try_from(self.counter.minute).expect("0-59"),
                    u8::try_from(self.counter.second).expect("0-59"),
                );
            }
            self.rebuild();
        }
        false
    }

    /// Offer the period's instances in order: by day and then time of day,
    /// or by instant for the ones `BYSETPOS` picks. True once the series has
    /// ended, which includes reaching a date past the last representable one.
    fn offer_period(&mut self, days: &[usize]) -> bool {
        let first_day = self.year.first_day;
        let date = |day: usize| date_of_days(first_day + i64::try_from(day).expect("small"));
        let times = std::mem::take(&mut self.times);
        let ended = 'period: {
            if self.rule.by_set_pos.is_empty() {
                for day in days {
                    let Some(date) = date(*day) else {
                        break 'period true;
                    };
                    for time in &times {
                        if let Some(candidate) = self.instant(date, *time)
                            && self.offer(candidate)
                        {
                            break 'period true;
                        }
                    }
                }
                break 'period false;
            }
            if times.is_empty() {
                break 'period false;
            }
            let time_count = i64::try_from(times.len()).expect("small");
            let day_count = i64::try_from(days.len()).expect("small");
            let mut picked: Vec<ZonedDateTime> = Vec::new();
            for position in &self.rule.by_set_pos {
                let position = i64::from(if *position > 0 {
                    position - 1
                } else {
                    *position
                });
                let day_position = position.div_euclid(time_count);
                let time_position =
                    usize::try_from(position.rem_euclid(time_count)).expect("in range");
                let day_index = if day_position < 0 {
                    day_count + day_position
                } else {
                    day_position
                };
                let Some(day) = usize::try_from(day_index)
                    .ok()
                    .and_then(|index| days.get(index))
                else {
                    continue;
                };
                let Some(date) = date(*day) else {
                    break 'period true;
                };
                let Some(candidate) = self.instant(date, times[time_position]) else {
                    continue;
                };
                if !picked
                    .iter()
                    .any(|seen| seen.timestamp() == candidate.timestamp())
                {
                    picked.push(candidate);
                }
            }
            picked.sort_by_key(ZonedDateTime::timestamp);
            for candidate in picked {
                if self.offer(candidate) {
                    break 'period true;
                }
            }
            false
        };
        self.times = times;
        ended
    }
}

fn time_of(hour: u8, minute: u8, second: u8) -> Time {
    Time::new(
        i8::try_from(hour).expect("0-23"),
        i8::try_from(minute).expect("0-59"),
        i8::try_from(second).expect("0-59"),
        0,
    )
    .expect("validated time of day")
}

#[cfg(test)]
#[path = "ics_recur_tests.rs"]
mod tests;
