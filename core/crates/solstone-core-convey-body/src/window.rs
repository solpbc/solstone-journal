// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A span of time's body rows, counted for the transcripts panel and listed for
//! `sol body window`.
//!
//! A day the owner hid gives none of its windows' rows, and the part of a night
//! or workout that started on it leaves the windows of the days after it. The
//! answer says when the span lies on a hidden day, so a reader can tell hidden
//! from empty.

use solstone_core_convey_http::window_catalog::{response_locale, strings, text};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, ZonedDateTime};
use serde_json::{Value, json};
use solstone_core_convey_http::coalesce::ResponseLocale;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_format::content::body_hidden_days::read_hidden_days;
use solstone_core_format::content::body_records::{
    BODY_RECORDS_FILE, BodyRow, RowKind, decode_rows, one_copy_per_row, parse_source_time,
    source_words,
};
use solstone_core_format::content::strava_body::{STRAVA_STREAM, STRAVA_TILE_FILE, strava_rows};
use solstone_core_time::{Precision, at_offset, from_local, zoned_rfc3339};

use crate::fmt::{clock, count_noun, days_through, grouped};
use crate::hidden::drop_hidden_rows;
use crate::listing::{body_streams, is_deleted, window_names};
use crate::query::decoded_query_params;
use crate::rows::read_window_text;

pub(crate) const MAX_WINDOW_DAYS: i64 = 7;
/// The most rows a span lists; counts are always whole.
const MAX_ROWS: usize = 2000;

pub(crate) async fn window_route(
    State(root): State<Arc<PathBuf>>,
    RawQuery(raw_query): RawQuery,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    let params = decoded_query_params(raw_query.as_deref().unwrap_or_default());
    let Some(window_start) = parse_window_bound(params.get("from").map(String::as_str)) else {
        return invalid_window_response(
            locale,
            text(locale, "journal.window.body.error.window.bounds"),
        );
    };
    let Some(window_end) = parse_window_bound(params.get("to").map(String::as_str)) else {
        return invalid_window_response(
            locale,
            text(locale, "journal.window.body.error.window.bounds"),
        );
    };
    if window_end <= window_start {
        return invalid_window_response(
            locale,
            text(locale, "journal.window.body.error.window.order"),
        );
    }
    if window_end.timestamp() - window_start.timestamp()
        > SignedDuration::from_hours(24 * MAX_WINDOW_DAYS)
    {
        return invalid_window_response(
            locale,
            strings(
                locale,
                "journal.window.body.error.window.length",
                [("days", &MAX_WINDOW_DAYS.to_string())],
            ),
        );
    }
    Json(build_window(&root, &window_start, &window_end, locale)).into_response()
}

