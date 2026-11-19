// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! How the Body app says dates and numbers.

use jiff::civil::{Date, DateTime};
use solstone_core_convey_http::window_catalog::{strings, text};

use crate::presentation::display_number;

pub(crate) fn number(value: f64) -> String {
    display_number("", value, None)
}

pub(crate) fn clock(time: DateTime, locale: &str) -> String {
    let hour = time.hour() % 12;
    format!(
        "{}:{:02} {}",
        if hour == 0 { 12 } else { hour },
        time.minute(),
        if time.hour() < 12 {
            text(locale, "journal.window.body.date.am")
        } else {
            text(locale, "journal.window.body.date.pm")
        }
    )
}

pub(crate) fn long_day(day: Date, locale: &str) -> String {
    strings(
        locale,
        "journal.window.body.date.full",
        [
            ("month", &month_full_name(day.month(), locale)),
            ("day", &day.day().to_string()),
            ("year", &day.year().to_string()),
        ],
    )
}

pub(crate) fn parse_day(day: &str) -> Option<Date> {
    solstone_core_time::parse_day(day)
}

/// Each date from `first` through `last`.
pub(crate) fn days_through(first: Date, last: Date) -> impl Iterator<Item = Date> {
    first
        .series(jiff::Span::new().days(1))
        .take_while(move |date| *date <= last)
}

pub(crate) fn short_day(day: &str, locale: &str) -> Option<String> {
    parse_day(day).map(|day| {
        strings(
            locale,
            "journal.window.body.date.short",
            [
                ("month", &month_abbr(day.month(), locale)),
                ("day", &day.day().to_string()),
            ],
        )
    })
}

pub(crate) fn short_day_year(day: &str, locale: &str) -> Option<String> {
    let date = parse_day(day)?;
    Some(strings(
        locale,
        "journal.window.body.date.full",
        [
            ("month", &month_abbr(date.month(), locale)),
            ("day", &date.day().to_string()),
            ("year", &date.year().to_string()),
        ],
    ))
}

pub(crate) fn month_abbr(month: i8, locale: &str) -> String {
    const MONTHS: [&str; 12] = [
        "journal.window.date.months.short.jan",
        "journal.window.date.months.short.feb",
        "journal.window.date.months.short.mar",
        "journal.window.date.months.short.apr",
        "journal.window.date.months.short.may",
        "journal.window.date.months.short.jun",
        "journal.window.date.months.short.jul",
        "journal.window.date.months.short.aug",
        "journal.window.date.months.short.sep",
        "journal.window.date.months.short.oct",
        "journal.window.date.months.short.nov",
        "journal.window.date.months.short.dec",
    ];
    text(locale, MONTHS[month_index(month)])
}

pub(crate) fn month_full_name(month: i8, locale: &str) -> String {
    const MONTHS: [&str; 12] = [
        "journal.window.shared.date.nav.months.label.january",
        "journal.window.shared.date.nav.months.label.february",
        "journal.window.shared.date.nav.months.label.march",
        "journal.window.shared.date.nav.months.label.april",
        "journal.window.shared.date.nav.months.label.may",
        "journal.window.shared.date.nav.months.label.june",
        "journal.window.shared.date.nav.months.label.july",
        "journal.window.shared.date.nav.months.label.august",
        "journal.window.shared.date.nav.months.label.september",
        "journal.window.shared.date.nav.months.label.october",
        "journal.window.shared.date.nav.months.label.november",
        "journal.window.shared.date.nav.months.label.december",
    ];
    text(locale, MONTHS[month_index(month)])
}

fn month_index(month: i8) -> usize {
    usize::try_from(month - 1).expect("a month is 1 to 12")
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
pub(crate) fn count_noun(count: u64, one_id: &str, many_id: &str, locale: &str) -> String {
    strings(
        locale,
        "journal.window.body.count.label",
        [
            ("count", &grouped(count)),
            (
                "noun",
                &text(locale, if count == 1 { one_id } else { many_id }),
            ),
        ],
    )
}
