// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{Datelike, NaiveDate};

pub fn format_news_list_date(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map(|date| {
            date.format("%a %b").to_string() + &format!(" {}, {}", date.day(), date.format("%Y"))
        })
        .unwrap_or_else(|_| day.to_owned())
}

pub fn next_newsletter_when<T>(_today: T) -> &'static str {
    "tomorrow morning"
}
