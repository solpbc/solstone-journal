// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A span of time's body rows, counted for the transcripts panel and listed for
//! `sol body window`.
//!
//! A day the owner hid gives none of its windows' rows, and the part of a night
//! or workout that started on it leaves the windows of the days after it. The
//! answer says when the span lies on a hidden day, so a reader can tell hidden
//! from empty.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use chrono_tz::Tz;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_format::content::body_hidden_days::read_hidden_days;
use solstone_core_format::content::body_records::{
    BODY_RECORDS_FILE, BodyRow, RowKind, decode_rows, one_copy_per_row, parse_source_time,
    source_words,
};
use solstone_core_format::content::strava_body::{STRAVA_STREAM, STRAVA_TILE_FILE, strava_rows};

use crate::fmt::{clock, count_noun, grouped};
use crate::hidden::drop_hidden_rows;
use crate::listing::{body_streams, is_deleted, window_names};
use crate::query::decoded_query_params;
use crate::rows::read_window_text;

pub(crate) const MAX_WINDOW_DAYS: i64 = 7;
/// The most rows a span lists; counts are always whole.
const MAX_ROWS: usize = 2000;

const INVALID_BOUNDS: &str = "Window requires valid from and to ISO timestamps.";
const END_BEFORE_START: &str = "Window end must be after window start.";

pub(crate) async fn window_route(
    State(root): State<Arc<PathBuf>>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let params = decoded_query_params(raw_query.as_deref().unwrap_or_default());
    let Some(window_start) = parse_window_bound(params.get("from").map(String::as_str)) else {
        return invalid_window_response(INVALID_BOUNDS);
    };
    let Some(window_end) = parse_window_bound(params.get("to").map(String::as_str)) else {
        return invalid_window_response(INVALID_BOUNDS);
    };
    if window_end <= window_start {
        return invalid_window_response(END_BEFORE_START);
    }
    if window_end - window_start > Duration::days(MAX_WINDOW_DAYS) {
        return invalid_window_response(&format!(
            "Window span must be {MAX_WINDOW_DAYS} days or less."
        ));
    }
    Json(build_window(&root, window_start, window_end)).into_response()
}

fn invalid_window_response(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

/// Parse a stored body time that carries its own offset: RFC 3339, or the
/// Apple Health export form (`2026-01-02 00:00:00 -0700`) its rows keep.
pub(crate) fn parse_offset_time(value: &str) -> Option<DateTime<FixedOffset>> {
    let value = value.trim();
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S %z"))
        .ok()
}

pub(crate) fn parse_window_bound(value: Option<&str>) -> Option<DateTime<FixedOffset>> {
    let mut text = value?.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    if let Some(value) = parse_offset_time(&text) {
        return Some(value);
    }
    if text.ends_with('Z') {
        text.pop();
        text.push_str("+00:00");
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%d %H:%M%:z",
    ] {
        if let Ok(value) = DateTime::parse_from_str(&text, format) {
            return Some(value);
        }
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&text, format) {
            return Some(DateTime::from_naive_utc_and_offset(
                value,
                FixedOffset::east_opt(0).expect("UTC offset exists"),
            ));
        }
    }
    NaiveDate::parse_from_str(&text, "%Y-%m-%d")
        .ok()
        .map(|value| {
            DateTime::from_naive_utc_and_offset(
                value.and_hms_opt(0, 0, 0).expect("midnight exists"),
                FixedOffset::east_opt(0).expect("UTC offset exists"),
            )
        })
}

/// A window's span in time: its key's clock, on the zone the source placed it
/// on. `None` when the key can't be read, or the source has no such record.
fn window_instants(
    zone: Option<Tz>,
    day: &str,
    name: &str,
) -> Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    let zone = zone?;
    let (clock_text, length) = name.split_once('_')?;
    if clock_text.len() != 6 || !clock_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let length = length.split('_').next()?.parse::<i64>().ok()?;
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let time = date.and_hms_opt(
        clock_text[0..2].parse().ok()?,
        clock_text[2..4].parse().ok()?,
        clock_text[4..6].parse().ok()?,
    )?;
    let start = zone.from_local_datetime(&time).earliest()?;
    Some((
        start.fixed_offset(),
        (start + Duration::seconds(length.max(1))).fixed_offset(),
    ))
}

/// One row's place in the span, or `None` when it falls outside it. A row with
/// no time belongs to its day.
fn row_span(
    row: &BodyRow,
    window_day: &str,
    zone: Option<Tz>,
) -> Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    match row.begins().and_then(parse_source_time) {
        Some(start) => {
            let end = row.end.as_deref().and_then(parse_source_time);
            Some((start, end.filter(|end| *end > start).unwrap_or(start)))
        }
        None => {
            let date = NaiveDate::parse_from_str(window_day, "%Y%m%d").ok()?;
            let midnight = date.and_hms_opt(0, 0, 0)?;
            let start = match zone {
                Some(zone) => zone
                    .from_local_datetime(&midnight)
                    .earliest()?
                    .fixed_offset(),
                None => DateTime::from_naive_utc_and_offset(
                    midnight,
                    FixedOffset::east_opt(0).expect("UTC offset exists"),
                ),
            };
            Some((start, start + Duration::days(1)))
        }
    }
}

