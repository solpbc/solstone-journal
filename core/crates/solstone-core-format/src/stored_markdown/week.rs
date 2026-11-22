// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Calendar labels shared by a saved week page and its Home view.

use jiff::civil::{Date, Weekday};
use solstone_core_window_catalog::{strings, text};

pub fn month_name(month: i8, locale: &str) -> String {
    let id = match month {
        1 => "journal.window.home.week.month.january",
        2 => "journal.window.home.week.month.february",
        3 => "journal.window.home.week.month.march",
        4 => "journal.window.home.week.month.april",
        5 => "journal.window.home.week.month.may",
        6 => "journal.window.home.week.month.june",
        7 => "journal.window.home.week.month.july",
        8 => "journal.window.home.week.month.august",
        9 => "journal.window.home.week.month.september",
        10 => "journal.window.home.week.month.october",
        11 => "journal.window.home.week.month.november",
        12 => "journal.window.home.week.month.december",
        _ => return String::new(),
    };
    text(locale, id)
}

pub fn weekday_name(date: Date, locale: &str) -> String {
    text(
        locale,
        match date.weekday() {
            Weekday::Monday => "journal.window.home.week.day.long.monday",
            Weekday::Tuesday => "journal.window.home.week.day.long.tuesday",
            Weekday::Wednesday => "journal.window.home.week.day.long.wednesday",
            Weekday::Thursday => "journal.window.home.week.day.long.thursday",
            Weekday::Friday => "journal.window.home.week.day.long.friday",
            Weekday::Saturday => "journal.window.home.week.day.long.saturday",
            Weekday::Sunday => "journal.window.home.week.day.long.sunday",
        },
    )
}

pub(super) fn date_heading(date: Date, locale: &str) -> String {
    strings(
        locale,
        "journal.window.map.date.long",
        [
            ("weekday", &weekday_name(date, locale)),
            ("month", &month_name(date.month(), locale)),
            ("day", &date.day().to_string()),
        ],
    )
}