fn invalid_window_response(locale: &str, detail: String) -> Response {
    error_envelope(
        "invalid_request_value",
        text(locale, "journal.window.body.error.invalid.request"),
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

/// Parse a stored body time that carries its own offset: RFC 3339, or the
/// Apple Health export form (`2026-01-02 00:00:00 -0700`) its rows keep.
pub(crate) fn parse_offset_time(value: &str) -> Option<ZonedDateTime> {
    let value = value.trim();
    solstone_core_time::parse_rfc3339(value)
        .ok()
        .or_else(|| with_offset("%Y-%m-%d %H:%M:%S", value))
}

/// `text` as a date and time in `format` followed by a numeric offset, read
/// as the journal always has (`-0700`, `-07:00` and ` -07 : 00` all read).
fn with_offset(format: &str, text: &str) -> Option<ZonedDateTime> {
    solstone_core_time::strptime_fixed(&format!("{format}%z"), text).ok()
}

/// A wall-clock time read as UTC.
fn at_utc(time: DateTime) -> Option<ZonedDateTime> {
    Offset::UTC
        .to_timestamp(time)
        .ok()
        .map(|ts| at_offset(ts, Offset::UTC))
}

/// `time` at the offset it has now, without its zone's rules.
fn fixed(time: &ZonedDateTime) -> ZonedDateTime {
    at_offset(time.timestamp(), time.offset())
}

pub(crate) fn parse_window_bound(value: Option<&str>) -> Option<ZonedDateTime> {
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
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Some(value) = with_offset(format, &text) {
            return Some(value);
        }
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(value) = DateTime::strptime(format, &text) {
            return at_utc(value);
        }
    }
    Date::strptime("%Y-%m-%d", &text)
        .ok()
        .and_then(|value| at_utc(value.to_datetime(Time::midnight())))
}

/// A window's span in time: its key's clock, on the zone the source placed it
/// on. `None` when the key can't be read, or the source has no such record.
fn window_instants(
    zone: Option<&TimeZone>,
    day: &str,
    name: &str,
) -> Option<(ZonedDateTime, ZonedDateTime)> {
    let zone = zone?;
    let (clock_text, length) = name.split_once('_')?;
    if clock_text.len() != 6 || !clock_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let length = length.split('_').next()?.parse::<i64>().ok()?;
    let date = solstone_core_time::parse_day(day)?;
    let time = Time::new(
        clock_text[0..2].parse().ok()?,
        clock_text[2..4].parse().ok()?,
        clock_text[4..6].parse().ok()?,
        0,
    )
    .ok()?;
    let start = from_local(zone, date.to_datetime(time)).earliest()?;
    let end = start
        .checked_add(SignedDuration::from_secs(length.max(1)))
        .ok()?;
    Some((fixed(&start), fixed(&end)))
}

/// One row's place in the span, or `None` when it falls outside it. A row with
/// no time belongs to its day.
fn row_span(
    row: &BodyRow,
    window_day: &str,
    zone: Option<&TimeZone>,
) -> Option<(ZonedDateTime, ZonedDateTime)> {
    match row.begins().and_then(parse_source_time) {
        Some(start) => {
            let end = row.end.as_deref().and_then(parse_source_time);
            let end = end
                .filter(|end| *end > start)
                .unwrap_or_else(|| start.clone());
            Some((start, end))
        }
        None => {
            let date = solstone_core_time::parse_day(window_day)?;
            let midnight = date.to_datetime(Time::midnight());
            let start = match zone {
                Some(zone) => fixed(&from_local(zone, midnight).earliest()?),
                None => at_utc(midnight)?,
            };
            let end = start.checked_add(SignedDuration::from_hours(24)).ok()?;
            Some((start, end))
        }
    }
}

fn overlaps(
    (start, end): (&ZonedDateTime, &ZonedDateTime),
    from: &ZonedDateTime,
    to: &ZonedDateTime,
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
    start: ZonedDateTime,
    end: ZonedDateTime,
}

/// The day-keyed windows a span could touch: a day each side of it, because a
/// window's day is the source's own and the span's offset isn't.
fn candidate_days(from: &ZonedDateTime, to: &ZonedDateTime) -> Vec<String> {
    // A day before and after on each bound's own fixed offset: 24 hours exactly.
    let first = from.date().yesterday().unwrap_or_else(|_| from.date());
    let last = to.date().tomorrow().unwrap_or_else(|_| to.date());
    days_through(first, last)
        .map(solstone_core_time::day_key)
        .collect()
}

/// The days the span covers, on its own clock.
fn span_days(from: &ZonedDateTime, to: &ZonedDateTime) -> BTreeSet<String> {
    let Ok(last) = to.timestamp().checked_sub(SignedDuration::from_nanos(1)) else {
        return BTreeSet::new();
    };
    let last = from.offset().to_datetime(last).date();
    days_through(from.date(), last)
        .map(solstone_core_time::day_key)
        .collect()
}

fn collect_rows(
    root: &Path,
    from: &ZonedDateTime,
    to: &ZonedDateTime,
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
                if let Some((start, end)) = window_instants(zone.as_ref(), &day, &name)
                    && !overlaps((&start, &end), from, to)
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
                if let Some((start, end)) = row_span(&row, &day, zone.as_ref())
                    .filter(|(start, end)| overlaps((start, end), from, to))
                {
                    found.push(Found {
                        source: source.to_owned(),
                        start,
                        end,
                        row,
                    });
                }
            }
        }
    }
    found.sort_by_key(|item| item.start.timestamp().as_millisecond());
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

fn iso(time: &ZonedDateTime) -> String {
    zoned_rfc3339(time, Precision::AutoSi, false)
}

/// Counts of what the span holds, kind by kind: the only thing the panel states.
fn signals(rows: &[Found], locale: &str) -> Vec<Value> {
    let mut counts = BTreeMap::<(String, &'static str, &'static str), u64>::new();
    for item in rows {
        let (one, many) = match item.row.kind {
            RowKind::Reading => (
                "journal.window.body.count.reading.one",
                "journal.window.body.count.reading.other",
            ),
            RowKind::Count => (
                "journal.window.body.count.entry.one",
                "journal.window.body.count.entry.other",
            ),
            RowKind::Logged => (
                "journal.window.body.count.entry.one",
                "journal.window.body.count.entry.other",
            ),
            RowKind::Stage => (
                "journal.window.body.count.interval.one",
                "journal.window.body.count.interval.other",
            ),
            RowKind::Dated => (
                "journal.window.body.count.entry.one",
                "journal.window.body.count.entry.other",
            ),
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
            json!({"label": label, "count": count, "count_label": count_noun(count, one, many, locale)})
        })
        .collect()
}

