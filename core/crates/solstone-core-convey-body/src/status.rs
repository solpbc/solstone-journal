// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The overview: `/api/status`, `/api/recent` and `/api/index`'s coverage.
//!
//! These read directory listings, import summaries and bundle manifests, never
//! a window's contents, so they stay fast on a journal of any size.

use solstone_core_convey_http::window_catalog::{response_locale, strings, text};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use solstone_core_convey_http::coalesce::ResponseLocale;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_format::content::body_hidden_days::{HiddenDays, read_hidden_days};

use crate::fmt::{count_label, days_through, month_abbr, parse_day, short_day, short_day_year};
use crate::listing::{BodyDays, DaySources, body_days, body_streams, window_count};
use crate::query::decoded_query_params;
use crate::router::unreadable_response;

const RECENT_DAY_LIMIT: usize = 14;
const RECENT_BATCH_LIMIT_CAP: usize = 100;

fn source_words(source: &str) -> &str {
    solstone_core_format::content::body_records::source_words(source)
}

fn source_item(source: &str, days: &[&String], locale: &str) -> Value {
    let first = days.first().map(|day| day.as_str());
    let last = days.last().map(|day| day.as_str());
    json!({
        "source": source,
        "name": source_words(source),
        "first_day": first,
        "first_label": first.and_then(|day| short_day_year(day, locale)),
        "last_day": last,
        "last_label": last.and_then(|day| short_day_year(day, locale)),
        "days": days.len(),
        "days_label": count_label(days.len() as u64, "journal.window.body.count.day", locale),
    })
}

/// Each source held, with its first and last day and how many days hold it.
fn source_items(days: &DaySources, locale: &str) -> Vec<Value> {
    body_streams()
        .iter()
        .filter_map(|(_, source)| {
            let held = days
                .iter()
                .filter(|(_, sources)| sources.contains(source))
                .map(|(day, _)| day)
                .collect::<Vec<_>>();
            (!held.is_empty()).then(|| source_item(source, &held, locale))
        })
        .collect()
}

fn coverage(days: &DaySources) -> Value {
    match (days.keys().next(), days.keys().next_back()) {
        (Some(start), Some(end)) => json!({"start": start, "end": end}),
        _ => Value::Null,
    }
}

fn grid_cell(
    day: &str,
    sources: Option<&BTreeSet<&'static str>>,
    earlier: Option<bool>,
    hidden: bool,
    locale: &str,
) -> Value {
    let date = short_day_year(day, locale).unwrap_or_default();
    if hidden {
        return json!({
            "day": day,
            "count": 0,
            "intensity": 0.0,
            "title": strings(locale, "journal.window.body.grid.hidden", [("date", &date)]),
            "earlier": false,
            "hidden": true,
        });
    }
    let count = sources.map_or(0, BTreeSet::len);
    let title = match sources {
        Some(sources) => {
            let id = match (
                sources.contains("apple_health"),
                sources.contains("oura"),
                sources.contains("strava"),
            ) {
                (true, false, false) => "journal.window.body.grid.source.apple",
                (false, true, false) => "journal.window.body.grid.source.oura",
                (false, false, true) => "journal.window.body.grid.source.strava",
                (true, true, false) => "journal.window.body.grid.source.apple.oura",
                (true, false, true) => "journal.window.body.grid.source.apple.strava",
                (false, true, true) => "journal.window.body.grid.source.oura.strava",
                (true, true, true) => "journal.window.body.grid.source.all",
                (false, false, false) => "journal.window.body.grid.empty",
            };
            strings(locale, id, [("date", &date)])
        }
        None if earlier == Some(true) => {
            strings(locale, "journal.window.body.grid.held", [("date", &date)])
        }
        None if earlier.is_some() => strings(
            locale,
            "journal.window.body.grid.held.month",
            [("date", &date)],
        ),
        None => strings(locale, "journal.window.body.grid.empty", [("date", &date)]),
    };
    json!({
        "day": day,
        "count": count,
        "intensity": count as f64 / 3.0,
        "title": title,
        "earlier": earlier.is_some() && sources.is_none(),
        "hidden": false,
    })
}

/// The grid runs over every day holding body data, the hidden ones too: a
/// hidden day's cell is how the owner reaches its unhide.
fn day_grid(root: &Path, days: &BodyDays, locale: &str) -> Vec<Value> {
    let held = || days.shown.keys().chain(&days.hidden);
    let (Some(first), Some(last)) = (
        held().min().and_then(|day| parse_day(day)),
        held().max().and_then(|day| parse_day(day)),
    ) else {
        return Vec::new();
    };
    let empty = days_through(first, last)
        .map(solstone_core_time::day_key)
        .filter(|day| !days.shown.contains_key(day) && !days.hidden.contains(day))
        .collect::<Vec<_>>();
    let earlier = solstone_core_location::old_imports::covered_days_among(
        root,
        empty.iter().map(String::as_str),
    );
    (first.year()..=last.year())
        .map(|year| {
            let start = first.max(jiff::civil::date(year, 1, 1));
            let end = last.min(jiff::civil::date(year, 12, 31));
            let mut weeks = Vec::<Vec<Value>>::new();
            let lead = start.weekday().to_monday_zero_offset().unsigned_abs();
            let mut week = vec![Value::Null; usize::from(lead)];
            for current in days_through(start, end) {
                let key = solstone_core_time::day_key(current);
                week.push(grid_cell(
                    &key,
                    days.shown.get(&key),
                    earlier.get(&key).copied(),
                    days.hidden.contains(&key),
                    locale,
                ));
                if week.len() == 7 {
                    weeks.push(std::mem::take(&mut week));
                }
            }
            if !week.is_empty() {
                week.resize(7, Value::Null);
                weeks.push(week);
            }
            json!({"year": year, "weeks": weeks, "month_labels": month_labels(&weeks, locale)})
        })
        .collect()
}

