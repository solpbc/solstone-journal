// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The overview: `/api/status`, `/api/recent` and `/api/index`'s coverage.
//!
//! These read directory listings, import summaries and bundle manifests, never
//! a window's contents, so they stay fast on a journal of any size.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::fmt::{count_noun, grouped, month_abbr, parse_day, short_day, short_day_year};
use crate::listing::{body_days, body_streams, window_count};
use crate::query::decoded_query_params;
use crate::router::unreadable_response;

const RECENT_DAY_LIMIT: usize = 14;
const RECENT_BATCH_LIMIT_CAP: usize = 100;

/// Every body day's sources, and how many a day holds at most, for the grid.
pub(crate) type BodyDays = BTreeMap<String, BTreeSet<&'static str>>;

fn source_words(source: &str) -> &str {
    solstone_core_format::content::body_records::source_words(source)
}

fn source_item(source: &str, days: &[&String]) -> Value {
    let first = days.first().map(|day| day.as_str());
    let last = days.last().map(|day| day.as_str());
    json!({
        "source": source,
        "name": source_words(source),
        "first_day": first,
        "first_label": first.and_then(short_day_year),
        "last_day": last,
        "last_label": last.and_then(short_day_year),
        "days": days.len(),
        "days_label": count_noun(days.len() as u64, "day", "days"),
    })
}

/// Each source held, with its first and last day and how many days hold it.
fn source_items(days: &BodyDays) -> Vec<Value> {
    body_streams()
        .iter()
        .filter_map(|(_, source)| {
            let held = days
                .iter()
                .filter(|(_, sources)| sources.contains(source))
                .map(|(day, _)| day)
                .collect::<Vec<_>>();
            (!held.is_empty()).then(|| source_item(source, &held))
        })
        .collect()
}

fn coverage(days: &BodyDays) -> Value {
    match (days.keys().next(), days.keys().next_back()) {
        (Some(start), Some(end)) => json!({"start": start, "end": end}),
        _ => Value::Null,
    }
}

fn grid_cell(day: &str, sources: Option<&BTreeSet<&'static str>>) -> Value {
    let count = sources.map_or(0, BTreeSet::len);
    let title = match sources {
        Some(sources) => {
            let names = sources
                .iter()
                .map(|source| source_words(source))
                .collect::<Vec<_>>()
                .join(" and ");
            format!("{} · {names}", short_day_year(day).unwrap_or_default())
        }
        None => format!("{} · nothing here", short_day_year(day).unwrap_or_default()),
    };
    json!({
        "day": day,
        "count": count,
        "intensity": count as f64 / 3.0,
        "title": title,
    })
}

fn day_grid(days: &BodyDays) -> Vec<Value> {
    let (Some(first), Some(last)) = (
        days.keys().next().and_then(|day| parse_day(day)),
        days.keys().next_back().and_then(|day| parse_day(day)),
    ) else {
        return Vec::new();
    };
    (first.year()..=last.year())
        .map(|year| {
            let start = first.max(NaiveDate::from_ymd_opt(year, 1, 1).unwrap());
            let end = last.min(NaiveDate::from_ymd_opt(year, 12, 31).unwrap());
            let mut weeks = Vec::<Vec<Value>>::new();
            let mut week = vec![Value::Null; start.weekday().num_days_from_monday() as usize];
            let mut current = start;
            while current <= end {
                let key = current.format("%Y%m%d").to_string();
                week.push(grid_cell(&key, days.get(&key)));
                if week.len() == 7 {
                    weeks.push(std::mem::take(&mut week));
                }
                current = current.succ_opt().expect("valid date successor");
            }
            if !week.is_empty() {
                week.resize(7, Value::Null);
                weeks.push(week);
            }
            json!({"year": year, "weeks": weeks, "month_labels": month_labels(&weeks)})
        })
        .collect()
}

fn month_labels(weeks: &[Vec<Value>]) -> Vec<Value> {
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
                json!({"index": index, "label": month_abbr(month.parse().expect("month"))})
            })
        })
        .collect()
}

pub(crate) fn build_status(root: &Path) -> Result<Value, String> {
    let days = body_days(root)?;
    Ok(json!({
        "coverage": coverage(&days),
        "latest_day": days.keys().next_back(),
        "days": days.len(),
        "days_label": grouped(days.len() as u64),
        "sources": source_items(&days),
        "day_grid": day_grid(&days),
        "imports": crate::audit::imports_section(root),
    }))
}

pub(crate) async fn status_route(State(root): State<Arc<PathBuf>>) -> Response {
    match build_status(&root) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => unreadable_response(error),
    }
}

pub(crate) fn invalid_day_response() -> Response {
    error_envelope(
        "invalid_day",
        "that day couldn't be used.",
        "Invalid day",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn invalid_limit_response(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn recent_days(
    root: &Path,
    days: &BodyDays,
    before: &str,
    limit: usize,
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
                "label": short_day(&day),
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
) -> Response {
    let params = decoded_query_params(raw_query.as_deref().unwrap_or_default());
    let before = params.get("before").map_or("", String::as_str);
    if parse_day(before).is_none() {
        return invalid_day_response();
    }
    let limit = match params.get("limit") {
        None => RECENT_DAY_LIMIT,
        Some(raw) => match raw.parse::<i64>() {
            Ok(limit) if limit < 1 => return invalid_limit_response("limit must be at least 1"),
            Ok(limit) => (limit as usize).min(RECENT_BATCH_LIMIT_CAP),
            Err(_) => return invalid_limit_response("limit must be an integer"),
        },
    };
    let days = match body_days(&root) {
        Ok(days) => days,
        Err(error) => return unreadable_response(error),
    };
    match recent_days(&root, &days, before, limit) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => unreadable_response(error),
    }
}

/// `/api/index`: the journal's first and last day with windows, and each month's
/// days, from directory listings alone.
pub(crate) fn index_payload(days: &BodyDays) -> Value {
    let mut months = BTreeMap::<String, u64>::new();
    for day in days.keys() {
        *months.entry(day[..6].to_owned()).or_insert(0) += 1;
    }
    json!({"coverage": coverage(days), "months": months})
}

/// `/api/stats/{month}`: each day's live windows, by name.
pub(crate) fn stats_payload(root: &Path, days: &BodyDays, month: &str) -> Value {
    let days = days
        .keys()
        .filter(|day| day[..6] == *month)
        .map(|day| (day.clone(), window_count(root, day)))
        .collect::<BTreeMap<_, _>>();
    json!(days)
}
