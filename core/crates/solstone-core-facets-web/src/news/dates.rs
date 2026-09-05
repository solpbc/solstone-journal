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

pub fn format_news_month(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map(|date| date.format("%B %Y").to_string())
        .unwrap_or_else(|_| day.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{format_news_list_date, format_news_month};

    #[test]
    fn invalid_calendar_days_remain_labels() {
        assert_eq!(format_news_list_date("20261332"), "20261332");
        assert_eq!(format_news_month("20261332"), "20261332");
    }
}