fn month_labels(weeks: &[Vec<Value>], locale: &str) -> Vec<Value> {
    let mut previous = None;
    weeks
        .iter()
        .enumerate()
        .filter_map(|(index, week)| {
            let day = week
                .iter()
                .find_map(|cell| cell.get("day").and_then(Value::as_str))?;
            let month = &day[4..6];
            (previous != Some(month)).then(|| {
                previous = Some(month);
                json!({"index": index, "label": month_abbr(month.parse().expect("month"), locale)})
            })
        })
        .collect()
}

/// The overview. A hidden day is left out of every count, the coverage and the
/// sources, and its cell says it's hidden.
pub(crate) fn build_status(root: &Path, locale: &str) -> Result<Value, String> {
    let hidden = read_hidden_days(root);
    let days = body_days(root, &hidden)?;
    Ok(json!({
        "coverage": coverage(&days.shown),
        "days": days.shown.len(),
        "sources": source_items(&days.shown, locale),
        "day_grid": day_grid(root, &days, locale),
        "imports": crate::audit::imports_section(root, locale),
        "hidden_unreadable": hidden == HiddenDays::Unreadable,
    }))
}

pub(crate) async fn status_route(
    State(root): State<Arc<PathBuf>>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    match build_status(&root, locale) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => unreadable_response(locale, error),
    }
}

pub(crate) fn invalid_day_response(locale: &str) -> Response {
    error_envelope(
        "invalid_day",
        text(locale, "journal.window.body.error.day.invalid"),
        text(locale, "journal.window.body.error.day.detail"),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn invalid_limit_response(locale: &str, detail: String) -> Response {
    error_envelope(
        "invalid_request_value",
        text(locale, "journal.window.body.error.invalid.request"),
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn recent_days(
    root: &Path,
    days: &DaySources,
    before: &str,
    limit: usize,
    locale: &str,
) -> Result<Value, String> {
    let mut chosen = days
        .iter()
        .rev()
        .filter(|(day, _)| day.as_str() < before)
        .map(|(day, sources)| (day.clone(), sources.clone()))
        .take(limit + 1)
        .collect::<Vec<_>>();
    let has_more = chosen.len() > limit;
    chosen.truncate(limit);
    let days = chosen
        .into_iter()
        .map(|(day, sources)| {
            let windows = window_count(root, &day);
            let names = sources
                .iter()
                .map(|source| source_words(source))
                .collect::<Vec<_>>();
            json!({
                "day": day,
                "label": short_day(&day, locale),
                "sources": names,
                "windows": windows,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"days": days, "has_more": has_more}))
}

pub(crate) async fn recent_route(
    State(root): State<Arc<PathBuf>>,
    RawQuery(raw_query): RawQuery,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    let params = decoded_query_params(raw_query.as_deref().unwrap_or_default());
    let before = params.get("before").map_or("", String::as_str);
    if parse_day(before).is_none() {
        return invalid_day_response(locale);
    }
    let limit = match params.get("limit") {
        None => RECENT_DAY_LIMIT,
        Some(raw) => match raw.parse::<i64>() {
            Ok(limit) if limit < 1 => {
                return invalid_limit_response(
                    locale,
                    text(locale, "journal.window.body.error.limit.minimum"),
                );
            }
            Ok(limit) => (limit as usize).min(RECENT_BATCH_LIMIT_CAP),
            Err(_) => {
                return invalid_limit_response(
                    locale,
                    text(locale, "journal.window.body.error.limit.integer"),
                );
            }
        },
    };
    let days = match body_days(&root, &read_hidden_days(&root)) {
        Ok(days) => days.shown,
        Err(error) => return unreadable_response(locale, error),
    };
    match recent_days(&root, &days, before, limit, locale) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => unreadable_response(locale, error),
    }
}

/// `/api/index`: the journal's first and last day with windows, and each month's
/// days, from directory listings alone.
pub(crate) fn index_payload(days: &DaySources) -> Value {
    let mut months = BTreeMap::<String, u64>::new();
    for day in days.keys() {
        *months.entry(day[..6].to_owned()).or_insert(0) += 1;
    }
    json!({"coverage": coverage(days), "months": months})
}

/// `/api/stats/{month}`: each day's live windows, by name.
pub(crate) fn stats_payload(root: &Path, days: &DaySources, month: &str) -> Value {
    let days = days
        .keys()
        .filter(|day| day[..6] == *month)
        .map(|day| (day.clone(), window_count(root, day)))
        .collect::<BTreeMap<_, _>>();
    json!(days)
}