/// What happened in the span: each experience (or the part of one that runs
/// into it) once, with its source's own times.
fn events(rows: &[Found], locale: &str) -> Vec<Value> {
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
            .unwrap_or_else(|| text(locale, "journal.window.body.experience.fallback"));
        events.push(json!({
            "kind": kind,
            "label": row.label,
            "source": source_words(&item.source),
            "start": iso(&item.start),
            "end": iso(&item.end),
            "start_label": clock(item.start.datetime(), locale),
            "end_label": clock(item.end.datetime(), locale),
        }));
    }
    events
}

fn sources(rows: &[Found], locale: &str) -> Value {
    let mut counts = BTreeMap::<&str, u64>::new();
    for item in rows.iter().filter(|item| is_entry(item)) {
        *counts.entry(source_words(&item.source)).or_default() += 1;
    }
    let chips = counts
        .iter()
        .map(|(name, count)| {
            json!({"name": name, "count": count, "count_label": count_noun(*count, "journal.window.body.count.entry.one", "journal.window.body.count.entry.other", locale)})
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
fn hourly(rows: &[Found], from: &ZonedDateTime, to: &ZonedDateTime, locale: &str) -> Vec<Value> {
    let offset = from.offset();
    let from_local = from.datetime();
    let mut bucket_start = from_local.date().at(from_local.hour(), 0, 0, 0);
    let mut items = Vec::new();
    let to_local = offset.to_datetime(to.timestamp());
    let on_offset = |time: DateTime| {
        at_offset(
            offset.to_timestamp(time).expect("fixed offset converts"),
            offset,
        )
    };
    while bucket_start < to_local {
        let bucket_end = bucket_start
            .checked_add(SignedDuration::from_hours(1))
            .expect("an hour's end is a clock time");
        let start = on_offset(bucket_start.max(from_local));
        let end = on_offset(bucket_end.min(to_local));
        let count = rows
            .iter()
            .filter(|item| is_entry(item) && item.start >= start && item.start < end)
            .count() as u64;
        items.push(json!({
            "start": iso(&start),
            "end": iso(&end),
            "label": clock(bucket_start, locale),
            "range_label": format!("{}–{}", clock(bucket_start, locale), clock(bucket_end, locale)),
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
    from: &ZonedDateTime,
    to: &ZonedDateTime,
    locale: &str,
) -> Value {
    let hidden = read_hidden_days(root).days().cloned().unwrap_or_default();
    let rows = collect_rows(root, from, to, &hidden);
    let on_hidden_day = !hidden.is_disjoint(&span_days(from, to));
    let entry_total = rows.iter().filter(|item| is_entry(item)).count() as u64;
    let span_minutes =
        ((to.timestamp() - from.timestamp()).as_secs() as f64 / 60.0 * 10.0).round() / 10.0;
    json!({
        "from": iso(from),
        "to": iso(to),
        "span_minutes": span_minutes,
        "hidden": on_hidden_day,
        "has_data": entry_total > 0,
        "entry_total": entry_total,
        "entry_total_label": grouped(entry_total),
        "signals": signals(&rows, locale),
        "events": events(&rows, locale),
        "sources": sources(&rows, locale),
        "hourly": hourly(&rows, from, to, locale),
        "rows": rows_listing(&rows),
        "rows_truncated": rows.len() > MAX_ROWS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_reads_every_offset_form_it_always_has() {
        for (text, expected) in [
            ("2026-01-10T09:00:00-07:00", "2026-01-10T09:00:00-07:00"),
            ("2026-01-10T09:00:00-0700", "2026-01-10T09:00:00-07:00"),
            (
                "2026-01-10T09:00:00.5+0530",
                "2026-01-10T09:00:00.500+05:30",
            ),
            ("2026-01-10 09:00 -07:00", "2026-01-10T09:00:00-07:00"),
            ("2026-01-10T09:00-07 : 00", "2026-01-10T09:00:00-07:00"),
            ("2026-01-02 00:00:00 -0700", "2026-01-02T00:00:00-07:00"),
            ("2026-01-10T09:00:00.5Z", "2026-01-10T09:00:00.500+00:00"),
            ("2026-01-10T09:00", "2026-01-10T09:00:00+00:00"),
            ("2026-01-10", "2026-01-10T00:00:00+00:00"),
        ] {
            let bound = parse_window_bound(Some(text)).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(iso(&bound), expected, "{text}");
        }
        for text in [
            "2026-01-10T09:00:00+24:00",
            "2026-01-10T09:00:00-07:00:00",
            "2026-01-10T09:00-07",
            "2026-01-10T09:00:00-07:60",
        ] {
            assert!(parse_window_bound(Some(text)).is_none(), "{text}");
        }
    }
}
