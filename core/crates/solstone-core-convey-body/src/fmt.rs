// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! How the Body app says dates and numbers.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

use crate::presentation::display_number;

pub(crate) fn number(value: f64) -> String {
    display_number("", value, None)
}

pub(crate) fn clock(time: NaiveDateTime) -> String {
    let hour = time.hour() % 12;
    format!(
        "{}:{:02} {}",
        if hour == 0 { 12 } else { hour },
        time.minute(),
        if time.hour() < 12 { "AM" } else { "PM" }
    )
}

pub(crate) fn long_day(day: NaiveDate) -> String {
    format!(
        "{} {}, {}",
        month_full_name(day.month()),
        day.day(),
        day.year()
    )
}

pub(crate) fn parse_day(day: &str) -> Option<NaiveDate> {
    (day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| NaiveDate::parse_from_str(day, "%Y%m%d").ok())
        .flatten()
}

pub(crate) fn short_day(day: &str) -> Option<String> {
    parse_day(day).map(|day| format!("{} {}", month_abbr(day.month()), day.day()))
}

pub(crate) fn short_day_year(day: &str) -> Option<String> {
    let date = parse_day(day)?;
    Some(format!(
        "{} {}, {}",
        month_abbr(date.month()),
        date.day(),
        date.year()
    ))
}

pub(crate) fn month_abbr(month: u32) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS[month as usize - 1]
}

fn month_full_name(month: u32) -> &'static str {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    MONTHS[month as usize - 1]
}

pub(crate) fn grouped_decimal(value: f64, places: usize) -> String {
    let raw = format!("{value:.places$}");
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let sign = whole.strip_prefix('-').map_or("", |_| "-");
    let digits = whole
        .trim_start_matches('-')
        .chars()
        .rev()
        .collect::<Vec<_>>();
    let grouped = digits
        .chunks(3)
        .map(|part| part.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(",")
        .chars()
        .rev()
        .collect::<String>();
    format!("{sign}{grouped}.{fraction}")
}

pub(crate) fn grouped(value: u64) -> String {
    grouped_decimal(value as f64, 0)
        .trim_end_matches('.')
        .to_owned()
}

/// "1 reading", "3 readings".
pub(crate) fn count_noun(count: u64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(count), if count == 1 { one } else { many })
}
