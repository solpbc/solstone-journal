// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One body day, as its windows hold it.
//!
//! Per source: what happened, each reading kind's count and first and last
//! time, each count kind's total, logged entries, and the dated documents'
//! names. Nothing here computes a minimum, maximum, average or comparison.
//!
//! A day the owner hid shows only that it's hidden. On another day, the part of
//! a night or workout that started on a hidden day is left out, and the day
//! says which hidden day took it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use chrono_tz::Tz;
use serde_json::{Value, json};
use solstone_core_format::content::body_hidden_days::{HiddenDays, read_hidden_days};
use solstone_core_format::content::body_records::{
    BODY_RECORD_STREAMS, BodyRow, RowKind, parse_source_time, source_words, user_entered,
};
use solstone_core_location::body_records::{LINE_VALUE, day_faces, line_distance};

use crate::fmt::{clock, count_noun, grouped, long_day, parse_day, short_day, short_day_year};
use crate::hidden::drop_hidden_rows;
use crate::listing::{DaySources, body_days};
use crate::old_lines::day_lines;
use crate::presentation::display_value;
use crate::rows::day_rows;
use crate::status::invalid_day_response;
use crate::strava::workouts_on;

pub(crate) async fn day_route(
    State(root): State<Arc<PathBuf>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    if parse_day(&day).is_none() {
        return invalid_day_response();
    }
    match build_day(&root, &day) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => crate::router::unreadable_response(error),
    }
}

fn on_clock(time: DateTime<FixedOffset>, zone: Option<Tz>) -> NaiveDateTime {
    zone.map_or_else(
        || time.naive_local(),
        |zone| time.with_timezone(&zone).naive_local(),
    )
}

/// A row's own time as the owner reads it: the date only when it isn't this day.
fn time_words(text: Option<&str>, day: NaiveDate, zone: Option<Tz>) -> Option<String> {
    let local = on_clock(parse_source_time(text?)?, zone);
    let label = clock(local);
    if local.date() == day {
        Some(label)
    } else {
        Some(format!(
            "{}, {label}",
            local.format("%b %-d").to_string().to_lowercase()
        ))
    }
}

/// `(stream, row id)` → the title the owner gave that account, from the day's
/// composed faces. A failure here costs only the owner's titles.
fn owner_titles(root: &Path, day: &str) -> BTreeMap<(String, String), String> {
    let Ok(faces) = day_faces(root, day) else {
        return BTreeMap::new();
    };
    let mut titles = BTreeMap::new();
    for face in &faces {
        let Some(title) = face
            .anchor
            .owner
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        else {
            continue;
        };
        for member in &face.members {
            if let Some((stream, row_id)) = member.key.split_once('\u{1f}') {
                titles.insert((stream.to_owned(), row_id.to_owned()), title.to_owned());
            }
        }
    }
    titles
}

#[derive(Default)]
struct Tally {
    count: u64,
    first: Option<DateTime<FixedOffset>>,
    last: Option<DateTime<FixedOffset>>,
}

impl Tally {
    fn add(&mut self, row: &BodyRow) {
        self.count += 1;
        if let Some(time) = row.begins().and_then(parse_source_time) {
            self.first = Some(self.first.map_or(time, |first| first.min(time)));
            self.last = Some(self.last.map_or(time, |last| last.max(time)));
        }
    }

    fn time_labels(&self, zone: Option<Tz>) -> (Option<String>, Option<String>) {
        let label =
            |time: Option<DateTime<FixedOffset>>| time.map(|time| clock(on_clock(time, zone)));
        (label(self.first), label(self.last))
    }
}

struct CountSum {
    label: String,
    record_type: String,
    unit: Option<String>,
    by: Option<String>,
    count: u64,
    total: Option<f64>,
}

