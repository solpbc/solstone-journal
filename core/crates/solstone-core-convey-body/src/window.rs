// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Body time-window payloads backed directly by normalized shards.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, Timelike};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::day::{
    clock, duration, family, grouped_signed as grouped_i64, grouped_unsigned, is_sleep_type,
    number, resolve_canonical_rows, round1, source_label, string_field, valid_day, value_number,
    workout_item,
};
use crate::query::decoded_query_params;
use crate::{
    MonthReader, NormalizedRow, SLEEP_SESSION_GAP_MINUTES, ShardReadError, friendly_type_name,
    friendly_unit_label, pick_day_sleep,
};

pub(crate) const MAX_WINDOW_DAYS: i64 = 7;
const FAMILY_ORDER: [&str; 10] = [
    "Sleep",
    "Glucose",
    "Recovery",
    "Activity",
    "Heart",
    "Mindfulness",
    "Hearing & audio",
    "Walking metrics",
    "Body measurements",
    "Other",
];

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
    match build_window(&root, window_start, window_end) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => crate::router::unavailable_response(
            crate::router::StoreError::ShardUnreadable(error.to_string()),
        ),
    }
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

pub(crate) fn parse_window_bound(value: Option<&str>) -> Option<DateTime<FixedOffset>> {
    let mut text = value?.trim().to_owned();
    if text.is_empty() {
        return None;
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

pub(crate) fn build_window(
    root: &Path,
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
) -> Result<Value, ShardReadError> {
    let rows = rows_for_window(root, window_start, window_end)?;
    let heart_rate = window_heart_rate(&rows);
    let glucose = window_glucose(&rows);
    let steps = window_steps(&rows);
    let workouts = workout_window_items(&rows, window_start, window_end);
    let events = window_events(&rows, window_start, window_end);
    let brief = window_brief(&heart_rate, &glucose, &steps, &workouts);
    let span_minutes = round1((window_end - window_start).num_seconds() as f64 / 60.0);
    Ok(json!({
        "from": iso(window_start), "to": iso(window_end), "span_minutes": span_minutes,
        "has_data": !rows.is_empty(), "entry_total": rows.len(), "entry_total_label": grouped(rows.len()),
        "families": window_family_items(&rows), "signals": window_signal_items(&rows),
        "heart_rate": heart_rate, "glucose": glucose, "steps": steps,
        "workouts": workouts, "events": events,
        "hourly": window_hourly_items(&rows, window_start, window_end, &events),
        "sources": window_sources(&rows), "brief": brief, "brief_label": brief.join(" · "),
    }))
}

fn rows_for_window(
    root: &Path,
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
) -> Result<Vec<NormalizedRow>, ShardReadError> {
    let mut reader = MonthReader::new(root);
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for month in month_keys_between(window_start - Duration::days(1), window_end) {
        for row in reader.read_month(&month)?.iter() {
            if let Some(key) = string_field(&row.dedupe_key).filter(|key| !key.is_empty())
                && !seen.insert(key.to_owned())
            {
                continue;
            }
            let Some((start, end)) = row_interval(row) else {
                continue;
            };
            if interval_overlaps(start, end, window_start, window_end) {
                rows.push(row.clone());
            }
        }
    }
    let mut undated = Vec::new();
    let mut by_day = BTreeMap::<String, Vec<NormalizedRow>>::new();
    for row in rows {
        match string_field(&row.day).filter(|day| valid_day(day)) {
            Some(day) => by_day.entry(day.to_owned()).or_default().push(row),
            None => undated.push(row),
        }
    }
    for rows in by_day.into_values() {
        undated.extend(resolve_canonical_rows(rows));
    }
    undated.sort_by_key(row_timestamp);
    Ok(undated)
}

fn window_family_items(rows: &[NormalizedRow]) -> Vec<Value> {
    let mut counts = BTreeMap::<&str, usize>::new();
    for row in rows {
        let record_type = string_field(&row.record_type).unwrap_or_default();
        if is_audit_only_oura(record_type) {
            continue;
        }
        *counts.entry(family(record_type)).or_default() += 1;
    }
    FAMILY_ORDER
        .into_iter()
        .filter_map(|name| {
            counts
                .get(name)
                .map(|count| json!({"name":name,"count":count,"count_label":grouped(*count)}))
        })
        .collect()
}

fn window_signal_items(rows: &[NormalizedRow]) -> Vec<Value> {
    let mut counts = BTreeMap::<String, usize>::new();
    for row in rows {
        let record_type = string_field(&row.record_type).unwrap_or_default();
        if !is_audit_only_oura(record_type) {
            *counts.entry(friendly_type_name(record_type)).or_default() += 1;
        }
    }
    let mut items = counts.into_iter().collect::<Vec<_>>();
    items.sort_by(|(left_name, left_count), (right_name, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_name.cmp(right_name))
    });
    items
        .into_iter()
        .map(|(label, count)| json!({"label":label,"count":count,"count_label":grouped(count)}))
        .collect()
}

fn window_heart_rate(rows: &[NormalizedRow]) -> Value {
    let heart_rows = rows
        .iter()
        .filter(|row| {
            matches!(
                string_field(&row.record_type),
                Some("HKQuantityTypeIdentifierHeartRate" | "oura.heartrate")
            )
        })
        .collect::<Vec<_>>();
    let values = heart_rows
        .iter()
        .filter_map(|row| value_number(row))
        .collect::<Vec<_>>();
    if values.is_empty() {
        return json!({"count":0,"min":null,"max":null,"unit":null,"label":null});
    }
    let units = heart_rows
        .iter()
        .filter_map(|row| string_field(&row.unit))
        .filter(|unit| !unit.is_empty())
        .collect::<BTreeSet<_>>();
    let unit = if units.len() == 1 {
        units.iter().next().copied()
    } else if units.is_empty() {
        None
    } else {
        Some("mixed")
    };
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = if low == high {
        number(low)
    } else {
        format!("{}–{}", number(low), number(high))
    };
    let display_unit = friendly_unit_label("HKQuantityTypeIdentifierHeartRate", unit);
    let label = display_unit.map_or(range.clone(), |unit| format!("{range} {unit}"));
    json!({"count":values.len(),"count_label":grouped(values.len()),"min":low,"max":high,"unit":unit,"label":label})
}

fn window_glucose(rows: &[NormalizedRow]) -> Value {
    let mut readings = rows
        .iter()
        .filter_map(|row| {
            let record_type = string_field(&row.record_type).unwrap_or_default();
            (family(record_type) == "Glucose").then_some(())?;
            let value = value_number(row)?;
            let time = row_timestamp(row)?;
            let unit = string_field(&row.unit).filter(|unit| !unit.is_empty());
            Some((time, value, unit.map(str::to_owned), source_label(row)))
        })
        .collect::<Vec<_>>();
    readings.sort_by_key(|(time, _, _, _)| *time);
    if readings.is_empty() {
        return json!({"count":0,"readings":[],"unit":null,"delta_label":null,"range_label":null,"min":null,"max":null});
    }
    let units = readings
        .iter()
        .filter_map(|(_, _, unit, _)| unit.as_deref())
        .collect::<BTreeSet<_>>();
    let unit = if units.len() == 1 {
        units.iter().next().copied()
    } else if units.is_empty() {
        None
    } else {
        Some("mixed")
    };
    let items = readings.iter().map(|(time, value, item_unit, source)| json!({"time":clock(time.naive_local()),"iso":iso(*time),"value":value,"value_label":number(*value),"unit":item_unit,"source":source})).collect::<Vec<_>>();
    let low = readings
        .iter()
        .map(|(_, value, _, _)| *value)
        .fold(f64::INFINITY, f64::min);
    let high = readings
        .iter()
        .map(|(_, value, _, _)| *value)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = if low == high {
        number(low)
    } else {
        format!("{}–{}", number(low), number(high))
    };
    let range = unit
        .filter(|unit| *unit != "mixed")
        .map_or(range.clone(), |unit| format!("{range} {unit}"));
    let first = items.first().cloned().expect("non-empty readings");
    let last = items.last().cloned().expect("non-empty readings");
    let mut delta = format!(
        "{} → {}",
        first["value_label"].as_str().expect("value label"),
        last["value_label"].as_str().expect("value label")
    );
    if let Some(unit) = unit.filter(|unit| *unit != "mixed") {
        delta.push(' ');
        delta.push_str(unit);
    }
    json!({"count":items.len(),"count_label":grouped(items.len()),"readings":items,"unit":unit,"delta_label":delta,"range_label":range,"first":first,"last":last,"min":low,"max":high})
}

fn window_steps(rows: &[NormalizedRow]) -> Value {
    let step_rows = rows
        .iter()
        .filter(|row| {
            string_field(&row.record_type).is_some_and(|type_name| type_name.contains("StepCount"))
        })
        .collect::<Vec<_>>();
    if step_rows.is_empty() {
        return json!({"samples":0,"mode":"none","label":null});
    }
    let sources = step_rows
        .iter()
        .map(|row| source_label(row))
        .collect::<BTreeSet<_>>();
    let values = step_rows
        .iter()
        .filter_map(|row| value_number(row))
        .collect::<Vec<_>>();
    if sources.len() == 1 && !values.is_empty() {
        let total = values.iter().sum::<f64>().round() as i64;
        let source = sources.into_iter().next().expect("one source");
        return json!({"mode":"total","samples":step_rows.len(),"samples_label":grouped(step_rows.len()),"total":total,"total_label":grouped_signed(total),"source":source,"label":format!("{} steps", grouped_signed(total))});
    }
    json!({"mode":"samples","samples":step_rows.len(),"samples_label":grouped(step_rows.len()),"label":format!("{} step samples", grouped(step_rows.len()))})
}

fn workout_window_items(
    rows: &[NormalizedRow],
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
) -> Vec<Value> {
    let mut items = rows.iter().filter_map(|row| {
        (string_field(&row.kind) == Some("workout")).then_some(())?;
        let (start, end) = row_interval(row)?;
        let minutes = overlap_minutes(start, end, window_start, window_end);
        (minutes > 0.0).then_some(())?;
        let item = workout_item(row);
        Some(json!({"name":item["name"],"start":iso(start),"end":iso(end),"start_label":clock(start.naive_local()),"end_label":clock(end.naive_local()),"overlap_minutes":round1(minutes),"overlap_label":duration(minutes),"duration_label":item["duration"],"source":item["source"],"distance":item["distance"],"energy":item["energy"],"metric_labels":item["metric_labels"],"metrics_label":item["metrics_label"]}))
    }).collect::<Vec<_>>();
    items.sort_by_key(|item| {
        item["start"]
            .as_str()
            .and_then(|value| parse_window_bound(Some(value)))
    });
    items
}

fn window_events(
    rows: &[NormalizedRow],
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
) -> Vec<Value> {
    let mut events = workout_window_items(rows, window_start, window_end).into_iter().map(|item| json!({"kind":"workout","label":item["name"],"start":item["start"],"end":item["end"],"start_label":item["start_label"],"end_label":item["end_label"],"overlap_minutes":item["overlap_minutes"],"overlap_label":item["overlap_label"],"metric_labels":item["metric_labels"],"metrics_label":item["metrics_label"],"distance":item["distance"],"energy":item["energy"],"source":item["source"]})).collect::<Vec<_>>();
    let mut by_source =
        BTreeMap::<String, Vec<(NaiveDateTime, NaiveDateTime, Option<String>)>>::new();
    let mut fixed_intervals = BTreeMap::<
        String,
        Vec<(
            NaiveDateTime,
            NaiveDateTime,
            DateTime<FixedOffset>,
            DateTime<FixedOffset>,
        )>,
    >::new();
    for row in rows {
        if !is_sleep_type(string_field(&row.record_type).unwrap_or_default()) {
            continue;
        }
        let Some((start, end)) = row_interval(row) else {
            continue;
        };
        let local_start = start.naive_local();
        let local_end = end.naive_local();
        let source = source_label(row);
        by_source
            .entry(source.clone())
            .or_default()
            .push((local_start, local_end, None));
        fixed_intervals
            .entry(source)
            .or_default()
            .push((local_start, local_end, start, end));
    }
    let final_day = (window_end - Duration::microseconds(1)).date_naive();
    let mut day = window_start.date_naive();
    let mut seen = BTreeSet::new();
    while day <= final_day {
        if let Some(sleep) = pick_day_sleep(&by_source, day, SLEEP_SESSION_GAP_MINUTES) {
            for (start, end) in sleep.main.into_iter().chain(sleep.naps) {
                let Some(intervals) = fixed_intervals.get(&sleep.source) else {
                    continue;
                };
                let Some(start) = intervals
                    .iter()
                    .find(|(local_start, _, _, _)| *local_start == start)
                    .map(|(_, _, start, _)| *start)
                else {
                    continue;
                };
                let Some(end) = intervals
                    .iter()
                    .find(|(_, local_end, _, _)| *local_end == end)
                    .map(|(_, _, _, end)| *end)
                else {
                    continue;
                };
                let minutes = overlap_minutes(start, end, window_start, window_end);
                let key = (sleep.source.clone(), iso(start), iso(end));
                if minutes > 0.0 && seen.insert(key) {
                    events.push(json!({"kind":"sleep","label":"Sleep","start":iso(start),"end":iso(end),"start_label":clock(start.naive_local()),"end_label":clock(end.naive_local()),"overlap_minutes":round1(minutes),"overlap_label":duration(minutes),"source":sleep.source}));
                }
            }
        }
        day = day.succ_opt().expect("valid next day");
    }
    events.sort_by_key(|item| {
        item["start"]
            .as_str()
            .and_then(|value| parse_window_bound(Some(value)))
    });
    events
}

fn window_hourly_items(
    rows: &[NormalizedRow],
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
    events: &[Value],
) -> Vec<Value> {
    let mut current = window_start
        .with_minute(0)
        .expect("valid minute")
        .with_second(0)
        .expect("valid second")
        .with_nanosecond(0)
        .expect("valid nanos");
    let mut hourly = Vec::new();
    while current < window_end {
        let next_hour = current + Duration::hours(1);
        let bucket_start = current.max(window_start);
        let bucket_end = next_hour.min(window_end);
        if bucket_end > bucket_start {
            let bucket_rows = rows
                .iter()
                .filter(|row| {
                    row_interval(row).is_some_and(|(start, end)| {
                        interval_overlaps(start, end, bucket_start, bucket_end)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            let heart_rate = window_heart_rate(&bucket_rows);
            let glucose = window_glucose(&bucket_rows);
            let steps = window_steps(&bucket_rows);
            let bucket_events = event_slice_for_bucket(events, bucket_start, bucket_end);
            let summary = hourly_summary(&heart_rate, &glucose, &steps, &bucket_events);
            hourly.push(json!({"start":iso(bucket_start),"end":iso(bucket_end),"label":clock(bucket_start.naive_local()),"range_label":format!("{} – {}",clock(bucket_start.naive_local()),clock(bucket_end.naive_local())),"has_data":!bucket_rows.is_empty() || !bucket_events.is_empty(),"entry_total":bucket_rows.len(),"entry_total_label":grouped(bucket_rows.len()),"families":window_family_items(&bucket_rows),"events":bucket_events,"heart_rate":heart_rate,"glucose":glucose,"steps":steps,"summary":summary,"summary_label":summary.join(" · ")}));
        }
        current = next_hour;
    }
    hourly
}

fn event_slice_for_bucket(
    events: &[Value],
    bucket_start: DateTime<FixedOffset>,
    bucket_end: DateTime<FixedOffset>,
) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| {
            let start = parse_window_bound(event["start"].as_str())?;
            let end = parse_window_bound(event["end"].as_str())?;
            let minutes = overlap_minutes(start, end, bucket_start, bucket_end);
            (minutes > 0.0).then(|| {
                let mut value = event.clone();
                value["overlap_minutes"] = json!(round1(minutes));
                value["overlap_label"] = json!(duration(minutes));
                value
            })
        })
        .collect()
}

fn window_sources(rows: &[NormalizedRow]) -> Value {
    let mut counts = BTreeMap::<String, usize>::new();
    for row in rows {
        *counts.entry(source_label(row)).or_default() += 1;
    }
    let names = counts.keys().cloned().collect::<Vec<_>>();
    let chips = counts
        .into_iter()
        .map(|(name, count)| json!({"name":name,"count":count,"count_label":grouped(count)}))
        .collect::<Vec<_>>();
    json!({"names":names,"count":names.len(),"chips":chips})
}

fn window_brief(
    heart_rate: &Value,
    glucose: &Value,
    steps: &Value,
    workouts: &[Value],
) -> Vec<String> {
    let mut items = Vec::new();
    if heart_rate["count"].as_u64().unwrap_or_default() > 0 {
        items.push(format!(
            "Heart rate {}",
            heart_rate["label"].as_str().expect("heart label")
        ));
    }
    if glucose["count"].as_u64().unwrap_or_default() > 0 {
        items.push(format!(
            "Glucose {}",
            glucose["delta_label"].as_str().expect("glucose label")
        ));
    }
    if let Some(label) = steps["label"].as_str() {
        items.push(label.to_owned());
    }
    if !workouts.is_empty() {
        items.push(format!(
            "{} workout{}",
            workouts.len(),
            if workouts.len() == 1 { "" } else { "s" }
        ));
    }
    items
}

fn hourly_summary(
    heart_rate: &Value,
    glucose: &Value,
    steps: &Value,
    events: &[Value],
) -> Vec<String> {
    let mut items = events
        .iter()
        .filter_map(|event| event["label"].as_str())
        .fold(Vec::new(), |mut labels, label| {
            if !labels.iter().any(|seen| seen == label) && labels.len() < 2 {
                labels.push(label.to_owned());
            }
            labels
        });
    if glucose["count"].as_u64().unwrap_or_default() > 0 {
        items.push(format!(
            "Glucose {}",
            glucose["range_label"]
                .as_str()
                .or_else(|| glucose["delta_label"].as_str())
                .expect("glucose summary")
        ));
    }
    if heart_rate["count"].as_u64().unwrap_or_default() > 0 {
        items.push(format!(
            "HR {}",
            heart_rate["label"].as_str().expect("heart summary")
        ));
    }
    if let Some(label) = steps["label"].as_str() {
        items.push(label.to_owned());
    }
    items
}

fn row_interval(row: &NormalizedRow) -> Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    let start = row_timestamp(row)?;
    let end = end_timestamp(row).unwrap_or(start).max(start);
    Some((start, end))
}

fn row_timestamp(row: &NormalizedRow) -> Option<DateTime<FixedOffset>> {
    [
        string_field(&row.start_date),
        string_field(&row.start_time),
        string_field(&row.end_date),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| parse_window_bound(Some(value)))
}

fn end_timestamp(row: &NormalizedRow) -> Option<DateTime<FixedOffset>> {
    string_field(&row.end_date).and_then(|value| parse_window_bound(Some(value)))
}
fn interval_overlaps(
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
) -> bool {
    if end <= start {
        window_start <= start && start < window_end
    } else {
        start < window_end && end > window_start
    }
}
fn overlap_minutes(
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    window_start: DateTime<FixedOffset>,
    window_end: DateTime<FixedOffset>,
) -> f64 {
    if !interval_overlaps(start, end, window_start, window_end) || end <= start {
        0.0
    } else {
        (end.min(window_end) - start.max(window_start))
            .num_seconds()
            .max(0) as f64
            / 60.0
    }
}
fn month_keys_between(start: DateTime<FixedOffset>, end: DateTime<FixedOffset>) -> Vec<String> {
    let mut current = NaiveDate::from_ymd_opt(start.year(), start.month(), 1).expect("month start");
    let final_month = NaiveDate::from_ymd_opt(end.year(), end.month(), 1).expect("month start");
    let mut keys = Vec::new();
    while current <= final_month {
        keys.push(format!("{:04}-{:02}", current.year(), current.month()));
        current = if current.month() == 12 {
            NaiveDate::from_ymd_opt(current.year() + 1, 1, 1).expect("next year")
        } else {
            NaiveDate::from_ymd_opt(current.year(), current.month() + 1, 1).expect("next month")
        };
    }
    keys
}
fn iso(time: DateTime<FixedOffset>) -> String {
    time.to_rfc3339()
}
fn grouped(value: usize) -> String {
    grouped_unsigned(value as u64)
}
fn grouped_signed(value: i64) -> String {
    grouped_i64(value)
}
fn is_audit_only_oura(record_type: &str) -> bool {
    matches!(record_type, "oura.session" | "oura.enhanced_tag")
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serde_json::{Map, Value};
    use tower::ServiceExt;

    use super::*;
    use crate::corpus_test::assert_recorded_payload;
    use crate::{
        BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedManifest, api_router,
        seed_body_journal,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-body-window-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[allow(clippy::too_many_arguments)] // Mirrors independent normalized-row fields.
    fn row(
        key: &str,
        day: &str,
        source: &str,
        record_type: &str,
        value: Option<Value>,
        start: &str,
        end: Option<&str>,
        kind: Option<&str>,
    ) -> Map<String, Value> {
        let mut item = Map::from_iter([
            ("dedupe_key".into(), Value::String(key.to_owned())),
            ("day".into(), Value::String(day.to_owned())),
            ("source_name".into(), Value::String(source.to_owned())),
            ("record_type".into(), Value::String(record_type.to_owned())),
            ("start_date".into(), Value::String(start.to_owned())),
        ]);
        if let Some(value) = value {
            item.insert("value".into(), value);
        }
        if let Some(end) = end {
            item.insert("end_date".into(), Value::String(end.to_owned()));
        }
        if let Some(kind) = kind {
            item.insert("kind".into(), Value::String(kind.to_owned()));
        }
        item
    }

    fn seed(root: &Path, aggregate: BodyAggregateSeed, rows: Vec<Map<String, Value>>) {
        let shards = if rows.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([("2026-08".to_owned(), rows)])
        };
        seed_body_journal(
            root,
            &BodyJournalSeed {
                dates: BTreeSet::new(),
                day_summaries: BTreeMap::new(),
                aggregate,
                journal_config: None,
                bundles: vec![BodySeedBundle {
                    import_id: "window".to_owned(),
                    source_family: "apple_health".to_owned(),
                    manifest: BodySeedManifest::Present {
                        source_type: Some("apple_health".to_owned()),
                        entry_count: Some(shards.values().map(Vec::len).sum::<usize>() as u64),
                        extra: Map::new(),
                    },
                    shards,
                }],
            },
        )
        .unwrap();
    }

    fn bound(value: &str) -> DateTime<FixedOffset> {
        parse_window_bound(Some(value)).unwrap()
    }

    #[test]
    fn corpus_window_payloads_match_recorded_success_cases() {
        let root = TempDir::new();
        crate::day::tests::seed_populated_body_journal(root.path());
        let payload = build_window(
            root.path(),
            bound("2026-08-01T00:00:00+00:00"),
            bound("2026-08-02T00:00:00+00:00"),
        )
        .unwrap();
        assert_recorded_payload("first_run", "/app/body/api/window", root.path(), &payload);
        assert_recorded_payload("fixed", "/app/body/api/window", root.path(), &payload);
    }

    #[tokio::test]
    async fn window_reads_shards_when_the_aggregate_is_absent() {
        let root = TempDir::new();
        seed(
            root.path(),
            BodyAggregateSeed::Absent,
            vec![row(
                "one",
                "20260801",
                "Watch",
                "Signal",
                Some(json!(1)),
                "2026-08-01T00:30:00Z",
                None,
                None,
            )],
        );
        let response = api_router(root.path()).oneshot(Request::get("/app/body/api/window?from=2026-08-01T00%3A00%3A00%2B00%3A00&to=2026-08-01T01%3A00%3A00%2B00%3A00").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn hourly_buckets_clip_to_the_requested_window() {
        let root = TempDir::new();
        seed(
            root.path(),
            BodyAggregateSeed::Absent,
            vec![row(
                "one",
                "20260801",
                "Watch",
                "Signal",
                Some(json!(1)),
                "2026-08-01T10:30:00Z",
                None,
                None,
            )],
        );
        let payload = build_window(
            root.path(),
            bound("2026-08-01T10:15:00+00:00"),
            bound("2026-08-01T11:45:00+00:00"),
        )
        .unwrap();
        assert_eq!(payload["hourly"][0]["start"], "2026-08-01T10:15:00+00:00");
        assert_eq!(payload["hourly"][0]["end"], "2026-08-01T11:00:00+00:00");
        assert_eq!(payload["hourly"][1]["start"], "2026-08-01T11:00:00+00:00");
        assert_eq!(payload["hourly"][1]["end"], "2026-08-01T11:45:00+00:00");
    }

    #[test]
    fn offset_bearing_rows_keep_their_own_offset_for_window_events() {
        let root = TempDir::new();
        seed(
            root.path(),
            BodyAggregateSeed::Absent,
            vec![row(
                "offset-workout",
                "20260801",
                "Watch",
                "HKWorkoutActivityTypeRunning",
                None,
                "2026-08-01T10:00:00+02:00",
                Some("2026-08-01T11:00:00+02:00"),
                Some("workout"),
            )],
        );
        let payload = build_window(
            root.path(),
            bound("2026-08-01T07:30:00Z"),
            bound("2026-08-01T08:30:00Z"),
        )
        .unwrap();
        assert_eq!(payload["entry_total"], 1);
        assert_eq!(payload["workouts"][0]["start"], "2026-08-01T10:00:00+02:00");
        assert_eq!(payload["events"][0]["start"], "2026-08-01T10:00:00+02:00");
        assert_eq!(payload["events"][0]["overlap_minutes"], 30.0);
    }

    #[test]
    fn bucket_events_recompute_their_distinct_partial_overlaps() {
        let root = TempDir::new();
        seed(
            root.path(),
            BodyAggregateSeed::Absent,
            vec![row(
                "workout",
                "20260801",
                "Watch",
                "HKWorkoutActivityTypeRunning",
                None,
                "2026-08-01T10:45:00Z",
                Some("2026-08-01T11:20:00Z"),
                Some("workout"),
            )],
        );
        let payload = build_window(
            root.path(),
            bound("2026-08-01T10:00:00+00:00"),
            bound("2026-08-01T12:00:00+00:00"),
        )
        .unwrap();
        assert_eq!(payload["hourly"][0]["events"][0]["overlap_minutes"], 15.0);
        assert_eq!(payload["hourly"][1]["events"][0]["overlap_minutes"], 20.0);
        assert!(
            payload["hourly"][0]["events"][0]["overlap_minutes"]
                .as_f64()
                .unwrap()
                < 60.0
        );
        assert!(
            payload["hourly"][1]["events"][0]["overlap_minutes"]
                .as_f64()
                .unwrap()
                < 60.0
        );
    }

    #[tokio::test]
    async fn refusal_ladder_and_wide_bound_grammar_match_the_reference() {
        let root = TempDir::new();
        seed(root.path(), BodyAggregateSeed::Absent, Vec::new());
        let app = api_router(root.path());
        let get = |path: &str| {
            let request = Request::get(path).body(Body::empty()).unwrap();
            let app = app.clone();
            async move { app.oneshot(request).await.unwrap() }
        };
        for path in [
            "/app/body/api/window",
            "/app/body/api/window?from=bad&to=2026-08-01T01%3A00%3A00Z",
            "/app/body/api/window?from=2026-08-01T00%3A00%3A00Z",
        ] {
            let response = get(path).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["reason_code"], "invalid_request_value");
            assert_eq!(body["error"], "one of those values couldn't be used.");
            assert_eq!(body["detail"], INVALID_BOUNDS);
        }
        for (path, detail) in [
            (
                "/app/body/api/window?from=2026-08-01T01%3A00%3A00Z&to=2026-08-01T00%3A00%3A00Z",
                END_BEFORE_START,
            ),
            (
                "/app/body/api/window?from=2026-08-01T00%3A00%3A00Z&to=2026-08-09T00%3A00%3A00Z",
                "Window span must be 7 days or less.",
            ),
            (
                "/app/body/api/window?from=2026-08-01T00%3A00%3A00Z&to=2026-08-01T00%3A00%3A00Z",
                END_BEFORE_START,
            ),
        ] {
            let response = get(path).await;
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["reason_code"], "invalid_request_value");
            assert_eq!(body["error"], "one of those values couldn't be used.");
            assert_eq!(body["detail"], detail);
        }
        for path in [
            "/app/body/api/window?from=2026-08-01T00%3A00%3A00&to=2026-08-01T01%3A00%3A00",
            "/app/body/api/window?from=2026-08-01&to=2026-08-08",
        ] {
            assert_eq!(get(path).await.status(), StatusCode::OK, "{path}");
        }
    }

    #[test]
    fn window_bounds_accept_minute_precision_and_round_span_minutes() {
        assert_eq!(
            parse_window_bound(Some("2026-08-01T10:30"))
                .unwrap()
                .to_rfc3339(),
            "2026-08-01T10:30:00+00:00"
        );
        assert_eq!(
            parse_window_bound(Some("2026-08-01T10:30+02:00"))
                .unwrap()
                .to_rfc3339(),
            "2026-08-01T10:30:00+02:00"
        );
        let root = TempDir::new();
        seed(root.path(), BodyAggregateSeed::Absent, Vec::new());
        let payload = build_window(
            root.path(),
            bound("2026-08-01T00:00:00Z"),
            bound("2026-08-01T00:01:35Z"),
        )
        .unwrap();
        assert_eq!(payload["span_minutes"], 1.6);
    }

    #[test]
    fn an_empty_window_is_a_successful_empty_payload() {
        let root = TempDir::new();
        seed(root.path(), BodyAggregateSeed::Absent, Vec::new());
        let payload = build_window(
            root.path(),
            bound("2026-08-01T00:00:00Z"),
            bound("2026-08-01T01:00:00Z"),
        )
        .unwrap();
        assert_eq!(payload["has_data"], false);
        assert_eq!(payload["entry_total"], 0);
        assert_eq!(payload["hourly"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn steps_never_sum_across_sources_but_total_for_one_source() {
        let root = TempDir::new();
        seed(
            root.path(),
            BodyAggregateSeed::Absent,
            vec![
                row(
                    "one",
                    "20260801",
                    "Watch",
                    "HKQuantityTypeIdentifierStepCount",
                    Some(json!(100)),
                    "2026-08-01T00:10:00Z",
                    None,
                    None,
                ),
                row(
                    "two",
                    "20260801",
                    "Phone",
                    "HKQuantityTypeIdentifierStepCount",
                    Some(json!(200)),
                    "2026-08-01T00:20:00Z",
                    None,
                    None,
                ),
            ],
        );
        let many = build_window(
            root.path(),
            bound("2026-08-01T00:00:00Z"),
            bound("2026-08-01T01:00:00Z"),
        )
        .unwrap();
        assert_eq!(many["steps"]["mode"], "samples");
        assert_eq!(many["steps"]["samples"], 2);
        assert!(many["steps"].get("total").is_none());
        let root = TempDir::new();
        seed(
            root.path(),
            BodyAggregateSeed::Absent,
            vec![row(
                "one",
                "20260801",
                "Watch",
                "HKQuantityTypeIdentifierStepCount",
                Some(json!(1200)),
                "2026-08-01T00:10:00Z",
                None,
                None,
            )],
        );
        let one = build_window(
            root.path(),
            bound("2026-08-01T00:00:00Z"),
            bound("2026-08-01T01:00:00Z"),
        )
        .unwrap();
        assert_eq!(
            one["steps"],
            json!({"mode":"total","samples":1,"samples_label":"1","total":1200,"total_label":"1,200","source":"Watch","label":"1,200 steps"})
        );
    }
}
