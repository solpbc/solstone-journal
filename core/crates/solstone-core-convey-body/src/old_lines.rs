// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the Body app says about old imports: the lines on a day page and the
//! overview's listing of old bundles.

use solstone_core_convey_http::window_catalog::{strings, text};
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_location::old_imports::{
    CoverScope, OldBundle, OldCover, OldSource, OldSpan, covers_on_day,
};

use crate::fmt::{month_full_name, short_day_year};

fn import_again(source: OldSource, locale: &str) -> String {
    match source {
        OldSource::AppleHealth => text(locale, "journal.window.body.import.again.apple.health"),
        OldSource::Oura => text(locale, "journal.window.body.import.again.oura"),
        OldSource::DexcomClarity => text(locale, "journal.window.body.import.again.dexcom"),
    }
}

fn day_line(cover: &OldCover, locale: &str) -> String {
    let source = cover.source.words();
    match &cover.scope {
        CoverScope::Day => strings(
            locale,
            "journal.window.body.import.old.day",
            [
                ("source", source),
                ("action", &import_again(cover.source, locale)),
            ],
        ),
        CoverScope::Month(month) => strings(
            locale,
            "journal.window.body.import.old.month",
            [
                ("source", source),
                ("month", &old_month_words(month, locale)),
                ("action", &import_again(cover.source, locale)),
            ],
        ),
    }
}

/// The day page's lines: one per source an old import covers and the new path
/// hasn't touched that day.
pub(crate) fn day_lines(root: &Path, day: &str, locale: &str) -> Vec<Value> {
    covers_on_day(root, day, None)
        .iter()
        .map(|cover| {
            json!({
                "source": cover.source.words(),
                "scope": match cover.scope {
                    CoverScope::Day => "day",
                    CoverScope::Month(_) => "month",
                },
                "line": day_line(cover, locale),
            })
        })
        .collect()
}

fn old_month_words(value: &str, locale: &str) -> String {
    let Ok(date) = jiff::civil::Date::strptime("%Y-%m-%d", format!("{value}-01")) else {
        return value.to_owned();
    };
    strings(
        locale,
        "journal.window.body.date.month.year",
        [
            ("month", &month_full_name(date.month(), locale)),
            ("year", &format!("{:04}", date.year())),
        ],
    )
}

fn span_label(span: &OldSpan, locale: &str) -> String {
    match span {
        OldSpan::Days(days) => {
            let first = days.iter().next().cloned().unwrap_or_default();
            let last = days.iter().next_back().cloned().unwrap_or_default();
            let first_label = short_day_year(&first, locale).unwrap_or(first);
            if days.len() == 1 {
                first_label
            } else {
                let last_label = short_day_year(&last, locale).unwrap_or(last);
                strings(
                    locale,
                    "journal.window.body.import.span.days",
                    [
                        ("first", &first_label),
                        ("last", &last_label),
                        ("count", &days.len().to_string()),
                    ],
                )
            }
        }
        OldSpan::Months(months) => {
            let words = |month: &str| old_month_words(month, locale);
            match (months.iter().next(), months.iter().next_back()) {
                (Some(only), Some(last)) if only == last => strings(
                    locale,
                    "journal.window.body.import.span.month",
                    [("month", &words(only))],
                ),
                (Some(first), Some(last)) => strings(
                    locale,
                    "journal.window.body.import.span.months",
                    [
                        ("first", &words(first)),
                        ("last", &words(last)),
                        ("count", &months.len().to_string()),
                    ],
                ),
                _ => text(locale, "journal.window.body.import.span.unknown"),
            }
        }
    }
}

fn scope_word(span: &OldSpan) -> &'static str {
    match span {
        OldSpan::Days(_) => "days",
        OldSpan::Months(_) => "months",
    }
}

/// One old bundle on the overview: its source, what it spans and what to do.
pub(crate) fn listing_item(bundle: &OldBundle, locale: &str) -> Value {
    let span = span_label(&bundle.span, locale);
    let line = strings(
        locale,
        "journal.window.body.import.old.listing",
        [
            ("source", bundle.source.words()),
            ("span", &span),
            ("action", &import_again(bundle.source, locale)),
        ],
    );
    json!({
        "id": bundle.id,
        "source": bundle.source.words(),
        "span_by": scope_word(&bundle.span),
        "span": span,
        "line": line,
        "importer": bundle.source != OldSource::DexcomClarity,
    })
}