fn overlaps(
    (start, end): (DateTime<FixedOffset>, DateTime<FixedOffset>),
    from: DateTime<FixedOffset>,
    to: DateTime<FixedOffset>,
) -> bool {
    if end > start {
        start < to && end > from
    } else {
        start >= from && start < to
    }
}

struct Found {
    source: String,
    row: BodyRow,
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
}

/// The day-keyed windows a span could touch: a day each side of it, because a
/// window's day is the source's own and the span's offset isn't.
fn candidate_days(from: DateTime<FixedOffset>, to: DateTime<FixedOffset>) -> Vec<String> {
    let first = (from - Duration::days(1)).date_naive();
    let last = (to + Duration::days(1)).date_naive();
    first
        .iter_days()
        .take_while(|date| *date <= last)
        .map(|date| date.format("%Y%m%d").to_string())
        .collect()
}

/// The days the span covers, on its own clock.
fn span_days(from: DateTime<FixedOffset>, to: DateTime<FixedOffset>) -> BTreeSet<String> {
    let last = (to.with_timezone(from.offset()) - Duration::nanoseconds(1)).date_naive();
    from.date_naive()
        .iter_days()
        .take_while(|date| *date <= last)
        .map(|date| date.format("%Y%m%d").to_string())
        .collect()
}

fn collect_rows(
    root: &Path,
    from: DateTime<FixedOffset>,
    to: DateTime<FixedOffset>,
    hidden: &BTreeSet<String>,
) -> Vec<Found> {
    let mut found = Vec::new();
    let mut took = BTreeSet::new();
    for (stream, source) in body_streams() {
        let zone = solstone_core_journal_config::placed_zone(root, source);
        for day in candidate_days(from, to) {
            if hidden.contains(&day) {
                continue;
            }
            let mut windows = Vec::new();
            for name in window_names(root, &day, stream) {
                if is_deleted(root, &day, stream, &name) {
                    continue;
                }
                if let Some(instants) = window_instants(zone, &day, &name)
                    && !overlaps(instants, from, to)
                {
                    continue;
                }
                let dir = root.join("chronicle").join(&day).join(stream).join(&name);
                let rows = if stream == STRAVA_STREAM {
                    read_window_text(&dir.join(STRAVA_TILE_FILE))
                        .ok()
                        .and_then(|text| strava_rows(&text))
                } else {
                    read_window_text(&dir.join(BODY_RECORDS_FILE))
                        .ok()
                        .and_then(|text| decode_rows(&text).ok())
                };
                if let Some(rows) = rows {
                    windows.push((name, rows));
                }
            }
            let mut rows = one_copy_per_row(windows)
                .into_iter()
                .flat_map(|(_, rows)| rows)
                .collect::<Vec<_>>();
            drop_hidden_rows(root, source, &day, &mut rows, hidden, &mut took);
            for row in rows {
                if let Some(span) =
                    row_span(&row, &day, zone).filter(|span| overlaps(*span, from, to))
                {
                    found.push(Found {
                        source: source.to_owned(),
                        start: span.0,
                        end: span.1,
                        row,
                    });
                }
            }
        }
    }
    found.sort_by_key(|item| item.start.timestamp_millis());
    one_row_per_experience(found)
}

/// An experience has a tile in every window of its span, and each repeats the
/// whole span: it counts once, as its own row or else its first tile in range.
fn one_row_per_experience(found: Vec<Found>) -> Vec<Found> {
    let starts = found
        .iter()
        .filter(|item| item.row.kind == RowKind::Experience)
        .map(|item| (item.source.clone(), item.row.id.clone()))
        .collect::<BTreeSet<_>>();
    let mut tiled = BTreeSet::new();
    found
        .into_iter()
        .filter(|item| {
            if item.row.kind != RowKind::ExperienceTile {
                return true;
            }
            let of = item.row.of.clone().unwrap_or_else(|| item.row.id.clone());
            !starts.contains(&(item.source.clone(), of.clone()))
                && tiled.insert((item.source.clone(), of))
        })
        .collect()
}

/// A workout's event marks a moment inside a workout already counted.
fn is_entry(item: &Found) -> bool {
    item.row.kind != RowKind::Event
}

fn iso(time: DateTime<FixedOffset>) -> String {
    time.to_rfc3339()
}