fn experience_item(
    row: &BodyRow,
    day: NaiveDate,
    zone: Option<Tz>,
    title: Option<&String>,
    continuing: bool,
) -> Value {
    let kind = row.experience.as_deref().unwrap_or("experience");
    let distance = row
        .values
        .get(LINE_VALUE)
        .and_then(|value| line_distance(value));
    json!({
        "kind": kind,
        "name": title.cloned().unwrap_or_else(|| row.label.clone()),
        "start": time_words(row.start.as_deref(), day, zone),
        "end": time_words(row.end.as_deref(), day, zone),
        "by": row.by,
        "distance": distance,
        "continuing": continuing,
    })
}

fn logged_who(row: &BodyRow) -> Option<String> {
    let by = row.by.as_deref().map(str::trim).filter(|by| !by.is_empty());
    match (user_entered(row), by) {
        (true, Some(by)) => Some(format!("you, in {by}")),
        (true, None) => Some("you".to_owned()),
        (false, Some(by)) => Some(by.to_owned()),
        (false, None) => None,
    }
}

/// One source's day, as the page shows it.
fn source_section(
    stream: &str,
    source: &str,
    rows: &[BodyRow],
    day: NaiveDate,
    zone: Option<Tz>,
    titles: &BTreeMap<(String, String), String>,
) -> Value {
    let starts_today: BTreeSet<&str> = rows
        .iter()
        .filter(|row| row.kind == RowKind::Experience)
        .map(|row| row.id.as_str())
        .collect();
    let mut experiences = Vec::new();
    let mut continuing = Vec::new();
    let mut seen_continuing = BTreeSet::new();
    let mut readings: BTreeMap<(String, Option<String>), Tally> = BTreeMap::new();
    let mut counts: BTreeMap<(String, Option<String>, Option<String>), CountSum> = BTreeMap::new();
    let mut logged: BTreeMap<(String, Option<String>), Tally> = BTreeMap::new();
    let mut documents = Vec::new();
    for row in rows {
        match row.kind {
            RowKind::Experience => {
                let title = titles.get(&(stream.to_owned(), row.id.clone()));
                experiences.push((
                    row.begins().and_then(parse_source_time),
                    experience_item(row, day, zone, title, false),
                ));
            }
            RowKind::ExperienceTile => {
                let of = row.of.as_deref().unwrap_or_default();
                if !starts_today.contains(of) && seen_continuing.insert(of.to_owned()) {
                    let title = titles.get(&(stream.to_owned(), of.to_owned()));
                    continuing.push((
                        row.begins().and_then(parse_source_time),
                        experience_item(row, day, zone, title, true),
                    ));
                }
            }
            RowKind::Reading => {
                readings
                    .entry((row.label.clone(), row.by.clone()))
                    .or_default()
                    .add(row);
            }
            RowKind::Stage => {
                readings
                    .entry((format!("sleep stage {}", row.label), row.by.clone()))
                    .or_default()
                    .add(row);
            }
            RowKind::Count => {
                let entry = counts
                    .entry((row.label.clone(), row.by.clone(), row.unit.clone()))
                    .or_insert_with(|| CountSum {
                        label: row.label.clone(),
                        record_type: row.record_type.clone(),
                        unit: row.unit.clone(),
                        by: row.by.clone(),
                        count: 0,
                        total: Some(0.0),
                    });
                entry.count += 1;
                entry.total = entry.total.and_then(|total| {
                    let value = row.value.as_deref()?.trim().parse::<f64>().ok()?;
                    value.is_finite().then_some(total + value)
                });
            }
            RowKind::Logged => {
                logged
                    .entry((row.label.clone(), logged_who(row)))
                    .or_default()
                    .add(row);
            }
            RowKind::Dated => documents.push(json!({"name": row.label, "value": row.value})),
            RowKind::Event | RowKind::Moved => {}
        }
    }
    experiences.sort_by_key(|(time, _)| time.map(|time| time.timestamp_millis()));
    continuing.sort_by_key(|(time, _)| time.map(|time| time.timestamp_millis()));
    let experiences = continuing
        .into_iter()
        .chain(experiences)
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    documents.sort_by_key(|item| item["name"].as_str().unwrap_or_default().to_lowercase());
    let readings = readings
        .iter()
        .map(|((name, by), tally)| {
            let (first, last) = tally.time_labels(zone);
            json!({
                "name": name,
                "count": tally.count,
                "count_label": count_noun(tally.count, "reading", "readings"),
                "first": first,
                "last": last,
                "by": by,
            })
        })
        .collect::<Vec<_>>();
    let counts = counts
        .values()
        .map(|sum| {
            json!({
                "name": sum.label,
                "total": sum
                    .total
                    .map(|total| display_value(&sum.record_type, total, sum.unit.as_deref())),
                "count": sum.count,
                "count_label": count_noun(sum.count, "entry", "entries"),
                "by": sum.by,
            })
        })
        .collect::<Vec<_>>();
    let logged = logged
        .iter()
        .map(|((name, who), tally)| {
            let (first, last) = tally.time_labels(zone);
            json!({
                "name": name,
                "count": tally.count,
                "count_label": count_noun(tally.count, "entry", "entries"),
                "first": first,
                "last": last,
                "who": who,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "source": source,
        "name": source_words(source),
        "experiences": experiences,
        "readings": readings,
        "counts": counts,
        "logged": logged,
        "documents": documents,
    })
}

fn nearest(days: &DaySources, day: &str) -> Value {
    let prev = days.keys().rev().find(|key| key.as_str() < day);
    let next = days.keys().find(|key| key.as_str() > day);
    json!({
        "prev": prev,
        "prev_label": prev.and_then(|value| short_day(value)),
        "next": next,
        "next_label": next.and_then(|value| short_day(value)),
    })
}

pub(crate) fn build_day(root: &Path, day: &str) -> Result<Value, String> {
    let date = parse_day(day).ok_or_else(|| "invalid day".to_owned())?;
    let store = read_hidden_days(root);
    let hidden_unreadable = store == HiddenDays::Unreadable;
    let hidden = store.days().cloned().unwrap_or_default();
    let days = body_days(root, &store)?.shown;
    if hidden.contains(day) {
        return Ok(json!({
            "day": day,
            "day_label": long_day(date),
            "hidden": true,
            "hidden_unreadable": false,
            "hidden_with": [],
            "has_data": false,
            "nearest": nearest(&days, day),
        }));
    }
    let (mut by_source, unreadable) = day_rows(root, day);
    let mut took = BTreeSet::new();
    for (source, rows) in &mut by_source {
        drop_hidden_rows(root, source, day, rows, &hidden, &mut took);
    }
    by_source.retain(|_, rows| !rows.is_empty());
    let (strava, strava_took) = workouts_on(root, day, &hidden);
    took.extend(strava_took);
    let titles = owner_titles(root, day);
    let entry_total = by_source
        .values()
        .flatten()
        .filter(|row| !matches!(row.kind, RowKind::ExperienceTile | RowKind::Event))
        .count() as u64;
    let sources = BODY_RECORD_STREAMS
        .iter()
        .filter_map(|stream| {
            let source = stream.strip_prefix("import.").unwrap_or(stream);
            let rows = by_source.get(source)?;
            let zone = solstone_core_journal_config::placed_zone(root, source);
            Some(source_section(stream, source, rows, date, zone, &titles))
        })
        .collect::<Vec<_>>();
    let has_data = !sources.is_empty() || !strava.is_empty() || unreadable > 0;
    let hidden_with = took
        .iter()
        .map(|day| json!({"day": day, "label": short_day_year(day)}))
        .collect::<Vec<_>>();
    Ok(json!({
        "day": day,
        "day_label": long_day(date),
        "hidden": false,
        "hidden_unreadable": hidden_unreadable,
        "hidden_with": hidden_with,
        "has_data": has_data,
        "entry_total": entry_total,
        "entry_total_label": grouped(entry_total),
        "sources": sources,
        "strava": strava,
        "unreadable_windows": unreadable,
        "earlier_imports": day_lines(root, day),
        "nearest": nearest(&days, day),
    }))
}