/// Counts of what the span holds, kind by kind: the only thing the panel states.
fn signals(rows: &[Found]) -> Vec<Value> {
    let mut counts = BTreeMap::<(String, &'static str, &'static str), u64>::new();
    for item in rows {
        let (one, many) = match item.row.kind {
            RowKind::Reading => ("reading", "readings"),
            RowKind::Count => ("entry", "entries"),
            RowKind::Logged => ("entry", "entries"),
            RowKind::Stage => ("interval", "intervals"),
            RowKind::Dated => ("entry", "entries"),
            RowKind::Experience | RowKind::ExperienceTile | RowKind::Event | RowKind::Moved => {
                continue;
            }
        };
        *counts
            .entry((item.row.label.clone(), one, many))
            .or_default() += 1;
    }
    let mut items = counts.into_iter().collect::<Vec<_>>();
    items.sort_by(|((left, ..), a), ((right, ..), b)| {
        b.cmp(a)
            .then_with(|| left.to_lowercase().cmp(&right.to_lowercase()))
    });
    items
        .into_iter()
        .map(|((label, one, many), count)| {
            json!({"label": label, "count": count, "count_label": count_noun(count, one, many)})
        })
        .collect()
}

/// What happened in the span: each experience (or the part of one that runs
/// into it) once, with its source's own times.
fn events(rows: &[Found]) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut events = Vec::new();
    for item in rows {
        let row = &item.row;
        if !matches!(row.kind, RowKind::Experience | RowKind::ExperienceTile) {
            continue;
        }
        let key = row.of.clone().unwrap_or_else(|| row.id.clone());
        if !seen.insert((item.source.clone(), key)) {
            continue;
        }
        let kind = row
            .experience
            .clone()
            .unwrap_or_else(|| "experience".to_owned());
        events.push(json!({
            "kind": kind,
            "label": row.label,
            "source": source_words(&item.source),
            "start": iso(item.start),
            "end": iso(item.end),
            "start_label": clock(item.start.naive_local()),
            "end_label": clock(item.end.naive_local()),
        }));
    }
    events
}

fn sources(rows: &[Found]) -> Value {
    let mut counts = BTreeMap::<&str, u64>::new();
    for item in rows.iter().filter(|item| is_entry(item)) {
        *counts.entry(source_words(&item.source)).or_default() += 1;
    }
    let chips = counts
        .iter()
        .map(|(name, count)| {
            json!({"name": name, "count": count, "count_label": count_noun(*count, "entry", "entries")})
        })
        .collect::<Vec<_>>();
    json!({"names": counts.keys().collect::<Vec<_>>(), "chips": chips})
}

fn rows_listing(rows: &[Found]) -> Vec<Value> {
    rows.iter()
        .take(MAX_ROWS)
        .map(|item| {
            let row = &item.row;
            json!({
                "source": source_words(&item.source),
                "kind": row.kind,
                "label": row.label,
                "experience": row.experience,
                "at": row.at,
                "start": row.start,
                "end": row.end,
                "date": row.date,
                "value": row.value,
                "unit": row.unit,
                "by": row.by,
            })
        })
        .collect()
}

/// One bucket per hour of the span on the request's own clock, with how many
/// entries begin in it. A count, never a value.
fn hourly(rows: &[Found], from: DateTime<FixedOffset>, to: DateTime<FixedOffset>) -> Vec<Value> {
    let offset = *from.offset();
    let mut bucket_start = from
        .naive_local()
        .with_minute(0)
        .and_then(|time| time.with_second(0))
        .and_then(|time| time.with_nanosecond(0))
        .expect("an hour's start is a clock time");
    let mut items = Vec::new();
    let to_local = to.with_timezone(&offset).naive_local();
    while bucket_start < to_local {
        let bucket_end = bucket_start + Duration::hours(1);
        let start = offset
            .from_local_datetime(&bucket_start.max(from.naive_local()))
            .earliest()
            .expect("fixed offset converts");
        let end = offset
            .from_local_datetime(&bucket_end.min(to_local))
            .earliest()
            .expect("fixed offset converts");
        let count = rows
            .iter()
            .filter(|item| is_entry(item) && item.start >= start && item.start < end)
            .count() as u64;
        items.push(json!({
            "start": iso(start),
            "end": iso(end),
            "label": clock(bucket_start),
            "range_label": format!("{}–{}", clock(bucket_start), clock(bucket_end)),
            "has_data": count > 0,
            "entry_total": count,
            "entry_total_label": grouped(count),
            "summary": [],
        }));
        bucket_start = bucket_end;
    }
    items
}

pub(crate) fn build_window(
    root: &Path,
    from: DateTime<FixedOffset>,
    to: DateTime<FixedOffset>,
) -> Value {
    let hidden = read_hidden_days(root).days().cloned().unwrap_or_default();
    let rows = collect_rows(root, from, to, &hidden);
    let on_hidden_day = !hidden.is_disjoint(&span_days(from, to));
    let entry_total = rows.iter().filter(|item| is_entry(item)).count() as u64;
    let span_minutes = ((to - from).num_seconds() as f64 / 60.0 * 10.0).round() / 10.0;
    json!({
        "from": iso(from),
        "to": iso(to),
        "span_minutes": span_minutes,
        "hidden": on_hidden_day,
        "has_data": entry_total > 0,
        "entry_total": entry_total,
        "entry_total_label": grouped(entry_total),
        "signals": signals(&rows),
        "events": events(&rows),
        "sources": sources(&rows),
        "hourly": hourly(&rows, from, to),
        "rows": rows_listing(&rows),
        "rows_truncated": rows.len() > MAX_ROWS,
    })
}
