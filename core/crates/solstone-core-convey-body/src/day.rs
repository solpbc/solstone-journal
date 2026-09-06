// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native construction of the read-only Body day payload.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};
use serde_json::{Map, Value, json};
use solstone_core_body_source::{BodyValue, FieldState, ValueState};
use solstone_core_convey_http::envelope::error_envelope;

use crate::router::{StoreError, ready_stats, unavailable_response};
use crate::{
    MonthReader, NormalizedRow, SLEEP_SESSION_GAP_MINUTES, SleepStagedInterval, display_number,
    display_value, find_day_summary, friendly_contributor_name, friendly_type_name,
    friendly_unit_label, has_chronicle_day, merge_sleep_sessions, pick_day_sleep,
    pick_main_session, read_trends_cache, trends_db_path, trends_signature, typical_by_signal,
};

const APPLE: &str = "apple_health";
const OURA_API: &str = "oura_api";
const HEART_RATE: &str = "HKQuantityTypeIdentifierHeartRate";
const RESTING_HEART_RATE: &str = "HKQuantityTypeIdentifierRestingHeartRate";
const OURA_HEART_RATE: &str = "oura.heartrate";
const OURA_SLEEP: &str = "oura.sleep";
const OURA_SLEEP_SCORE: &str = "oura.daily_sleep";
const OURA_READINESS: &str = "oura.daily_readiness";
const OURA_ACTIVITY: &str = "oura.daily_activity";
const OURA_WORKOUT: &str = "oura.workout";
const OURA_CARDIOVASCULAR_AGE: &str = "oura.daily_cardiovascular_age";
const OURA_SESSION: &str = "oura.session";
const OURA_TAG: &str = "oura.enhanced_tag";
const HEART_CURVE_MIN_READINGS: usize = 12;
const BP_SYSTOLIC: &str = "BloodPressureSystolic";
const BP_DIASTOLIC: &str = "BloodPressureDiastolic";
const BP_READING_LIMIT: usize = 6;
const RHYTHM_IRREGULAR: &str = "IrregularHeartRhythmEvent";
const RHYTHM_HIGH: &str = "HighHeartRateEvent";
const RHYTHM_LOW: &str = "LowHeartRateEvent";
const AFIB_BURDEN: &str = "AtrialFibrillationBurden";
const RUNNING_POWER: &str = "RunningPower";
const RUNNING_SPEED: &str = "RunningSpeed";
const RUNNING_STRIDE_LENGTH: &str = "RunningStrideLength";
const RUNNING_GROUND_CONTACT_TIME: &str = "RunningGroundContactTime";
const RUNNING_VERTICAL_OSCILLATION: &str = "RunningVerticalOscillation";
const CURVE_SEGMENT_GAP_MINUTES: f64 = 45.0;
const SVG_HEIGHT: f64 = 260.0;
const SVG_WIDTH: f64 = 1440.0;
const SLEEP_AXIS_START_HOUR: i64 = 18;

/// Serve one calendar day of imported Body data.
pub(crate) async fn day_route(
    State(root): State<std::sync::Arc<PathBuf>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    let target = match parse_day(&day) {
        Some(value) => value,
        None => return invalid_day_response(),
    };
    let stats = match ready_stats(&root) {
        Ok(stats) => stats,
        Err(error) => return unavailable_response(error),
    };
    let mut reader = MonthReader::new(root.as_ref());
    match build_day(&root, target, stats.as_deref(), &mut reader) {
        Ok(payload) => Json(payload).into_response(),
        Err(DayError::Shard(error)) => unavailable_response(StoreError::ShardUnreadable(error)),
        Err(DayError::Store(error)) => unavailable_response(StoreError::Read(error)),
        Err(DayError::Chronicle(error)) => error_envelope(
            "internal_error",
            "that request didn't finish.",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

#[derive(Debug)]
pub(crate) enum DayError {
    Shard(String),
    Store(String),
    Chronicle(String),
}

fn invalid_day_response() -> Response {
    error_envelope(
        "invalid_day",
        "that day couldn't be used.",
        "Invalid day",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn parse_day(day: &str) -> Option<NaiveDate> {
    (day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| NaiveDate::parse_from_str(day, "%Y%m%d").ok())
        .flatten()
}

pub(crate) fn build_day(
    root: &Path,
    target: NaiveDate,
    stats: Option<&crate::HealthDedupeStats>,
    reader: &mut MonthReader,
) -> Result<Value, DayError> {
    let day = day_key(target);
    let months = day_months(target);
    let mut rows = Vec::new();
    for month in months {
        rows.extend(
            reader
                .read_month(&month)
                .map_err(|error| DayError::Shard(error.to_string()))?
                .iter()
                .cloned(),
        );
    }
    let rows = dedupe_cross_month(rows);
    let audit_rows = rows_for_day(&rows, &day);
    let day_rows = resolve_canonical_rows(audit_rows.clone());
    let previous_rows =
        resolve_canonical_rows(rows_for_day(&rows, &day_key(target - Duration::days(1))));
    let next_rows =
        resolve_canonical_rows(rows_for_day(&rows, &day_key(target + Duration::days(1))));
    let typical = warmed_typical(root, &day)?;
    let sleep = sleep_analysis(&day_rows, &previous_rows, &next_rows, target, &typical);
    let glucose = glucose_stats(&day_rows);
    let glucose_series = glucose_series(&day_rows);
    let activity = activity_analysis(&day_rows);
    let heart = heart_analysis(&day_rows, &typical);
    let recovery = recovery_analysis(&day_rows, &typical);
    let families = family_rows(&day_rows);
    let mind_sound = fact_items(
        &families
            .get("Mindfulness")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .chain(families.get("Hearing & audio").cloned().unwrap_or_default())
            .collect::<Vec<_>>(),
    );
    let walking = walking_facts(&families.get("Walking metrics").cloned().unwrap_or_default());
    let body = body_facts(
        &families
            .get("Body measurements")
            .cloned()
            .unwrap_or_default(),
    );
    let mut other = families.get("Other").cloned().unwrap_or_default();
    other.extend(
        families
            .get("Sleep")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| {
                let record_type = string_field(&row.record_type).unwrap_or_default();
                !is_sleep_type(record_type) && record_type != OURA_SLEEP_SCORE
            }),
    );
    other.retain(|row| {
        !matches!(
            string_field(&row.record_type),
            Some(OURA_SESSION | OURA_TAG)
        )
    });
    let other = fact_items(&other);
    let source = sources(&day_rows);
    let date_label = long_day(target);
    let summary = find_day_summary(root, &day)
        .map_err(|error| DayError::Chronicle(error.to_string()))?
        .unwrap_or_default();
    let has_data = !day_rows.is_empty();
    let nearest = nearest(stats.map(|value| &value.by_day), &day);
    let prompts = if has_data {
        prompts(
            &date_label,
            sleep.is_some(),
            !glucose_series.is_empty(),
            activity.as_ref().is_some_and(|value| {
                !value["workouts"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .is_empty()
            }),
            has_chronicle_day(root, &day),
        )
    } else {
        Vec::new()
    };
    Ok(json!({
        "day": day,
        "date_label": date_label,
        "summary_markdown": summary,
        "glucose": glucose,
        "entry_total": day_rows.len(),
        "has_data": has_data,
        "lede": lede(&day_rows, sleep.as_ref(), &glucose_series, activity.as_ref()),
        "sleep": sleep,
        "glucose_series": glucose_series,
        "activity": activity,
        "heart": heart,
        "recovery": recovery,
        "mind_sound": (!mind_sound.is_empty()).then(|| json!({"facts": mind_sound})),
        "walking": (!walking.is_empty()).then(|| json!({"facts": walking})),
        "body_measurements": (!body.is_empty()).then(|| json!({"facts": body})),
        "other_signals": (!other.is_empty()).then(|| json!({"facts": other})),
        "sources": source,
        "prompts": prompts,
        "audit": audit(&audit_rows),
        "nearest": nearest,
    }))
}

fn day_months(day: NaiveDate) -> Vec<String> {
    let mut months = vec![format!("{:04}-{:02}", day.year(), day.month())];
    if day.day() <= 2 {
        let prior = day.with_day(1).expect("first day exists") - Duration::days(1);
        months.push(format!("{:04}-{:02}", prior.year(), prior.month()));
    }
    let next = day + Duration::days(1);
    let next_month = format!("{:04}-{:02}", next.year(), next.month());
    if next_month != months[0] {
        months.push(next_month);
    }
    months
}

fn day_key(day: NaiveDate) -> String {
    day.format("%Y%m%d").to_string()
}

fn rows_for_day(rows: &[NormalizedRow], day: &str) -> Vec<NormalizedRow> {
    rows.iter()
        .filter(|row| string_field(&row.day) == Some(day))
        .cloned()
        .collect()
}

/// Applies Python's cross-month latest-import-id rule after per-month reads.
fn dedupe_cross_month(rows: Vec<NormalizedRow>) -> Vec<NormalizedRow> {
    let mut passthrough = Vec::new();
    let mut positions = BTreeMap::<String, usize>::new();
    let mut kept = Vec::<NormalizedRow>::new();
    for mut row in rows {
        let Some(key) = string_field(&row.dedupe_key)
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
        else {
            passthrough.push(row);
            continue;
        };
        let Some(position) = positions.get(&key).copied() else {
            normalize_import_ids(&mut row);
            positions.insert(key, kept.len());
            kept.push(row);
            continue;
        };
        let latest = latest_import_id(&row);
        let existing_latest = latest_import_id(&kept[position]);
        if latest >= existing_latest {
            let ids = merged_import_ids(&row, &kept[position]);
            row.import_ids = ids;
            kept[position] = row;
        } else {
            kept[position].import_ids = merged_import_ids(&kept[position], &row);
        }
    }
    passthrough.extend(kept);
    passthrough
}

fn normalize_import_ids(row: &mut NormalizedRow) {
    if let Some(id) = string_field(&row.import_id)
        && !row.import_ids.contains(&id.to_owned())
    {
        row.import_ids.push(id.to_owned());
    }
}
fn row_import_ids(row: &NormalizedRow) -> Vec<String> {
    let mut ids = row.import_ids.clone();
    if let Some(id) = string_field(&row.import_id)
        && !ids.contains(&id.to_owned())
    {
        ids.push(id.to_owned());
    }
    ids
}
fn latest_import_id(row: &NormalizedRow) -> String {
    row_import_ids(row).into_iter().max().unwrap_or_default()
}
fn merged_import_ids(left: &NormalizedRow, right: &NormalizedRow) -> Vec<String> {
    row_import_ids(left)
        .into_iter()
        .chain(row_import_ids(right))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn resolve_canonical_rows(rows: Vec<NormalizedRow>) -> Vec<NormalizedRow> {
    let mut fragments = BTreeSet::new();
    for row in &rows {
        if string_field(&row.source_family) != Some(OURA_API) {
            continue;
        }
        match string_field(&row.record_type) {
            Some(OURA_SLEEP) => {
                fragments.insert("SleepAnalysis");
            }
            Some("oura.daily_spo2") => {
                fragments.insert("OxygenSaturation");
            }
            Some(OURA_HEART_RATE) => {
                fragments.insert(HEART_RATE);
            }
            Some(OURA_ACTIVITY) => {
                for value in [
                    "StepCount",
                    "ActiveEnergyBurned",
                    "BasalEnergyBurned",
                    "DistanceWalkingRunning",
                ] {
                    fragments.insert(value);
                }
            }
            Some(OURA_WORKOUT) => {
                fragments.insert("WorkoutActivityType");
            }
            _ => {}
        }
    }
    if fragments.is_empty() {
        return rows;
    }
    rows.into_iter()
        .filter(|row| {
            if string_field(&row.source_family) != Some(APPLE)
                || !string_field(&row.source_name)
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains("oura")
            {
                return true;
            }
            let record_type = string_field(&row.record_type).unwrap_or_default();
            !fragments.iter().any(|fragment| {
                if fragment.starts_with("HK") {
                    record_type == *fragment
                } else {
                    record_type.contains(fragment)
                }
            })
        })
        .collect()
}

fn warmed_typical(root: &Path, day: &str) -> Result<BTreeMap<String, f64>, DayError> {
    let path = trends_db_path(root);
    let signature = trends_signature(root).map_err(|error| DayError::Store(error.to_string()))?;
    let payload =
        read_trends_cache(path, signature).map_err(|error| DayError::Store(error.to_string()))?;
    Ok(typical_by_signal(payload.as_deref(), day))
}

pub(crate) fn string_field(field: &FieldState<Value>) -> Option<&str> {
    match field {
        FieldState::Present(Value::String(value)) => Some(value),
        _ => None,
    }
}
pub(crate) fn json_field(field: &FieldState<Value>) -> Option<&Value> {
    match field {
        FieldState::Present(value) => Some(value),
        _ => None,
    }
}
fn string_value(value: &BodyValue) -> Option<String> {
    match value {
        BodyValue::String(value) => Some(
            value
                .code_points()
                .iter()
                .map(|point| char::from_u32(*point).unwrap_or('\u{fffd}'))
                .collect(),
        ),
        _ => None,
    }
}
fn number_value(value: &BodyValue) -> Option<f64> {
    match value {
        BodyValue::Integer(value) => format!(
            "{}{}",
            if value.is_negative() { "-" } else { "" },
            value.digits()
        )
        .parse()
        .ok(),
        BodyValue::Number(value) => Some(*value),
        BodyValue::String(value) => {
            string_value(&BodyValue::String(value.clone())).and_then(|value| value.parse().ok())
        }
        _ => None,
    }
}
pub(crate) fn value_number(row: &NormalizedRow) -> Option<f64> {
    match &row.value {
        ValueState::Present(value) => number_value(value),
        ValueState::Absent => None,
    }
}
pub(crate) fn metadata(row: &NormalizedRow) -> Option<&Map<String, Value>> {
    json_field(&row.metadata).and_then(Value::as_object)
}
pub(crate) fn record_time(row: &NormalizedRow) -> Option<NaiveDateTime> {
    [
        string_field(&row.start_date),
        string_field(&row.start_time),
        string_field(&row.end_date),
    ]
    .into_iter()
    .flatten()
    .find_map(parse_time)
}
pub(crate) fn end_time(row: &NormalizedRow) -> Option<NaiveDateTime> {
    string_field(&row.end_date).and_then(parse_time)
}
pub(crate) fn parse_time(value: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.naive_local())
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").ok())
}
pub(crate) fn source_label(row: &NormalizedRow) -> String {
    string_field(&row.source_name)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if string_field(&row.source_family) == Some(OURA_API) {
                "Oura (API)"
            } else {
                string_field(&row.source_family).unwrap_or("unknown")
            }
        })
        .to_owned()
}
fn is_ring_source_label(label: &str) -> bool {
    label.to_lowercase().contains("oura")
}
fn source_via(row: &NormalizedRow) -> String {
    match string_field(&row.source_family) {
        Some(OURA_API) => "Oura API".to_owned(),
        Some(value) => value
            .replace('_', " ")
            .split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().to_string() + chars.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => String::new(),
    }
}
fn unit(row: &NormalizedRow) -> Option<&str> {
    string_field(&row.unit).filter(|value| !value.is_empty())
}
fn group_by_type<'a>(rows: &[&'a NormalizedRow]) -> BTreeMap<&'a str, Vec<&'a NormalizedRow>> {
    let mut grouped = BTreeMap::new();
    for row in rows {
        if let Some(record_type) = string_field(&row.record_type) {
            grouped
                .entry(record_type)
                .or_insert_with(Vec::new)
                .push(*row);
        }
    }
    grouped
}
fn single_unit(rows: &[&NormalizedRow]) -> (Option<String>, bool) {
    let units = rows
        .iter()
        .filter_map(|row| unit(row))
        .collect::<BTreeSet<_>>();
    if units.len() > 1 {
        return (None, false);
    }
    (units.iter().next().map(|unit| (*unit).to_owned()), true)
}
fn display_range_line(record_type: &str, low: f64, high: f64, unit: Option<&str>) -> String {
    let low_label = display_number(record_type, low, unit);
    let high_label = display_number(record_type, high, unit);
    let span = if low_label == high_label {
        low_label
    } else {
        format!("{low_label}–{high_label}")
    };
    match friendly_unit_label(record_type, unit).as_deref() {
        None | Some("") => span,
        Some("%") => format!("{span}%"),
        Some(label) => format!("{span} {label}"),
    }
}
fn pace_seconds_per_km(unit: &str) -> Option<f64> {
    match unit {
        "m/s" => Some(1000.0),
        "km/h" | "km/hr" => Some(3600.0),
        _ => None,
    }
}
fn pace_label(speed: f64, unit: &str) -> Option<String> {
    let scale = pace_seconds_per_km(unit)?;
    if speed <= 0.0 {
        return None;
    }
    let total = (scale / speed).round() as i64;
    Some(format!("{}:{:02}", total / 60, total % 60))
}
pub(crate) fn family(record_type: &str) -> &'static str {
    if record_type.contains("BloodGlucose") || record_type.ends_with("Glucose") {
        "Glucose"
    } else if record_type.contains("HeartRate")
        || record_type.contains("BloodPressure")
        || record_type.contains("Atrial")
        || record_type == OURA_CARDIOVASCULAR_AGE
        || record_type == "oura.vo2_max"
    {
        "Heart"
    } else if is_sleep_type(record_type)
        || record_type == OURA_SLEEP_SCORE
        || record_type.contains("WristTemperature")
    {
        "Sleep"
    } else if record_type.contains("StepCount")
        || record_type.contains("Energy")
        || record_type.contains("Distance")
        || record_type.contains("Exercise")
        || record_type.contains("Running")
        || record_type.contains("Workout")
        || record_type == OURA_ACTIVITY
        || record_type == OURA_WORKOUT
    {
        "Activity"
    } else if matches!(
        record_type,
        OURA_READINESS
            | "oura.daily_resilience"
            | "oura.temperature_deviation"
            | "oura.daily_stress"
            | "oura.daily_spo2"
    ) {
        "Recovery"
    } else if record_type.contains("Mindful") {
        "Mindfulness"
    } else if record_type.contains("Audio") {
        "Hearing & audio"
    } else if record_type.contains("BodyMass")
        || record_type.contains("BodyFat")
        || record_type.contains("LeanBodyMass")
        || record_type.contains("Height")
    {
        "Body measurements"
    } else if record_type.contains("Walking") || record_type.contains("Flights") {
        "Walking metrics"
    } else {
        "Other"
    }
}
pub(crate) fn is_sleep_type(record_type: &str) -> bool {
    record_type.contains("SleepAnalysis") || record_type == OURA_SLEEP
}
fn family_rows(rows: &[NormalizedRow]) -> BTreeMap<&'static str, Vec<NormalizedRow>> {
    let mut values = BTreeMap::new();
    for row in rows {
        values
            .entry(family(string_field(&row.record_type).unwrap_or_default()))
            .or_insert_with(Vec::new)
            .push(row.clone());
    }
    values
}

pub(crate) fn number(value: f64) -> String {
    display_number("", value, None)
}
pub(crate) fn duration(minutes: f64) -> String {
    let total = minutes.round().max(0.0) as i64;
    let (hours, minutes) = (total / 60, total % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}
pub(crate) fn clock(time: NaiveDateTime) -> String {
    let hour = time.hour() % 12;
    format!(
        "{}:{:02} {}",
        if hour == 0 { 12 } else { hour },
        time.minute(),
        if time.hour() < 12 { "AM" } else { "PM" }
    )
}
pub(crate) fn long_day(day: NaiveDate) -> String {
    format!(
        "{} {}, {}",
        month_full_name(day.month()),
        day.day(),
        day.year()
    )
}
pub(crate) fn short_day(day: &str) -> Option<String> {
    parse_day(day).map(|day| format!("{} {}", month_abbr(day.month()), day.day()))
}

pub(crate) fn month_abbr(month: u32) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS[month as usize - 1]
}

pub(crate) fn month_full_name(month: u32) -> &'static str {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    MONTHS[month as usize - 1]
}

pub(crate) fn valid_day(value: &str) -> bool {
    value.len() == 8
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && NaiveDate::parse_from_str(value, "%Y%m%d").is_ok()
}

fn sleep_analysis(
    rows: &[NormalizedRow],
    previous: &[NormalizedRow],
    next: &[NormalizedRow],
    target: NaiveDate,
    typical: &BTreeMap<String, f64>,
) -> Option<Value> {
    let mut by_source = BTreeMap::<String, Vec<SleepStagedInterval>>::new();
    for row in previous.iter().chain(rows).chain(next) {
        if !is_sleep_type(string_field(&row.record_type).unwrap_or_default()) {
            continue;
        }
        let Some(start) = record_time(row) else {
            continue;
        };
        let end = end_time(row).unwrap_or(start);
        let stage = metadata(row)
            .and_then(|data| data.get("stage").or_else(|| data.get("type")))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| match &row.value {
                ValueState::Present(value) => string_value(value),
                ValueState::Absent => None,
            })
            .or_else(|| {
                (string_field(&row.record_type) == Some(OURA_SLEEP)).then(|| "asleep".to_owned())
            });
        by_source
            .entry(source_label(row))
            .or_default()
            .push((start, end.max(start), stage));
    }
    let sleep = pick_day_sleep(&by_source, target, SLEEP_SESSION_GAP_MINUTES)?;
    let axis_day = target - Duration::days(1);
    let view = |session: (NaiveDateTime, NaiveDateTime)| json!({"window": format!("{} – {}", clock(session.0), clock(session.1)), "duration": duration((session.1-session.0).num_seconds() as f64/60.0)});
    let segment = |session: (NaiveDateTime, NaiveDateTime), kind: &str| {
        let axis = |time: NaiveDateTime| {
            ((time.date() - axis_day).num_days() * 1440
                + time.hour() as i64 * 60
                + time.minute() as i64
                - SLEEP_AXIS_START_HOUR * 60) as f64
        };
        let mut left = axis(session.0).clamp(0.0, 1440.0);
        let right = axis(session.1).clamp(0.0, 1440.0);
        let width = (right - left).max(4.0);
        left = left.min(1440.0 - width);
        json!({"x": round1(left),"width":round1(width),"kind":kind})
    };
    let naps = sleep
        .naps
        .into_iter()
        .filter(|nap| {
            ((nap.0.date() - axis_day).num_days() * 1440
                + nap.0.hour() as i64 * 60
                + nap.0.minute() as i64
                - SLEEP_AXIS_START_HOUR * 60)
                < 1440
        })
        .collect::<Vec<_>>();
    let mut segments = Vec::new();
    if let Some(main) = sleep.main {
        segments.push(segment(main, "main"));
    }
    segments.extend(naps.iter().map(|nap| segment(*nap, "nap")));
    let score = rows
        .iter()
        .find(|row| string_field(&row.record_type) == Some(OURA_SLEEP_SCORE))
        .and_then(value_number);
    let mut payload = json!({
        "source":sleep.source,"other_sources":sleep.other_sources,
        "window":sleep.main.map(|main| view(main)["window"].clone()),
        "duration":sleep.main.map(|main| view(main)["duration"].clone()),
        "asleep_duration": if sleep.has_stage_detail { sleep.asleep_minutes.map(duration) } else { None },
        "in_bed_duration":sleep.in_bed_minutes.map(duration), "has_stage_detail":sleep.has_stage_detail,
        "naps":naps.iter().map(|nap|view(*nap)).collect::<Vec<_>>(), "comparison_line":sleep_comparison_line(&by_source, target),
        "bar":{"segments":segments,"ticks":[{"x":360,"label":"12 AM"},{"x":720,"label":"6 AM"},{"x":1080,"label":"12 PM"}]},
        "score_line":score.map(|value|format!("Sleep score {} · Oura's score",number(value))),
        "score_contributors":contributors(rows.iter().find(|row| string_field(&row.record_type)==Some(OURA_SLEEP_SCORE))),
    });
    if score.is_some()
        && let Some(value) = typical.get("sleep_score")
    {
        payload["score_typical"] = json!(number(*value));
        payload["score_typical_label"] = json!(format!("your 90-day median {}", number(*value)));
    }
    if (sleep.asleep_minutes.is_some() || sleep.in_bed_minutes.is_some())
        && let Some(value) = typical.get("asleep_minutes")
    {
        payload["asleep_typical"] = json!(duration(*value));
        payload["asleep_typical_label"] = json!(format!("your 90-day median {}", duration(*value)));
    }
    Some(payload)
}

fn sleep_comparison_line(
    by_source: &BTreeMap<String, Vec<SleepStagedInterval>>,
    target: NaiveDate,
) -> Option<String> {
    let mut spans = BTreeMap::new();
    for (source, intervals) in by_source {
        let sessions = merge_sleep_sessions(
            intervals.iter().map(|(start, end, _)| (*start, *end)),
            SLEEP_SESSION_GAP_MINUTES,
        );
        if let Some((start, end)) = pick_main_session(sessions, target).0 {
            spans.insert(source.as_str(), (end - start).num_seconds() as f64 / 60.0);
        }
    }
    let ring = spans
        .keys()
        .filter(|source| is_ring_source_label(source))
        .copied()
        .collect::<Vec<_>>();
    let others = spans
        .keys()
        .filter(|source| !is_ring_source_label(source))
        .copied()
        .collect::<Vec<_>>();
    if ring.is_empty() || others.is_empty() {
        return None;
    }
    Some(
        others
            .into_iter()
            .chain(ring)
            .map(|source| format!("{source} saw {}", duration(spans[source])))
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

fn glucose_rows(rows: &[NormalizedRow]) -> impl Iterator<Item = &NormalizedRow> {
    rows.iter()
        .filter(|row| family(string_field(&row.record_type).unwrap_or_default()) == "Glucose")
}
pub(crate) fn glucose_stats(rows: &[NormalizedRow]) -> Option<Value> {
    let values = glucose_rows(rows)
        .filter_map(value_number)
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    };
    let unit = glucose_rows(rows).filter_map(unit).collect::<BTreeSet<_>>();
    Some(
        json!({"count":values.len(),"min":values.iter().copied().fold(f64::INFINITY,f64::min),"max":values.iter().copied().fold(f64::NEG_INFINITY,f64::max),"mean":mean(&values),"unit":if unit.len()==1 {unit.iter().next().copied()} else if unit.is_empty(){None}else{Some("mixed")}}),
    )
}
fn glucose_series(rows: &[NormalizedRow]) -> Vec<Value> {
    let mut groups = BTreeMap::<String, Vec<(NaiveDateTime, f64)>>::new();
    for row in glucose_rows(rows) {
        if let (Some(time), Some(value)) = (record_time(row), value_number(row)) {
            groups
                .entry(unit(row).unwrap_or("value").to_owned())
                .or_default()
                .push((time, value));
        }
    }
    groups.into_iter().map(|(unit,mut readings)| { readings.sort_by_key(|(time,_)|*time); let values=readings.iter().map(|(_,value)|*value).collect::<Vec<_>>(); let low=values.iter().copied().fold(f64::INFINITY,f64::min);let high=values.iter().copied().fold(f64::NEG_INFINITY,f64::max);let pad=((high-low)*0.08).max(2.0);let lo=(low-pad).floor();let hi=(high+pad).ceil();let y=|value:f64|round1(SVG_HEIGHT-(value-lo)/(hi-lo)*SVG_HEIGHT);let points=readings.iter().map(|(time,value)|json!([time.hour()*60+time.minute(),value])).collect::<Vec<_>>();let mut segments=Vec::<Vec<(NaiveDateTime,f64)>>::new();for reading in readings.iter().copied(){let new_segment=segments.last().is_none_or(|segment|reading.0.signed_duration_since(segment.last().expect("reading").0).num_minutes()>45);if new_segment{segments.push(Vec::new());}segments.last_mut().expect("segment").push(reading);}let paths=segments.iter().filter(|segment|segment.len()>1).map(|segment|format!("M{}",segment.iter().map(|(time,value)|format!("{} {}",time.hour()*60+time.minute(),y(*value))).collect::<Vec<_>>().join(" L"))).collect::<Vec<_>>();let dots=segments.iter().filter(|segment|segment.len()==1).map(|segment|json!([(segment[0].0.hour()*60+segment[0].0.minute()) as f64,y(segment[0].1)])).collect::<Vec<_>>();json!({"unit":unit,"count":values.len(),"count_label":values.len().to_string(),"min":low,"max":high,"mean":mean(&values),"range_label":format!("{}–{} {}",number(low),number(high),unit),"mean_label":number(mean(&values)),"points":points,"svg":{"width":SVG_WIDTH,"height":SVG_HEIGHT,"paths":paths,"dots":dots,"y_min_label":number(lo),"y_max_label":number(hi)}}) }).collect()
}

fn activity_analysis(rows: &[NormalizedRow]) -> Option<Value> {
    let workouts = rows
        .iter()
        .filter(|row| string_field(&row.kind) == Some("workout"))
        .collect::<Vec<_>>();
    let activity = rows
        .iter()
        .filter(|row| {
            string_field(&row.kind) != Some("workout")
                && family(string_field(&row.record_type).unwrap_or_default()) == "Activity"
        })
        .collect::<Vec<_>>();
    if workouts.is_empty() && activity.is_empty() {
        return None;
    }
    let mut step_rows = activity
        .iter()
        .filter(|row| string_field(&row.record_type).is_some_and(|kind| kind.contains("StepCount")))
        .map(|row| (*row).clone())
        .collect::<Vec<_>>();
    for row in rows
        .iter()
        .filter(|row| string_field(&row.record_type) == Some(OURA_ACTIVITY))
    {
        if let Some(steps) = metadata(row)
            .and_then(|data| data.get("steps"))
            .and_then(Value::as_f64)
        {
            let mut synthetic = row.clone();
            synthetic.value = ValueState::Present(BodyValue::Number(steps));
            if let Some(start) = record_time(row) {
                synthetic.end_date = FieldState::Present(json!(format!(
                    "{}T00:00:00",
                    (start.date() + Duration::days(1)).format("%Y-%m-%d")
                )));
            }
            step_rows.push(synthetic);
        }
    }
    let steps=primary_total(&step_rows).map(|(total,source,samples,others)|json!({"mode":"total","total":total.round() as i64,"total_label":format!("{:.0}",total.round()).chars().rev().collect::<Vec<_>>().chunks(3).map(|part|part.iter().collect::<String>()).collect::<Vec<_>>().join(",").chars().rev().collect::<String>(),"source":source,"samples":samples,"others":others,"others_label":if others.is_empty(){None}else{Some(format!("{} also contributed",others.join(", ")))}})).or_else(||(!step_rows.is_empty()).then(||json!({"mode":"samples","samples":step_rows.len(),"samples_label":step_rows.len().to_string()})));
    let workout_items = workouts
        .iter()
        .map(|row| workout_item(row))
        .collect::<Vec<_>>();
    let mut kinds = BTreeMap::<String, usize>::new();
    for item in &workout_items {
        *kinds
            .entry(item["name"].as_str().unwrap_or_default().to_owned())
            .or_default() += 1;
    }
    let workout_summary = (!kinds.is_empty()).then(|| {
        kinds
            .into_iter()
            .map(|(name, count)| {
                if count == 1 {
                    name
                } else {
                    format!("{name} ×{count}")
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let running_rows = activity
        .iter()
        .filter(|row| is_running_dynamics_type(string_field(&row.record_type).unwrap_or_default()))
        .copied()
        .collect::<Vec<_>>();
    let counters=activity.iter().filter(|row|string_field(&row.record_type)!=Some("HKQuantityTypeIdentifierStepCount")).filter(|row|string_field(&row.record_type)!=Some(OURA_WORKOUT)).filter(|row|!is_running_dynamics_type(string_field(&row.record_type).unwrap_or_default())).filter_map(|row|{let kind=string_field(&row.record_type).unwrap_or_default();(kind==OURA_ACTIVITY).then(||value_number(row).map(|value|json!({"label":"Daily activity","count":1,"count_label":"1","value":format!("{} · Oura's score",number(value))}))).flatten()}).collect::<Vec<_>>();
    Some(
        json!({"workouts":workout_items,"workout_summary":workout_summary,"steps":steps,"running":if running_rows.is_empty(){None}else{Some(running_dynamics(&running_rows))},"counters":counters}),
    )
}

fn is_running_dynamics_type(record_type: &str) -> bool {
    [
        RUNNING_POWER,
        RUNNING_SPEED,
        RUNNING_STRIDE_LENGTH,
        RUNNING_GROUND_CONTACT_TIME,
        RUNNING_VERTICAL_OSCILLATION,
    ]
    .iter()
    .any(|fragment| record_type.contains(fragment))
}

fn running_dynamics(rows: &[&NormalizedRow]) -> Vec<Value> {
    let grouped = group_by_type(rows);
    let mut types = grouped.into_iter().collect::<Vec<_>>();
    types.sort_by_key(|(record_type, _)| friendly_type_name(record_type));
    types
        .into_iter()
        .map(|(record_type, rows)| {
            let values = rows.iter().filter_map(|row| value_number(row)).collect::<Vec<_>>();
            let (unit, consistent) = single_unit(&rows);
            let summary = if values.is_empty() || !consistent {
                None
            } else {
                let low = values.iter().copied().fold(f64::INFINITY, f64::min);
                let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let average = mean(&values);
                let pace = record_type.contains(RUNNING_SPEED)
                    .then(|| unit.as_deref().and_then(|unit| {
                        Some((
                            pace_label(high, unit)?,
                            pace_label(low, unit)?,
                            pace_label(average, unit)?,
                        ))
                    }))
                    .flatten();
                if let Some((fast, slow, middle)) = pace {
                    let span = if fast == slow { fast } else { format!("{fast}–{slow}") };
                    Some(format!("{span} /km · avg {middle} /km"))
                } else {
                    let span = display_range_line(record_type, low, high, unit.as_deref());
                    (low == high).then_some(span.clone()).or_else(|| {
                        Some(format!(
                            "{span} · avg {}",
                            display_number(record_type, average, unit.as_deref())
                        ))
                    })
                }
            };
            json!({"label":friendly_type_name(record_type),"count":rows.len(),"count_label":rows.len().to_string(),"summary":summary})
        })
        .collect()
}

fn primary_total(rows: &[NormalizedRow]) -> Option<(f64, String, usize, Vec<String>)> {
    let mut values = BTreeMap::<String, (f64, f64, usize)>::new();
    for row in rows {
        if let Some(value) = value_number(row) {
            let coverage = match (record_time(row), end_time(row)) {
                (Some(start), Some(end)) if end > start => (end - start).num_seconds() as f64,
                _ => 0.0,
            };
            let entry = values.entry(source_label(row)).or_insert((0.0, 0.0, 0));
            entry.0 += value;
            entry.1 += coverage;
            entry.2 += 1;
        }
    }
    let (source, (total, _, samples)) = if values.len() == 1 {
        values.iter().next()?
    } else {
        values
            .iter()
            .filter(|(_, (_, coverage, _))| *coverage > 0.0)
            .max_by(|(a, (_, ac, _)), (b, (_, bc, _))| {
                ac.partial_cmp(bc)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.cmp(a))
            })?
    };
    Some((
        *total,
        source.clone(),
        *samples,
        values
            .keys()
            .filter(|name| *name != source)
            .cloned()
            .collect(),
    ))
}
pub(crate) fn workout_item(row: &NormalizedRow) -> Value {
    let data = metadata(row);
    let name = data
        .and_then(|data| data.get("activity"))
        .and_then(Value::as_str)
        .map(title)
        .unwrap_or_else(|| friendly_type_name(string_field(&row.record_type).unwrap_or("Workout")));
    let metric = |value_key: &str, unit_key: &str, record_type: &str| {
        data.and_then(|data| data.get(value_key))
            .and_then(Value::as_f64)
            .map(|value| {
                let unit = data
                    .and_then(|data| data.get(unit_key))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let label = if record_type.contains("Distance") {
                    format!("{} {unit}", grouped_decimal(value, 1))
                } else {
                    format!(
                        "{} {}",
                        number(value),
                        friendly_unit_label(record_type, Some(unit))
                            .unwrap_or_else(|| unit.to_owned())
                    )
                };
                json!({"value":value,"unit":unit,"record_type":record_type,"label":label})
            })
    };
    let distance = metric(
        if string_field(&row.record_type) == Some(OURA_WORKOUT) {
            "distance"
        } else {
            "totalDistance"
        },
        if string_field(&row.record_type) == Some(OURA_WORKOUT) {
            "distance_unit"
        } else {
            "totalDistanceUnit"
        },
        "HKQuantityTypeIdentifierDistanceWalkingRunning",
    );
    let energy = metric(
        if string_field(&row.record_type) == Some(OURA_WORKOUT) {
            "calories"
        } else {
            "totalEnergyBurned"
        },
        if string_field(&row.record_type) == Some(OURA_WORKOUT) {
            "calories_unit"
        } else {
            "totalEnergyBurnedUnit"
        },
        "HKQuantityTypeIdentifierActiveEnergyBurned",
    );
    let labels = [distance.as_ref(), energy.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|item| item["label"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    json!({"name":name,"source":source_label(row),"start":record_time(row).map(clock),"duration":workout_duration(row),"distance":distance,"energy":energy,"metric_labels":labels,"metrics_label":labels.join(" · ")})
}
fn workout_duration(row: &NormalizedRow) -> Option<String> {
    let metadata = metadata(row);
    let minutes = metadata
        .and_then(|data| data.get("duration"))
        .and_then(Value::as_f64)
        .or_else(|| match (record_time(row), end_time(row)) {
            (Some(start), Some(end)) if end >= start => {
                Some((end - start).num_seconds() as f64 / 60.0)
            }
            _ => None,
        })?;
    if minutes <= 0.0 {
        None
    } else if minutes < 1.0 {
        Some("<1m".to_owned())
    } else {
        Some(duration(minutes))
    }
}

fn heart_analysis(rows: &[NormalizedRow], typical: &BTreeMap<String, f64>) -> Option<Value> {
    let samples = rows
        .iter()
        .filter(|row| {
            matches!(
                string_field(&row.record_type),
                Some(HEART_RATE | OURA_HEART_RATE)
            )
        })
        .filter_map(|row| Some((row, value_number(row)?)))
        .collect::<Vec<_>>();
    let resting_row = rows
        .iter()
        .filter(|row| string_field(&row.record_type) == Some(RESTING_HEART_RATE))
        .filter_map(|row| value_number(row).map(|value| (row, value)))
        .max_by_key(|(row, _)| record_time(row));
    let ring_resting = rows
        .iter()
        .filter(|row| string_field(&row.record_type) == Some(OURA_SLEEP))
        .filter_map(|row| {
            metadata(row)
                .and_then(|data| data.get("lowest_heart_rate"))
                .and_then(Value::as_f64)
                .filter(|value| *value > 0.0)
        })
        .min_by(f64::total_cmp);
    let mut facts = Vec::new();
    if let Some(row) = rows
        .iter()
        .filter(|row| string_field(&row.record_type) == Some(OURA_CARDIOVASCULAR_AGE))
        .max_by_key(|row| record_time(row))
        && let Some(value) = value_number(row)
    {
        facts.push(json!({"label":"Vascular age","count":1,"count_label":"1","value":format!("{} · Oura's estimate",number(value))}));
    }
    if let Some((row, value)) = resting_row {
        let mut item = json!({"label":"Resting heart rate","count":1,"count_label":"1","value":format!("{} bpm",number(value))});
        if let Some(baseline) = typical.get("resting_hr") {
            item["typical"] = json!(format!("{} bpm", number(*baseline)));
            item["typical_label"] = json!(format!("your 90-day median {} bpm", number(*baseline)));
        }
        facts.push(item);
        if let Some(ring) = ring_resting {
            let line = format!(
                "{} {} bpm · Oura (API) {} bpm",
                source_label(row),
                number(value),
                number(ring)
            );
            return finalize_heart(&samples, facts, rows, Some(line));
        }
    } else if let Some(ring) = ring_resting {
        let mut item = json!({"label":"Resting heart rate","count":1,"count_label":"1","value":format!("{} bpm · Oura's measurement",number(ring))});
        if let Some(baseline) = typical.get("resting_hr") {
            item["typical"] = json!(format!("{} bpm", number(*baseline)));
            item["typical_label"] = json!(format!("your 90-day median {} bpm", number(*baseline)));
        }
        facts.push(item);
    }
    finalize_heart(&samples, facts, rows, None)
}

fn finalize_heart(
    samples: &[(&NormalizedRow, f64)],
    mut facts: Vec<Value>,
    rows: &[NormalizedRow],
    resting_comparison: Option<String>,
) -> Option<Value> {
    let blood_pressure = blood_pressure(rows);
    let rhythm = rhythm_summary(rows);
    if blood_pressure.is_none() {
        facts.extend(blood_pressure_facts(rows));
    }
    heart_payload(
        samples,
        facts,
        blood_pressure,
        rhythm,
        heart_comparison_line(samples),
        resting_comparison,
    )
}

fn heart_payload(
    samples: &[(&NormalizedRow, f64)],
    facts: Vec<Value>,
    blood_pressure: Option<Value>,
    rhythm: Option<Value>,
    comparison_line: Option<String>,
    resting_comparison: Option<String>,
) -> Option<Value> {
    if samples.is_empty() && blood_pressure.is_none() && rhythm.is_none() && facts.is_empty() {
        return None;
    }
    let values = samples.iter().map(|(_, value)| *value).collect::<Vec<_>>();
    Some(
        json!({"heart_rate":if values.is_empty(){None}else{Some(json!({"min":values.iter().copied().fold(f64::INFINITY,f64::min),"max":values.iter().copied().fold(f64::NEG_INFINITY,f64::max),"count":values.len(),"count_label":values.len().to_string(),"unit":samples.first().and_then(|(row,_)|unit(row)),"label":format!("{}–{} bpm",number(values.iter().copied().fold(f64::INFINITY,f64::min)),number(values.iter().copied().fold(f64::NEG_INFINITY,f64::max))),"summary":format!("{}–{} bpm · {} readings",number(values.iter().copied().fold(f64::INFINITY,f64::min)),number(values.iter().copied().fold(f64::NEG_INFINITY,f64::max)),values.len())}))},"series":heart_series(samples),"facts":facts,"blood_pressure":blood_pressure,"rhythm":rhythm,"comparison_line":comparison_line,"resting_comparison_line":resting_comparison}),
    )
}

fn blood_pressure(rows: &[NormalizedRow]) -> Option<Value> {
    let mut by_start = BTreeMap::<String, BTreeMap<&str, (&NormalizedRow, f64)>>::new();
    for row in rows {
        let record_type = string_field(&row.record_type).unwrap_or_default();
        let component = if record_type.contains(BP_SYSTOLIC) {
            "systolic"
        } else if record_type.contains(BP_DIASTOLIC) {
            "diastolic"
        } else {
            continue;
        };
        let Some(start) = string_field(&row.start_date).filter(|value| !value.is_empty()) else {
            continue;
        };
        let Some(value) = value_number(row) else {
            continue;
        };
        by_start
            .entry(start.to_owned())
            .or_default()
            .entry(component)
            .or_insert((row, value));
    }
    let mut pairs = by_start
        .into_values()
        .filter_map(|components| {
            Some((*components.get("systolic")?, *components.get("diastolic")?))
        })
        .filter(|((row, _), _)| record_time(row).is_some())
        .collect::<Vec<_>>();
    pairs.sort_by_key(|((row, _), _)| record_time(row));
    let mut readings = Vec::new();
    let mut systolic_values = Vec::new();
    let mut diastolic_values = Vec::new();
    let mut card_unit = None;
    for ((systolic_row, systolic), (_, diastolic)) in pairs {
        let moment = record_time(systolic_row).expect("filtered parseable start");
        let pair_unit = unit(systolic_row);
        if card_unit.is_none() {
            card_unit = pair_unit.map(str::to_owned);
        }
        let mut label = format!("{}/{}", number(systolic), number(diastolic));
        if let Some(pair_unit) = pair_unit {
            label.push(' ');
            label.push_str(pair_unit);
        }
        readings.push(json!({"time":clock(moment),"label":label}));
        systolic_values.push(systolic);
        diastolic_values.push(diastolic);
    }
    if readings.is_empty() {
        return None;
    }
    let count = readings.len();
    let range_label = (count > BP_READING_LIMIT).then(|| {
        let span = |values: &[f64]| {
            let low = values.iter().copied().fold(f64::INFINITY, f64::min);
            let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            format!("{}–{}", number(low), number(high))
        };
        let suffix = card_unit
            .as_deref()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        format!(
            "systolic {}{suffix} · diastolic {}{suffix}",
            span(&systolic_values),
            span(&diastolic_values)
        )
    });
    Some(
        json!({"count":count,"count_label":count.to_string(),"unit":card_unit,"mode":if count<=BP_READING_LIMIT {"readings"} else {"range"},"readings":if count<=BP_READING_LIMIT {readings} else {Vec::new()},"range_label":range_label}),
    )
}

fn blood_pressure_facts(rows: &[NormalizedRow]) -> Vec<Value> {
    let blood_pressure_rows = rows
        .iter()
        .filter(|row| {
            let record_type = string_field(&row.record_type).unwrap_or_default();
            record_type.contains(BP_SYSTOLIC) || record_type.contains(BP_DIASTOLIC)
        })
        .collect::<Vec<_>>();
    let mut groups = group_by_type(&blood_pressure_rows)
        .into_iter()
        .collect::<Vec<_>>();
    groups.sort_by_key(|(record_type, _)| friendly_type_name(record_type));
    groups
        .into_iter()
        .map(|(record_type, rows)| {
            let values = rows.iter().filter_map(|row| value_number(row)).collect::<Vec<_>>();
            let (unit, consistent) = single_unit(&rows);
            let value = if values.is_empty() || !consistent {
                None
            } else if values.len() == 1 {
                Some(display_value(record_type, values[0], unit.as_deref()))
            } else {
                Some(display_range_line(
                    record_type,
                    values.iter().copied().fold(f64::INFINITY, f64::min),
                    values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    unit.as_deref(),
                ))
            };
            json!({"label":friendly_type_name(record_type),"count":rows.len(),"count_label":rows.len().to_string(),"value":value})
        })
        .collect()
}

fn is_rhythm_event_type(record_type: &str) -> bool {
    [RHYTHM_IRREGULAR, RHYTHM_HIGH, RHYTHM_LOW]
        .iter()
        .any(|fragment| record_type.contains(fragment))
}

fn rhythm_summary(rows: &[NormalizedRow]) -> Option<Value> {
    let mut event_rows = Vec::new();
    let mut burden_rows = Vec::new();
    for row in rows {
        let record_type = string_field(&row.record_type).unwrap_or_default();
        if is_rhythm_event_type(record_type) {
            event_rows.push(row);
        } else if record_type.contains(AFIB_BURDEN) {
            burden_rows.push(row);
        }
    }
    if event_rows.is_empty() && burden_rows.is_empty() {
        return None;
    }
    let mut event_groups = group_by_type(&event_rows).into_iter().collect::<Vec<_>>();
    event_groups.sort_by_key(|(record_type, _)| friendly_type_name(record_type));
    let events = event_groups
        .into_iter()
        .map(|(record_type, rows)| {
            let label = friendly_type_name(record_type);
            let count = rows.len();
            let sources = rows.iter().map(|row| source_label(row)).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
            let word = if count == 1 { "event" } else { "events" };
            let detail = format!("{count} {word} · reported by {}", sources.join(", "));
            json!({"label":label,"count":count,"count_label":count.to_string(),"sources":sources,"detail":detail,"line":format!("{label} · {detail}")})
        })
        .collect::<Vec<_>>();
    let burden = (!burden_rows.is_empty()).then(|| {
        let record_type = string_field(&burden_rows[0].record_type).unwrap_or_default();
        let label = friendly_type_name(record_type);
        let count = burden_rows.len();
        let sources = burden_rows.iter().map(|row| source_label(row)).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
        let value_label = burden_rows
            .iter()
            .filter_map(|row| value_number(row).map(|value| (*row, value)))
            .max_by_key(|(row, _)| record_time(row))
            .map(|(row, value)| display_value(record_type, value, unit(row)));
        let attribution = format!("reported by {}", sources.join(", "));
        let detail = match &value_label {
            None => format!("{count} {} · {attribution}", if count == 1 { "entry" } else { "entries" }),
            Some(value) if count > 1 => format!("latest {value} · {count} entries · {attribution}"),
            Some(value) => format!("{value} · {attribution}"),
        };
        json!({"label":label,"count":count,"count_label":count.to_string(),"sources":sources,"value":value_label,"detail":detail,"line":format!("{label} · {detail}")})
    });
    Some(json!({"events":events,"burden":burden}))
}

fn heart_comparison_line(samples: &[(&NormalizedRow, f64)]) -> Option<String> {
    let mut by_source = BTreeMap::<String, Vec<f64>>::new();
    for (row, value) in samples {
        by_source.entry(source_label(row)).or_default().push(*value);
    }
    let ring = by_source
        .keys()
        .filter(|source| is_ring_source_label(source))
        .collect::<Vec<_>>();
    let others = by_source
        .keys()
        .filter(|source| !is_ring_source_label(source))
        .collect::<Vec<_>>();
    if ring.is_empty() || others.is_empty() {
        return None;
    }
    Some(
        others
            .into_iter()
            .chain(ring)
            .map(|source| {
                let values = &by_source[source];
                let low = values.iter().copied().fold(f64::INFINITY, f64::min);
                let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let span = if low == high {
                    number(low)
                } else {
                    format!("{}–{}", number(low), number(high))
                };
                format!("{source} {span} bpm")
            })
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

fn heart_series(samples: &[(&NormalizedRow, f64)]) -> Option<Value> {
    if samples.len() < HEART_CURVE_MIN_READINGS {
        return None;
    };
    let raw_units = samples
        .iter()
        .filter_map(|(row, _)| unit(row))
        .collect::<BTreeSet<_>>();
    let unit = match raw_units.len() {
        0 => None,
        1 => raw_units.iter().next().map(|unit| (*unit).to_owned()),
        _ => {
            let display_units = raw_units
                .iter()
                .filter_map(|unit| friendly_unit_label(HEART_RATE, Some(unit)))
                .collect::<BTreeSet<_>>();
            if display_units.len() != 1 {
                return None;
            }
            display_units.into_iter().next()
        }
    };
    let mut readings = samples
        .iter()
        .filter_map(|(row, value)| record_time(row).map(|time| (time, *value)))
        .collect::<Vec<_>>();
    if readings.len() < HEART_CURVE_MIN_READINGS {
        return None;
    };
    readings.sort_by_key(|(time, _)| *time);
    let values = readings.iter().map(|(_, value)| *value).collect::<Vec<_>>();
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let pad = ((high - low) * 0.08).max(2.0);
    let lo = (low - pad).floor();
    let hi = (high + pad).ceil();
    let y = |value: f64| round1(SVG_HEIGHT - (value - lo) / (hi - lo) * SVG_HEIGHT);
    let mut buckets = BTreeMap::<i64, Vec<f64>>::new();
    for (time, value) in readings {
        buckets
            .entry((time.hour() as i64 * 60 + time.minute() as i64) / 5)
            .or_default()
            .push(value);
    }
    let stats = buckets
        .into_iter()
        .map(|(bucket, mut values)| {
            values.sort_by(f64::total_cmp);
            let middle = values.len() / 2;
            let median = if values.len() % 2 == 0 {
                (values[middle - 1] + values[middle]) / 2.0
            } else {
                values[middle]
            };
            (
                bucket as f64 * 5.0 + 2.5,
                median,
                values[0],
                *values.last().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut segments = Vec::<Vec<(f64, f64, f64, f64)>>::new();
    for stat in stats.iter().copied() {
        let new_segment = segments.last().is_none_or(|segment| {
            stat.0 - segment.last().expect("segment has stat").0 > CURVE_SEGMENT_GAP_MINUTES
        });
        if new_segment {
            segments.push(Vec::new());
        }
        segments.last_mut().expect("segment exists").push(stat);
    }
    let mut paths = Vec::new();
    let mut band_paths = Vec::new();
    let mut dots = Vec::new();
    for segment in segments {
        if segment.len() == 1 {
            dots.push(json!([segment[0].0, y(segment[0].1)]));
            continue;
        }
        paths.push(format!(
            "M{}",
            segment
                .iter()
                .map(|(x, mid, _, _)| format!("{x} {}", y(*mid)))
                .collect::<Vec<_>>()
                .join(" L")
        ));
        let upper = segment
            .iter()
            .map(|(x, _, _, high)| format!("{x} {}", y(*high)));
        let lower = segment
            .iter()
            .rev()
            .map(|(x, _, low, _)| format!("{x} {}", y(*low)));
        band_paths.push(format!(
            "M{} Z",
            upper.chain(lower).collect::<Vec<_>>().join(" L")
        ));
    }
    Some(
        json!({"unit":unit,"unit_label":friendly_unit_label(HEART_RATE,unit.as_deref()),"count":values.len(),"count_label":values.len().to_string(),"min":low,"max":high,"bucket_minutes":5,"points":stats.iter().map(|(x,mid,_,_)|json!([x,mid])).collect::<Vec<_>>(),"bands":stats.iter().map(|(x,_,low,high)|json!([x,low,high])).collect::<Vec<_>>(),"svg":{"width":SVG_WIDTH,"height":SVG_HEIGHT,"paths":paths,"band_paths":band_paths,"dots":dots,"y_min_label":number(lo),"y_max_label":number(hi)}}),
    )
}

fn recovery_analysis(rows: &[NormalizedRow], typical: &BTreeMap<String, f64>) -> Option<Value> {
    let row = rows
        .iter()
        .find(|row| string_field(&row.record_type) == Some(OURA_READINESS))?;
    let value = value_number(row)?;
    let mut fact = json!({"label":"Readiness","detail":format!("{} · Oura's score",number(value)),"line":format!("Readiness {} · Oura's score",number(value))});
    if let Some(baseline) = typical.get("readiness") {
        fact["typical"] = json!(number(*baseline));
        fact["typical_label"] = json!(format!("your 90-day median {}", number(*baseline)));
    }
    Some(json!({"facts":[fact],"contributors":contributors(Some(row))}))
}

fn contributors(row: Option<&NormalizedRow>) -> Vec<Value> {
    metadata_opt(row)
        .and_then(|data| data.get("contributors"))
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter(|(_, value)| value.is_number())
                .map(|(name, value)| json!({"label":friendly_contributor_name(name),"value":value}))
                .collect()
        })
        .unwrap_or_default()
}
fn metadata_opt(row: Option<&NormalizedRow>) -> Option<&Map<String, Value>> {
    row.and_then(metadata)
}

fn fact_items(rows: &[NormalizedRow]) -> Vec<Value> {
    let mut grouped = BTreeMap::<String, Vec<&NormalizedRow>>::new();
    for row in rows {
        grouped
            .entry(
                string_field(&row.record_type)
                    .unwrap_or_default()
                    .to_owned(),
            )
            .or_default()
            .push(row);
    }
    let mut grouped = grouped.into_iter().collect::<Vec<_>>();
    grouped.sort_by(|(left, left_rows), (right, right_rows)| {
        right_rows
            .len()
            .cmp(&left_rows.len())
            .then_with(|| friendly_type_name(left).cmp(&friendly_type_name(right)))
    });
    grouped.into_iter().map(|(record_type,items)| { let values=items.iter().filter_map(|row|value_number(row)).collect::<Vec<_>>(); let count=items.len(); let value=if record_type.contains("MindfulSession") { let sources=items.iter().map(|row|source_label(row)).collect::<BTreeSet<_>>(); let minutes=items.iter().filter_map(|row|match(record_time(row),end_time(row)){(Some(start),Some(end))if end>start=>Some((end-start).num_seconds()as f64/60.0),_=>None}).sum::<f64>();(sources.len()==1&&minutes>0.0).then(||duration(minutes)) } else if record_type.contains("AudioExposure")&&count>1 { let units=items.iter().filter_map(|row|unit(row)).collect::<BTreeSet<_>>();if !values.is_empty()&&units.len()==1{Some(format!("{count} entries · {}–{} {}",number(values.iter().copied().fold(f64::INFINITY,f64::min)),number(values.iter().copied().fold(f64::NEG_INFINITY,f64::max)),friendly_unit_label(&record_type,units.iter().next().copied()).unwrap_or_else(||units.iter().next().unwrap().to_string())))}else{None} } else if count==1 || record_type.contains("RestingHeartRate") { items.iter().filter_map(|row|value_number(row).map(|value|(row,value))).max_by_key(|(row,_)|record_time(row)).map(|(row,value)|display_value(&record_type,value,unit(row))) } else {None};json!({"label":friendly_type_name(&record_type),"count":count,"count_label":count.to_string(),"value":value}) }).collect()
}
fn walking_facts(rows: &[NormalizedRow]) -> Vec<Value> {
    fact_items(rows)
}
fn body_facts(rows: &[NormalizedRow]) -> Vec<Value> {
    fact_items(rows)
}
fn sources(rows: &[NormalizedRow]) -> Option<Value> {
    if rows.is_empty() {
        return None;
    };
    let mut counts = BTreeMap::<String, usize>::new();
    for row in rows {
        *counts.entry(source_label(row)).or_default() += 1;
    }
    let names = counts.keys().cloned().collect::<Vec<_>>();
    let via = rows
        .iter()
        .map(source_via)
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(" + ");
    Some(
        json!({"names":names,"chips":counts.into_iter().map(|(name,count)|json!({"name":name,"count":count,"count_label":count.to_string(),"entries_label":format!("{} {}",count,if count==1{"entry"}else{"entries"})})).collect::<Vec<_>>(),"entry_total":rows.len(),"entry_total_label":rows.len().to_string(),"via":if via.is_empty(){"imports"}else{&via}}),
    )
}
fn nearest(by_day: Option<&BTreeMap<String, u64>>, day: &str) -> Value {
    let Some(by_day) = by_day else {
        return json!({"prev":null,"prev_label":null,"next":null,"next_label":null});
    };
    let prev = by_day.keys().filter(|key| key.as_str() < day).max();
    let next = by_day.keys().filter(|key| key.as_str() > day).min();
    json!({"prev":prev,"prev_label":prev.and_then(|value|short_day(value)),"next":next,"next_label":next.and_then(|value|short_day(value))})
}
fn prompts(
    date_label: &str,
    has_sleep: bool,
    has_glucose: bool,
    has_workouts: bool,
    has_journal: bool,
) -> Vec<String> {
    let mut prompts = vec![format!(
        "How did my body on {date_label} compare with nearby days?"
    )];
    if has_glucose {
        prompts.push(format!(
            "What was on my calendar during the glucose peak on {date_label}?"
        ));
    }
    if has_workouts && has_journal {
        prompts.push(format!(
            "What happened in my journal after the workouts on {date_label}?"
        ));
    }
    if has_sleep {
        prompts.push(format!(
            "What did my evening look like before the sleep ending {date_label}?"
        ));
    }
    if prompts.len() >= 3 {
        prompts.truncate(3);
        return prompts;
    }
    for item in [
        has_journal.then(|| format!("What does my journal hold for {date_label}?")),
        Some(format!("Who did I spend {date_label} with?")),
    ]
    .into_iter()
    .flatten()
    {
        if prompts.len() >= 3 {
            break;
        }
        prompts.push(item)
    }
    prompts
}
fn lede(
    rows: &[NormalizedRow],
    sleep: Option<&Value>,
    glucose: &[Value],
    activity: Option<&Value>,
) -> String {
    let mut parts = Vec::new();
    if let Some(sleep) = sleep
        && let Some(asleep) = sleep.get("asleep_duration").and_then(Value::as_str)
    {
        let bed = sleep.get("in_bed_duration").and_then(Value::as_str);
        if Some(asleep) != bed {
            parts.push(format!("slept {asleep} (in bed {})", bed.unwrap_or("")));
        } else if let Some(window) = sleep.get("window").and_then(Value::as_str) {
            parts.push(format!("slept {window}"));
        }
    }
    for series in glucose {
        if let Some(range) = series.get("range_label").and_then(Value::as_str) {
            parts.push(format!("glucose {range}"));
        }
    }
    if let Some(workouts) = activity
        .and_then(|item| item.get("workouts"))
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
    {
        parts.push(format!(
            "{} workout{}",
            workouts.len(),
            if workouts.len() == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        if rows.is_empty() {
            return "No body data present for this day.".to_owned();
        }
        let mut groups = BTreeMap::<&str, usize>::new();
        for row in rows {
            *groups
                .entry(family(string_field(&row.record_type).unwrap_or_default()))
                .or_default() += 1;
        }
        let names = groups.into_iter().collect::<Vec<_>>();
        let names = names.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        let entries = format!(
            "{} {}",
            rows.len(),
            if rows.len() == 1 { "entry" } else { "entries" }
        );
        parts.push(match names.len() {
            1 => format!("{entries} across {}", names[0]),
            2 => format!("{entries} across {} and {}", names[0], names[1]),
            _ => format!(
                "{entries} across {}, {}, and {} more areas",
                names[0],
                names[1],
                names.len() - 2
            ),
        });
    }
    let mut text = parts.join(", ");
    text.replace_range(..1, &text[..1].to_uppercase());
    format!("{text}.")
}
fn audit(rows: &[NormalizedRow]) -> Value {
    let mut types = BTreeMap::<String, usize>::new();
    let mut imports = BTreeSet::<String>::new();
    for row in rows {
        *types
            .entry(
                string_field(&row.record_type)
                    .unwrap_or_default()
                    .to_owned(),
            )
            .or_default() += 1;
        imports.extend(row_import_ids(row));
    }
    json!({"types":types,"import_ids":imports,"oura_appendix":oura_appendix(rows)})
}
fn oura_appendix(rows: &[NormalizedRow]) -> Vec<Value> {
    let mut output = Vec::new();
    for row in rows {
        let kind = string_field(&row.record_type).unwrap_or_default();
        let data = metadata(row);
        if kind == OURA_CARDIOVASCULAR_AGE {
            if let Some(value) = data
                .and_then(|data| data.get("pulse_wave_velocity"))
                .and_then(Value::as_f64)
            {
                output.push(json!({"label":"Pulse-wave velocity","detail":format!("{} m/s · Oura's measurement",number(value))}));
            }
        } else if matches!(kind, OURA_SESSION | OURA_TAG) {
            let label = data
                .and_then(|data| {
                    data.get(if kind == OURA_SESSION {
                        "type"
                    } else {
                        "custom_name"
                    })
                })
                .and_then(Value::as_str)
                .map(title)
                .unwrap_or_else(|| {
                    if kind == OURA_SESSION {
                        "Session".to_owned()
                    } else {
                        "Tag".to_owned()
                    }
                });
            let mut detail = record_time(row).map(clock).unwrap_or_default();
            if !detail.is_empty() {
                detail.push_str(" · ");
            }
            detail.push_str("Oura (API)");
            output.push(json!({"label":label,"detail":detail}));
        }
    }
    output
}
fn title(text: &str) -> String {
    text.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}
pub(crate) fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}
pub(crate) fn grouped_decimal(value: f64, places: usize) -> String {
    let raw = format!("{value:.places$}");
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let sign = whole.strip_prefix('-').map_or("", |_| "-");
    let digits = whole
        .trim_start_matches('-')
        .chars()
        .rev()
        .collect::<Vec<_>>();
    let grouped = digits
        .chunks(3)
        .map(|part| part.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(",")
        .chars()
        .rev()
        .collect::<String>();
    format!("{sign}{grouped}.{fraction}")
}

pub(crate) fn grouped_unsigned(value: u64) -> String {
    grouped_decimal(value as f64, 0)
        .trim_end_matches('.')
        .to_owned()
}

pub(crate) fn grouped_signed(value: i64) -> String {
    grouped_decimal(value as f64, 0)
        .trim_end_matches('.')
        .to_owned()
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{Map, Value, json};

    use super::*;
    use crate::{
        BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedManifest, TrendAnnotation,
        TrendCoverage, TrendSignal, TrendValue, TrendsPayload, read_health_dedupe_stats,
        replace_trends_cache, seed_body_journal, trends_db_path, trends_signature,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-body-day-corpus-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("temporary journal creates");
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

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("row object").clone()
    }
    fn iso(day: NaiveDate, clock: &str) -> String {
        format!("{}T{clock}+00:00", day.format("%Y-%m-%d"))
    }
    #[allow(clippy::too_many_arguments)]
    fn health_row(
        kind: &str,
        start: String,
        value: Option<Value>,
        unit: Option<&str>,
        source: &str,
        end: Option<String>,
        metadata: Option<Value>,
        dedupe: Option<String>,
    ) -> Map<String, Value> {
        let record_type = kind;
        let day = start[..10].replace('-', "");
        let mut row = object(
            json!({"schema":"solstone.health.normalized.v1","source_family":APPLE,"kind":"record","record_type":record_type,"day":day,"start_date":start,"source_name":source,"month":&start[..7]}),
        );
        if let Some(key) = dedupe {
            row.insert("dedupe_key".into(), json!(key));
        } else {
            row.insert(
                "dedupe_key".into(),
                json!(format!(
                    "{APPLE}:{record_type}:{source}:{}",
                    row["start_date"]
                )),
            );
        }
        if let Some(value) = value {
            row.insert("value".into(), value);
        }
        if let Some(unit) = unit {
            row.insert("unit".into(), json!(unit));
        }
        if let Some(end) = end {
            row.insert("end_date".into(), json!(end));
        }
        if let Some(metadata) = metadata {
            row.insert("metadata".into(), metadata);
        }
        row
    }
    #[allow(clippy::too_many_arguments)]
    fn oura_row(
        record_type: &str,
        day: &str,
        start: Option<String>,
        end: Option<String>,
        value: Option<Value>,
        unit: Option<&str>,
        kind: &str,
        metadata: Option<Value>,
        dedupe: Option<String>,
    ) -> Map<String, Value> {
        let start = start.unwrap_or_else(|| {
            format!("{}-{}-{}T04:00:00+00:00", &day[..4], &day[4..6], &day[6..])
        });
        let mut row = object(
            json!({"schema":"solstone.health.oura.v1","source_family":OURA_API,"kind":kind,"record_type":record_type,"day":day,"start_date":start,"source_record_id":format!("{record_type}-{day}"),"month":&start[..7]}),
        );
        row.insert(
            "dedupe_key".into(),
            json!(
                dedupe.unwrap_or_else(|| format!(
                    "oura-api:{record_type}:{day}:{}",
                    row["start_date"]
                ))
            ),
        );
        if let Some(end) = end {
            row.insert("end_date".into(), json!(end));
        }
        if let Some(value) = value {
            row.insert("value".into(), value);
        }
        if let Some(unit) = unit {
            row.insert("unit".into(), json!(unit));
        }
        if let Some(metadata) = metadata {
            row.insert("metadata".into(), metadata);
        }
        row
    }
    fn bundle(import_id: &str, source: &str, rows: Vec<Map<String, Value>>) -> BodySeedBundle {
        let days_affected = rows
            .iter()
            .filter_map(|row| row.get("day").and_then(Value::as_str))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|day| Value::String(day.to_owned()))
            .collect::<Vec<_>>();
        let mut extra = Map::new();
        if let Some(imported_at) = match import_id {
            "20260810_080000" => Some("2026-08-10T08:00:00Z"),
            "20260810_090000" => Some("2026-08-10T09:00:00Z"),
            "20260810_100000" => Some("2026-08-10T10:00:00Z"),
            _ => None,
        } {
            extra.insert("days_affected".to_owned(), Value::Array(days_affected));
            extra.insert(
                "imported_at".to_owned(),
                Value::String(imported_at.to_owned()),
            );
            extra.insert(
                "imported_via".to_owned(),
                Value::String("body-corpus".to_owned()),
            );
            extra.insert(
                "source_hash".to_owned(),
                Value::String(format!("sha256:body-corpus-{import_id}")),
            );
        }
        let mut shards = BTreeMap::<String, Vec<Map<String, Value>>>::new();
        for row in rows {
            let month = row["month"].as_str().expect("month").to_owned();
            shards.entry(month).or_default().push(row);
        }
        BodySeedBundle {
            import_id: import_id.to_owned(),
            source_family: source.to_owned(),
            manifest: BodySeedManifest::Present {
                source_type: Some(source.to_owned()),
                entry_count: Some(shards.values().map(Vec::len).sum::<usize>() as u64),
                extra,
            },
            shards,
        }
    }

    /// Rust transcription of the deterministic Python corpus seed.
    pub(crate) fn seed_populated_body_journal(root: &Path) -> crate::BodySeedReport {
        let anchor = NaiveDate::from_ymd_opt(2026, 8, 1).expect("anchor");
        let mut apple = Vec::new();
        let mut oura = Vec::new();
        let mut correction = Vec::new();
        let first = NaiveDate::from_ymd_opt(2026, 5, 2).unwrap();
        oura.push(oura_row(
            OURA_SLEEP,
            "20260502",
            Some(iso(first, "23:00:00")),
            Some(iso(first + Duration::days(1), "07:00:00")),
            Some(json!(28800)),
            Some("s"),
            "sleep_period",
            Some(json!({"lowest_heart_rate":53,"time_in_bed":28800})),
            None,
        ));
        let mut current = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let mut offset = 0i64;
        while current <= end {
            let day = day_key(current);
            apple.push(health_row(
                RESTING_HEART_RATE,
                iso(current, "07:00:00"),
                Some(json!((52 + offset % 5).to_string())),
                Some("count/min"),
                "Synthetic Watch",
                None,
                None,
                None,
            ));
            apple.push(health_row(
                "HKQuantityTypeIdentifierBodyMass",
                iso(current, "08:00:00"),
                Some(json!(170.0 + offset as f64 * 0.03)),
                Some("lb"),
                "Synthetic Scale",
                None,
                None,
                None,
            ));
            oura.push(oura_row(
                OURA_READINESS,
                &day,
                None,
                None,
                Some(json!(70 + offset % 12)),
                Some("score"),
                "daily_summary",
                None,
                None,
            ));
            oura.push(oura_row(
                OURA_SLEEP_SCORE,
                &day,
                None,
                None,
                Some(json!(78 + offset % 9)),
                Some("score"),
                "daily_summary",
                None,
                None,
            ));
            oura.push(oura_row(
                OURA_ACTIVITY,
                &day,
                None,
                None,
                Some(json!(80 + offset % 10)),
                Some("score"),
                "daily_summary",
                Some(json!({"steps":7000+offset*13})),
                None,
            ));
            let row_day = if current == end { "20260801" } else { &day };
            oura.push(oura_row(
                OURA_SLEEP,
                row_day,
                Some(iso(current, "23:00:00")),
                Some(iso(current + Duration::days(1), "07:00:00")),
                Some(json!(28800)),
                Some("s"),
                "sleep_period",
                Some(json!({"lowest_heart_rate":52+offset%5,"time_in_bed":28800})),
                None,
            ));
            current += Duration::days(1);
            offset += 1;
        }
        let collision = "oura-api:oura.daily_readiness:20260801".to_owned();
        oura.push(oura_row(
            OURA_READINESS,
            "20260801",
            None,
            None,
            Some(json!(76)),
            Some("score"),
            "daily_summary",
            Some(json!({"contributors":{"activity_balance":74,"sleep_balance":81}})),
            Some(collision.clone()),
        ));
        oura.push(oura_row(
            OURA_SLEEP_SCORE,
            "20260801",
            None,
            None,
            Some(json!(84)),
            Some("score"),
            "daily_summary",
            None,
            None,
        ));
        oura.push(oura_row(
            OURA_ACTIVITY,
            "20260801",
            None,
            None,
            Some(json!(88)),
            Some("score"),
            "daily_summary",
            Some(json!({"steps":11234})),
            None,
        ));
        oura.push(oura_row(OURA_WORKOUT,"20260801",Some(iso(anchor,"16:00:00")),Some(iso(anchor,"16:45:00")),None,None,"workout",Some(json!({"activity":"running","distance":7200,"distance_unit":"m","calories":510,"calories_unit":"kcal"})),None));
        correction.push(oura_row(
            OURA_READINESS,
            "20260801",
            None,
            None,
            Some(json!(82)),
            Some("score"),
            "daily_summary",
            Some(json!({"contributors":{"activity_balance":78,"sleep_balance":86}})),
            Some(collision),
        ));
        for index in 0..12 {
            apple.push(health_row(
                HEART_RATE,
                iso(anchor, &format!("08:{:02}:00", index * 5)),
                Some(json!((60 + index).to_string())),
                Some("count/min"),
                "Synthetic Watch",
                None,
                None,
                None,
            ));
        }
        apple.push(health_row(
            RESTING_HEART_RATE,
            iso(anchor, "07:00:00"),
            Some(json!("58")),
            Some("count/min"),
            "Synthetic Watch",
            None,
            None,
            None,
        ));
        for (clock, value) in [
            ("07:00:00", 91),
            ("11:00:00", 104),
            ("15:00:00", 98),
            ("19:00:00", 112),
        ] {
            apple.push(health_row(
                "HKQuantityTypeIdentifierBloodGlucose",
                iso(anchor, clock),
                Some(json!(value.to_string())),
                Some("mg/dL"),
                "Synthetic CGM",
                None,
                None,
                None,
            ));
        }
        apple.push(health_row(
            "HKQuantityTypeIdentifierBodyMass",
            iso(anchor, "08:00:00"),
            Some(json!(172.4)),
            Some("lb"),
            "Synthetic Scale",
            None,
            None,
            None,
        ));
        apple.push(health_row(
            "HKQuantityTypeIdentifierBodyFatPercentage",
            iso(anchor, "08:05:00"),
            Some(json!(0.18)),
            Some("%"),
            "Synthetic Scale",
            None,
            None,
            None,
        ));
        apple.push(health_row(
            "HKQuantityTypeIdentifierStepCount",
            iso(anchor, "00:00:00"),
            Some(json!("11234")),
            Some("count"),
            "Oura Ring",
            Some(iso(anchor + Duration::days(1), "00:00:00")),
            None,
            None,
        ));
        let mut cycling = health_row(
            "HKWorkoutActivityTypeCycling",
            iso(anchor, "17:00:00"),
            None,
            None,
            "Synthetic Watch",
            Some(iso(anchor, "18:00:00")),
            Some(
                json!({"totalDistance":18.2,"totalDistanceUnit":"km","totalEnergyBurned":620,"totalEnergyBurnedUnit":"kcal"}),
            ),
            None,
        );
        cycling.insert("kind".into(), json!("workout"));
        apple.push(cycling);
        for (start, end) in [("12:00:00", "12:10:00"), ("20:00:00", "20:15:00")] {
            apple.push(health_row(
                "HKCategoryTypeIdentifierMindfulSession",
                iso(anchor, start),
                None,
                None,
                "Synthetic Watch",
                Some(iso(anchor, end)),
                None,
                None,
            ));
        }
        for (clock, value) in [("13:00:00", "71"), ("13:30:00", "76")] {
            apple.push(health_row(
                "HKQuantityTypeIdentifierHeadphoneAudioExposure",
                iso(anchor, clock),
                Some(json!(value)),
                Some("dBASPL"),
                "Synthetic Watch",
                None,
                None,
                None,
            ));
        }
        apple.push(health_row(
            "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
            iso(anchor, "22:00:00"),
            Some(json!("36.5")),
            Some("degC"),
            "Synthetic Watch",
            None,
            None,
            Some(String::new()),
        ));
        let seed = BodyJournalSeed {
            dates: BTreeSet::new(),
            day_summaries: BTreeMap::from([(
                "20260801".into(),
                "# Synthetic Body Summary\n\nA deterministic corpus day.\n".into(),
            )]),
            bundles: vec![
                bundle("20260810_080000", APPLE, apple),
                bundle("20260810_090000", OURA_API, oura),
                bundle("20260810_100000", OURA_API, correction),
                BodySeedBundle {
                    import_id: "unknown-source".into(),
                    source_family: "unknown_health_source".into(),
                    manifest: BodySeedManifest::Present {
                        source_type: Some("unknown_health_source".into()),
                        entry_count: None,
                        extra: Map::new(),
                    },
                    shards: BTreeMap::new(),
                },
            ],
            aggregate: BodyAggregateSeed::Direct,
            journal_config: None,
        };
        let report = seed_body_journal(root, &seed).expect("corpus journal seeds");
        // The Python aggregate skips the one intentionally keyless row; the
        // generic Rust fixture writer otherwise records every supplied row.
        rusqlite::Connection::open(root.join("imports/health-dedupe.sqlite"))
            .expect("aggregate opens")
            .execute("DELETE FROM health_dedupe WHERE dedupe_key = ''", [])
            .expect("keyless aggregate row removes");
        let unreadable = root.join("imports/unreadable-manifest");
        fs::create_dir_all(&unreadable).expect("broken bundle creates");
        fs::write(
            unreadable.join("manifest.json"),
            "{\"import_id\": \"unreadable\"",
        )
        .expect("broken manifest writes");
        report
    }

    fn row(import_id: &str, dedupe_key: &str) -> NormalizedRow {
        let path = std::env::temp_dir().join(format!(
            "solstone-body-day-{}-{}.jsonl",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            format!(
                "{}\n",
                json!({
                    "day":"20260101", "dedupe_key":dedupe_key,
                    "import_id":import_id, "record_type":"Signal"
                })
            ),
        )
        .expect("row writes");
        let row = crate::read_normalized_shard(&path)
            .expect("row parses")
            .pop()
            .expect("one row");
        fs::remove_file(path).expect("temporary row removes");
        row
    }
    fn simple_row(kind: &str, day: &str, start: &str, value: Option<Value>) -> NormalizedRow {
        let mut row = row("fixture", &format!("fixture:{kind}:{start}"));
        row.day = FieldState::Present(json!(day));
        row.record_type = FieldState::Present(json!(kind));
        row.start_date = FieldState::Present(json!(start));
        if let Some(value) = value {
            row.value = ValueState::Present(
                solstone_core_body_source::parse(&serde_json::to_vec(&value).expect("value"))
                    .expect("body value"),
            );
        }
        row
    }
    fn seed_fixture(root: &Path, bundles: Vec<BodySeedBundle>) {
        seed_body_journal(
            root,
            &BodyJournalSeed {
                dates: BTreeSet::new(),
                day_summaries: BTreeMap::new(),
                bundles,
                aggregate: BodyAggregateSeed::Direct,
                journal_config: None,
            },
        )
        .expect("fixture seeds");
    }

    mod routing_and_validation {
        use super::*;
        use axum::body::{Body, to_bytes};
        use axum::http::Request;
        use tower::ServiceExt;

        #[test]
        fn calendar_validation_requires_eight_digits_and_a_real_day() {
            assert_eq!(parse_day("20260229"), None);
            assert_eq!(parse_day("20260228").unwrap().day(), 28);
            assert_eq!(parse_day("2026-02-28"), None);
        }

        #[tokio::test]
        async fn invalid_days_use_the_reference_envelope() {
            for day in ["abc", "2026-08-01", "20260231"] {
                let response = crate::api_router(TempDir::new().path())
                    .oneshot(
                        Request::get(format!("/app/body/api/day/{day}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
                let body: Value = serde_json::from_slice(
                    &to_bytes(response.into_body(), usize::MAX).await.unwrap(),
                )
                .unwrap();
                assert_eq!(
                    body,
                    json!({"reason_code":"invalid_day","error":"that day couldn't be used.","detail":"Invalid day"})
                );
            }
        }
    }

    mod cross_month_dedupe_and_mirrors {
        use super::*;

        #[test]
        fn month_boundary_reads_prior_and_next_only_when_required() {
            assert_eq!(
                day_months(parse_day("20260201").unwrap()),
                ["2026-02", "2026-01"]
            );
            assert_eq!(
                day_months(parse_day("20260131").unwrap()),
                ["2026-01", "2026-02"]
            );
        }

        #[test]
        fn later_equal_import_id_wins_and_keeps_all_imports() {
            let rows = dedupe_cross_month(vec![row("b", "same"), row("b", "same")]);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].import_ids, ["b"]);
        }

        #[test]
        fn oura_heart_mirror_only_supersedes_the_exact_heart_rate_type() {
            let mut api = row("api", "api-heart");
            api.source_family = FieldState::Present(json!(OURA_API));
            api.record_type = FieldState::Present(json!(OURA_HEART_RATE));
            let mut hrv_mirror = row("apple", "mirror-hrv");
            hrv_mirror.source_family = FieldState::Present(json!(APPLE));
            hrv_mirror.source_name = FieldState::Present(json!("Oura"));
            hrv_mirror.record_type =
                FieldState::Present(json!("HKQuantityTypeIdentifierHeartRateVariabilitySDNN"));
            assert_eq!(resolve_canonical_rows(vec![api, hrv_mirror]).len(), 2);
        }

        #[test]
        fn mirror_rows_without_an_api_pipe_remain_in_the_payload() {
            let root = TempDir::new();
            let heart = health_row(
                HEART_RATE,
                "2026-01-01T08:00:00+00:00".to_owned(),
                Some(json!(60)),
                Some("count/min"),
                "Oura Ring",
                None,
                None,
                None,
            );
            let hrv = health_row(
                "HKQuantityTypeIdentifierHeartRateVariabilitySDNN",
                "2026-01-01T08:05:00+00:00".to_owned(),
                Some(json!(40)),
                Some("ms"),
                "Oura Ring",
                None,
                None,
                None,
            );
            seed_fixture(
                root.path(),
                vec![bundle("20260102_080000", APPLE, vec![heart, hrv])],
            );
            let mut reader = MonthReader::new(root.path());
            let payload = build_day(
                root.path(),
                parse_day("20260101").unwrap(),
                None,
                &mut reader,
            )
            .unwrap();
            let audit_total = payload["audit"]["types"]
                .as_object()
                .unwrap()
                .values()
                .map(Value::as_u64)
                .sum::<Option<u64>>()
                .unwrap();
            assert_eq!(payload["entry_total"], audit_total);
            assert!(payload["audit"]["types"].get(HEART_RATE).is_some());
            assert!(
                payload["audit"]["types"]
                    .get("HKQuantityTypeIdentifierHeartRateVariabilitySDNN")
                    .is_some()
            );
        }

        #[test]
        fn cross_month_latest_import_wins_and_keeps_sorted_union() {
            let mut older = simple_row(
                "Signal",
                "20260102",
                "2025-12-31T23:00:00+00:00",
                Some(json!(1)),
            );
            older.dedupe_key = FieldState::Present(json!("collision"));
            older.import_id = FieldState::Present(json!("20260101_090000"));
            older.import_ids = vec!["20260101_090000".into()];
            let mut newer = older.clone();
            newer.value = ValueState::Present(solstone_core_body_source::parse(b"2").unwrap());
            newer.import_id = FieldState::Present(json!("20260101_100000"));
            newer.import_ids = vec!["20260101_100000".into()];
            let rows = dedupe_cross_month(vec![older, newer]);
            assert_eq!(value_number(&rows[0]), Some(2.0));
            assert_eq!(rows[0].import_ids, ["20260101_090000", "20260101_100000"]);
        }

        #[test]
        fn audit_only_oura_rows_stay_visible_outside_leftovers() {
            let mut session =
                simple_row(OURA_SESSION, "20260101", "2026-01-01T10:00:00+00:00", None);
            session.source_family = FieldState::Present(json!(OURA_API));
            let mut tag = session.clone();
            tag.record_type = FieldState::Present(json!(OURA_TAG));
            let other = vec![session, tag];
            assert!(fact_items(&other).len() == 2);
            let leftovers = other
                .into_iter()
                .filter(|row| {
                    !matches!(
                        string_field(&row.record_type),
                        Some(OURA_SESSION | OURA_TAG)
                    )
                })
                .collect::<Vec<_>>();
            assert!(leftovers.is_empty());
            assert_eq!(
                lede(
                    &[simple_row(
                        OURA_SESSION,
                        "20260101",
                        "2026-01-01T10:00:00+00:00",
                        None
                    )],
                    None,
                    &[],
                    None
                ),
                "1 entry across Other."
            );
        }

        #[test]
        fn fact_items_cover_duration_range_and_single_value() {
            let mut mindful = simple_row(
                "HKCategoryTypeIdentifierMindfulSession",
                "20260101",
                "2026-01-01T10:00:00+00:00",
                None,
            );
            mindful.end_date = FieldState::Present(json!("2026-01-01T10:15:00+00:00"));
            let mut mindful_two = mindful.clone();
            mindful_two.start_date = FieldState::Present(json!("2026-01-01T12:00:00+00:00"));
            mindful_two.end_date = FieldState::Present(json!("2026-01-01T12:10:00+00:00"));
            let mut audio = simple_row(
                "HKQuantityTypeIdentifierHeadphoneAudioExposure",
                "20260101",
                "2026-01-01T13:00:00+00:00",
                Some(json!(71)),
            );
            audio.unit = FieldState::Present(json!("dBASPL"));
            let mut audio_two = audio.clone();
            audio_two.value = ValueState::Present(solstone_core_body_source::parse(b"76").unwrap());
            let mut single = simple_row(
                "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
                "20260101",
                "2026-01-01T22:00:00+00:00",
                Some(json!(36.5)),
            );
            single.unit = FieldState::Present(json!("degC"));
            let facts = fact_items(&[mindful, mindful_two, audio, audio_two, single]);
            assert!(facts.iter().any(|fact| fact["value"] == "25m"));
            assert!(
                facts
                    .iter()
                    .any(|fact| fact["value"] == "2 entries · 71–76 dB")
            );
            assert!(facts.iter().any(|fact| fact["value"] == "36.5 °C"));
        }

        #[test]
        fn day_two_uses_prior_month_collision_winner_and_audit_ids_are_exact() {
            let root = TempDir::new();
            let mut older = health_row(
                "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
                "2026-03-02T08:00:00+00:00".to_owned(),
                Some(json!(36.1)),
                Some("degC"),
                "Watch",
                None,
                None,
                Some("collision".into()),
            );
            older.insert("day".into(), json!("20260302"));
            older.insert("month".into(), json!("2026-02"));
            let mut newer = older.clone();
            newer.insert("value".into(), json!(36.9));
            let mut off_day = older.clone();
            off_day.insert("day".into(), json!("20260301"));
            off_day.insert("dedupe_key".into(), json!("off-day"));
            seed_fixture(
                root.path(),
                vec![
                    bundle("20260301_090000", APPLE, vec![older]),
                    bundle("20260301_100000", APPLE, vec![newer]),
                    bundle("20260301_110000", APPLE, vec![off_day]),
                ],
            );
            let mut reader = MonthReader::new(root.path());
            let payload = build_day(
                root.path(),
                parse_day("20260302").unwrap(),
                None,
                &mut reader,
            )
            .unwrap();
            assert_eq!(payload["other_signals"]["facts"][0]["value"], "36.9 °C");
            assert_eq!(
                payload["audit"]["import_ids"],
                json!(["20260301_090000", "20260301_100000"])
            );
        }

        #[test]
        fn sleep_adjacent_strings_and_null_preserve_the_null_fact() {
            let mut stage = simple_row(
                "HKCategoryTypeIdentifierSleepAnalysis",
                "20260101",
                "2026-01-01T22:00:00+00:00",
                Some(json!("asleep core")),
            );
            stage.end_date = FieldState::Present(json!("2026-01-02T06:00:00+00:00"));
            let score = simple_row(
                OURA_SLEEP_SCORE,
                "20260102",
                "2026-01-02T04:00:00+00:00",
                Some(json!("good")),
            );
            let null = simple_row(
                "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
                "20260101",
                "2026-01-01T09:00:00+00:00",
                Some(Value::Null),
            );
            let sleep = sleep_analysis(
                &[score],
                &[stage],
                &[],
                parse_day("20260102").unwrap(),
                &BTreeMap::new(),
            )
            .expect("stage string builds sleep");
            assert_eq!(sleep["asleep_duration"], "8h 00m");
            assert_eq!(sleep["has_stage_detail"], true);
            let facts = fact_items(&[null]);
            assert_eq!(facts[0]["label"], "Wrist temperature");
            assert!(facts[0]["value"].is_null());
        }

        #[test]
        fn passthrough_rows_precede_keyed_rows_for_first_readiness_lookup() {
            let mut keyed = simple_row(
                OURA_READINESS,
                "20260101",
                "2026-01-01T04:00:00+00:00",
                Some(json!(70)),
            );
            keyed.dedupe_key = FieldState::Present(json!("keyed"));
            let mut passthrough = keyed.clone();
            passthrough.dedupe_key = FieldState::Absent;
            passthrough.value =
                ValueState::Present(solstone_core_body_source::parse(b"90").unwrap());
            let ordered = dedupe_cross_month(vec![keyed, passthrough]);
            assert_eq!(
                recovery_analysis(&ordered, &BTreeMap::new()).unwrap()["facts"][0]["detail"],
                "90 · Oura's score"
            );
        }
    }

    mod sleep_and_boundaries {
        use super::*;

        #[test]
        fn six_pm_axis_clamps_to_the_display_day() {
            let target = parse_day("20260102").unwrap();
            let axis_day = target - Duration::days(1);
            let minute = (target.and_hms_opt(0, 0, 0).unwrap().date() - axis_day).num_days() * 1440
                - SLEEP_AXIS_START_HOUR * 60;
            assert_eq!(minute, 360);
        }

        #[test]
        fn main_session_and_nap_keep_their_separate_windows() {
            let target = parse_day("20260102").unwrap();
            let mut night = simple_row(
                OURA_SLEEP,
                "20260101",
                "2026-01-01T22:00:00+00:00",
                Some(json!(1)),
            );
            night.end_date = FieldState::Present(json!("2026-01-02T06:00:00+00:00"));
            let mut nap = simple_row(
                OURA_SLEEP,
                "20260102",
                "2026-01-02T14:00:00+00:00",
                Some(json!(1)),
            );
            nap.end_date = FieldState::Present(json!("2026-01-02T14:30:00+00:00"));
            let sleep =
                sleep_analysis(&[nap], &[night], &[], target, &BTreeMap::new()).expect("sleep");
            assert_eq!(sleep["duration"], "8h 00m");
            assert_eq!(sleep["naps"].as_array().unwrap().len(), 1);
        }

        #[test]
        fn seeded_naps_appear_in_the_sleep_list_and_bar_segments() {
            let root = TempDir::new();
            let main = health_row(
                "HKCategoryTypeIdentifierSleepAnalysis",
                "2026-01-01T22:00:00+00:00".to_owned(),
                None,
                None,
                "Primary Watch",
                Some("2026-01-02T06:00:00+00:00".to_owned()),
                None,
                None,
            );
            let nap = health_row(
                "HKCategoryTypeIdentifierSleepAnalysis",
                "2026-01-02T14:00:00+00:00".to_owned(),
                None,
                None,
                "Primary Watch",
                Some("2026-01-02T14:30:00+00:00".to_owned()),
                None,
                None,
            );
            seed_fixture(
                root.path(),
                vec![bundle("20260103_080000", APPLE, vec![main, nap])],
            );
            let mut reader = MonthReader::new(root.path());
            let payload = build_day(
                root.path(),
                parse_day("20260102").unwrap(),
                None,
                &mut reader,
            )
            .unwrap();
            assert_eq!(payload["sleep"]["naps"].as_array().unwrap().len(), 1);
            assert!(
                payload["sleep"]["bar"]["segments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|segment| segment["kind"] == "nap")
            );
        }

        #[test]
        fn seeded_secondary_sleep_source_is_named_without_summing_coverage() {
            let root = TempDir::new();
            let primary = health_row(
                "HKCategoryTypeIdentifierSleepAnalysis",
                "2026-01-01T21:00:00+00:00".to_owned(),
                None,
                None,
                "Primary Watch",
                Some("2026-01-02T07:00:00+00:00".to_owned()),
                None,
                None,
            );
            let secondary = health_row(
                "HKCategoryTypeIdentifierSleepAnalysis",
                "2026-01-01T22:00:00+00:00".to_owned(),
                None,
                None,
                "Secondary Band",
                Some("2026-01-02T06:00:00+00:00".to_owned()),
                None,
                None,
            );
            seed_fixture(
                root.path(),
                vec![bundle("20260103_080000", APPLE, vec![primary, secondary])],
            );
            let mut reader = MonthReader::new(root.path());
            let payload = build_day(
                root.path(),
                parse_day("20260102").unwrap(),
                None,
                &mut reader,
            )
            .unwrap();
            assert_eq!(payload["sleep"]["source"], "Primary Watch");
            assert_eq!(payload["sleep"]["duration"], "10h 00m");
            assert_eq!(payload["sleep"]["other_sources"], json!(["Secondary Band"]));
        }

        #[test]
        fn longest_source_wins_and_axis_end_naps_drop() {
            let target = parse_day("20260102").unwrap();
            let mut short = simple_row(
                OURA_SLEEP,
                "20260101",
                "2026-01-01T23:00:00+00:00",
                Some(json!(1)),
            );
            short.end_date = FieldState::Present(json!("2026-01-02T05:00:00+00:00"));
            short.source_name = FieldState::Present(json!("Watch"));
            let mut long = short.clone();
            long.source_name = FieldState::Present(json!("Ring"));
            long.start_date = FieldState::Present(json!("2026-01-01T21:00:00+00:00"));
            let mut clipped = simple_row(
                OURA_SLEEP,
                "20260102",
                "2026-01-02T17:59:00+00:00",
                Some(json!(1)),
            );
            clipped.end_date = FieldState::Present(json!("2026-01-02T18:02:00+00:00"));
            clipped.source_name = FieldState::Present(json!("Ring"));
            let mut dropped = simple_row(
                OURA_SLEEP,
                "20260102",
                "2026-01-02T18:00:00+00:00",
                Some(json!(1)),
            );
            dropped.end_date = FieldState::Present(json!("2026-01-02T18:10:00+00:00"));
            dropped.source_name = FieldState::Present(json!("Ring"));
            let sleep = sleep_analysis(
                &[clipped, dropped],
                &[short, long],
                &[],
                target,
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(sleep["duration"], "8h 00m");
            assert_eq!(sleep["naps"].as_array().unwrap().len(), 1);
            assert_eq!(
                sleep["bar"]["segments"][1],
                json!({"x":1436.0,"width":4.0,"kind":"nap"})
            );
        }

        #[test]
        fn duration_fields_distinguish_full_partial_and_missing_stage_detail() {
            let target = parse_day("20260102").unwrap();
            let session = |kind: &str, stage: Option<&str>, end: &str| {
                let mut row = simple_row(
                    kind,
                    "20260101",
                    "2026-01-01T22:00:00+00:00",
                    Some(json!(1)),
                );
                row.end_date = FieldState::Present(json!(end));
                if let Some(stage) = stage {
                    row.metadata = FieldState::Present(json!({"stage":stage}));
                }
                row
            };
            let full = sleep_analysis(
                &[],
                &[session(
                    OURA_SLEEP,
                    Some("asleep"),
                    "2026-01-02T06:00:00+00:00",
                )],
                &[],
                target,
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(full["duration"], full["asleep_duration"]);
            assert_eq!(full["duration"], full["in_bed_duration"]);
            let bare = sleep_analysis(
                &[],
                &[session(
                    "HKCategoryTypeIdentifierSleepAnalysis",
                    None,
                    "2026-01-02T06:00:00+00:00",
                )],
                &[],
                target,
                &BTreeMap::new(),
            )
            .unwrap();
            assert!(bare["asleep_duration"].is_null());
            assert_eq!(bare["in_bed_duration"], "8h 00m");
            let mut in_bed = session(
                "HKCategoryTypeIdentifierSleepAnalysis",
                Some("in bed"),
                "2026-01-02T06:00:00+00:00",
            );
            let mut asleep = session(
                "HKCategoryTypeIdentifierSleepAnalysis",
                Some("asleep"),
                "2026-01-02T05:00:00+00:00",
            );
            asleep.start_date = FieldState::Present(json!("2026-01-01T23:00:00+00:00"));
            in_bed.source_name = FieldState::Present(json!("Watch"));
            asleep.source_name = FieldState::Present(json!("Watch"));
            let partial =
                sleep_analysis(&[], &[in_bed, asleep], &[], target, &BTreeMap::new()).unwrap();
            assert_eq!(partial["duration"], "8h 00m");
            assert_eq!(partial["in_bed_duration"], "8h 00m");
            assert_eq!(partial["asleep_duration"], "6h 00m");
        }

        #[test]
        fn warmed_typical_is_cache_scoped_not_shard_month_scoped() {
            let root = TempDir::new();
            let signal = crate::TrendSignal {
                key: "readiness".into(),
                label: "Readiness".into(),
                unit_label: String::new(),
                daily: (1..=14)
                    .map(|offset| {
                        (
                            (parse_day("20260120").unwrap() - Duration::days(offset))
                                .format("%Y%m%d")
                                .to_string(),
                            TrendValue::Real(75.0),
                        )
                    })
                    .collect(),
                coverage: crate::TrendCoverage {
                    first_day: "20260106".into(),
                    last_day: "20260119".into(),
                    days: 14,
                },
            };
            crate::replace_trends_cache(
                trends_db_path(root.path()),
                trends_signature(root.path()).unwrap(),
                crate::TrendsPayload {
                    signals: vec![signal],
                    annotations: vec![],
                    generated_at_day: "20260120".into(),
                },
            )
            .unwrap();
            assert_eq!(
                warmed_typical(root.path(), "20260120")
                    .unwrap()
                    .get("readiness"),
                Some(&75.0)
            );
        }
        #[test]
        fn month_end_bedtime_is_not_a_nap_but_midmonth_nap_is() {
            let target = parse_day("20260131").unwrap();
            let mut bedtime = simple_row(
                OURA_SLEEP,
                "20260131",
                "2026-01-31T23:00:00+00:00",
                Some(json!(1)),
            );
            bedtime.end_date = FieldState::Present(json!("2026-02-01T07:00:00+00:00"));
            assert!(sleep_analysis(&[bedtime], &[], &[], target, &BTreeMap::new()).is_none());
            let mut nap = simple_row(
                "HKCategoryTypeIdentifierSleepAnalysis",
                "20260115",
                "2026-01-15T14:00:00+00:00",
                Some(json!(1)),
            );
            nap.end_date = FieldState::Present(json!("2026-01-15T14:30:00+00:00"));
            let mut main = simple_row(
                "HKCategoryTypeIdentifierSleepAnalysis",
                "20260114",
                "2026-01-14T22:00:00+00:00",
                Some(json!(1)),
            );
            main.end_date = FieldState::Present(json!("2026-01-15T06:00:00+00:00"));
            assert_eq!(
                sleep_analysis(
                    &[nap],
                    &[main],
                    &[],
                    parse_day("20260115").unwrap(),
                    &BTreeMap::new()
                )
                .unwrap()["naps"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }

        #[test]
        fn sleep_comparison_requires_ring_and_cross_device_main_sessions() {
            let target = parse_day("20260102").unwrap();
            let session = |source: &str| {
                let mut row = simple_row(
                    "HKCategoryTypeIdentifierSleepAnalysis",
                    "20260101",
                    "2026-01-01T22:00:00+00:00",
                    Some(json!(1)),
                );
                row.source_name = FieldState::Present(json!(source));
                row.end_date = FieldState::Present(json!("2026-01-02T06:00:00+00:00"));
                row
            };
            let dual = sleep_analysis(
                &[],
                &[session("Watch"), session("Oura Ring")],
                &[],
                target,
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(
                dual["comparison_line"],
                "Watch saw 8h 00m · Oura Ring saw 8h 00m"
            );
            let ring_only =
                sleep_analysis(&[], &[session("Oura Ring")], &[], target, &BTreeMap::new())
                    .unwrap();
            assert!(ring_only["comparison_line"].is_null());
        }
    }

    mod activity_and_sources {
        use super::*;
        #[test]
        fn equal_coverage_primary_totals_choose_alphabetically_first() {
            let mut a = simple_row(
                "HKQuantityTypeIdentifierStepCount",
                "20260101",
                "2026-01-01T00:00:00+00:00",
                Some(json!(10)),
            );
            a.end_date = FieldState::Present(json!("2026-01-01T01:00:00+00:00"));
            a.source_name = FieldState::Present(json!("Alpha"));
            let mut z = a.clone();
            z.source_name = FieldState::Present(json!("Zulu"));
            let total = primary_total(&[z, a]).unwrap();
            assert_eq!(total.1, "Alpha");
            assert_eq!(total.3, vec!["Zulu".to_owned()]);
        }

        #[test]
        fn secondary_step_source_is_named_without_contributing_to_the_total() {
            let root = TempDir::new();
            let primary = health_row(
                "HKQuantityTypeIdentifierStepCount",
                "2026-01-01T00:00:00+00:00".to_owned(),
                Some(json!(1200)),
                Some("count"),
                "Primary Watch",
                Some("2026-01-01T02:00:00+00:00".to_owned()),
                None,
                None,
            );
            let secondary = health_row(
                "HKQuantityTypeIdentifierStepCount",
                "2026-01-01T03:00:00+00:00".to_owned(),
                Some(json!(800)),
                Some("count"),
                "Secondary Phone",
                Some("2026-01-01T04:00:00+00:00".to_owned()),
                None,
                None,
            );
            seed_fixture(
                root.path(),
                vec![bundle("20260102_080000", APPLE, vec![primary, secondary])],
            );
            let mut reader = MonthReader::new(root.path());
            let payload = build_day(
                root.path(),
                parse_day("20260101").unwrap(),
                None,
                &mut reader,
            )
            .unwrap();
            let steps = payload["activity"]["steps"].clone();
            assert_eq!(steps["total"], 1200);
            assert_eq!(steps["others"], json!(["Secondary Phone"]));
            assert_eq!(steps["others_label"], "Secondary Phone also contributed");
        }

        #[test]
        fn single_source_steps_without_an_interval_keep_the_total_mode() {
            let mut row = simple_row(
                "HKQuantityTypeIdentifierStepCount",
                "20260101",
                "2026-01-01T08:00:00+00:00",
                Some(json!(1234)),
            );
            row.source_name = FieldState::Present(json!("Watch"));
            let steps = activity_analysis(&[row]).unwrap()["steps"].clone();
            assert_eq!(steps["mode"], "total");
            assert_eq!(steps["total"], 1234);
            assert_eq!(steps["source"], "Watch");
        }
        #[test]
        fn workout_kind_is_the_discriminator() {
            let base = simple_row(
                "HKWorkoutActivityTypeRunning",
                "20260101",
                "2026-01-01T10:00:00+00:00",
                None,
            );
            assert!(
                activity_analysis(std::slice::from_ref(&base)).unwrap()["workouts"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            let mut workout = base;
            workout.kind = FieldState::Present(json!("workout"));
            assert_eq!(
                activity_analysis(&[workout]).unwrap()["workouts"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }

        #[test]
        fn running_dynamics_formats_pace_and_keeps_nonrunning_days_empty() {
            let mut speed = simple_row(
                RUNNING_SPEED,
                "20260101",
                "2026-01-01T06:00:00+00:00",
                Some(json!(2.5)),
            );
            speed.unit = FieldState::Present(json!("m/s"));
            let mut speed_later = speed.clone();
            speed_later.start_date = FieldState::Present(json!("2026-01-01T06:01:00+00:00"));
            speed_later.value =
                ValueState::Present(solstone_core_body_source::parse(b"3.125").unwrap());
            let mut power = simple_row(
                RUNNING_POWER,
                "20260101",
                "2026-01-01T06:00:00+00:00",
                Some(json!(240)),
            );
            power.unit = FieldState::Present(json!("W"));
            let mut power_later = power.clone();
            power_later.value =
                ValueState::Present(solstone_core_body_source::parse(b"260").unwrap());
            let activity = activity_analysis(&[speed, speed_later, power, power_later]).unwrap();
            assert_eq!(activity["running"][0]["count"], 2);
            assert_eq!(activity["running"][0]["summary"], "240–260 W · avg 250");
            assert_eq!(
                activity["running"][1]["summary"],
                "5:20–6:40 /km · avg 5:56 /km"
            );
            let empty = activity_analysis(&[simple_row(
                "HKQuantityTypeIdentifierStepCount",
                "20260101",
                "2026-01-01T00:00:00+00:00",
                Some(json!(1)),
            )])
            .unwrap();
            assert!(empty["running"].is_null());
        }

        #[test]
        fn running_speed_unsupported_and_inconsistent_units_are_distinct() {
            // This is the consistent-unit, unsupported pace-table case.
            let mut mph = simple_row(
                RUNNING_SPEED,
                "20260101",
                "2026-01-01T06:00:00+00:00",
                Some(json!(5)),
            );
            mph.unit = FieldState::Present(json!("mph"));
            let mut mph_later = mph.clone();
            mph_later.value = ValueState::Present(solstone_core_body_source::parse(b"6").unwrap());
            assert_eq!(
                running_dynamics(&[&mph, &mph_later])[0]["summary"],
                "5–6 mph · avg 5.5"
            );
            // This is the inconsistent-unit case, distinct from unsupported pace units.
            let mut metric = mph.clone();
            metric.unit = FieldState::Present(json!("m/s"));
            assert!(running_dynamics(&[&mph, &metric])[0]["summary"].is_null());
        }
    }

    mod heart_glucose_and_recovery {
        use super::*;
        #[test]
        fn curve_threshold_is_twelve_readings() {
            let rows = (0..11)
                .map(|index| {
                    let mut row = simple_row(
                        HEART_RATE,
                        "20260101",
                        &format!("2026-01-01T08:{:02}:00+00:00", index),
                        Some(json!(60 + index)),
                    );
                    row.unit = FieldState::Present(json!("count/min"));
                    row
                })
                .collect::<Vec<_>>();
            assert!(heart_analysis(&rows, &BTreeMap::new()).unwrap()["series"].is_null());
            let mut twelve = rows;
            twelve.push({
                let mut row = simple_row(
                    HEART_RATE,
                    "20260101",
                    "2026-01-01T09:00:00+00:00",
                    Some(json!(72)),
                );
                row.unit = FieldState::Present(json!("count/min"));
                row
            });
            assert_eq!(
                heart_analysis(&twelve, &BTreeMap::new()).unwrap()["series"]["count"],
                12
            );
        }

        #[test]
        fn curve_combines_display_equivalent_units_across_pipes() {
            let rows = (0..HEART_CURVE_MIN_READINGS)
                .map(|index| {
                    let is_oura = index % 2 == 1;
                    let mut row = simple_row(
                        if is_oura { OURA_HEART_RATE } else { HEART_RATE },
                        "20260101",
                        &format!("2026-01-01T08:{:02}:00+00:00", index),
                        Some(json!(60 + index)),
                    );
                    row.unit =
                        FieldState::Present(json!(if is_oura { "bpm" } else { "count/min" }));
                    row.source_family =
                        FieldState::Present(json!(if is_oura { OURA_API } else { APPLE }));
                    row.source_name =
                        FieldState::Present(json!(if is_oura { "Oura API" } else { "Watch" }));
                    row
                })
                .collect::<Vec<_>>();
            let series = heart_analysis(&rows, &BTreeMap::new()).unwrap()["series"].clone();
            assert_eq!(series["count"], HEART_CURVE_MIN_READINGS);
            assert_eq!(series["unit_label"], "bpm");
        }
        #[test]
        fn ring_resting_hr_is_a_comparison_not_a_second_fact() {
            let mut watch = simple_row(
                RESTING_HEART_RATE,
                "20260101",
                "2026-01-01T07:00:00+00:00",
                Some(json!(58)),
            );
            watch.unit = FieldState::Present(json!("count/min"));
            watch.source_name = FieldState::Present(json!("Watch"));
            let mut ring = simple_row(
                OURA_SLEEP,
                "20260101",
                "2026-01-01T23:00:00+00:00",
                Some(json!(1)),
            );
            ring.metadata = FieldState::Present(json!({"lowest_heart_rate":56}));
            let card = heart_analysis(&[watch, ring], &BTreeMap::new()).unwrap();
            assert_eq!(card["facts"].as_array().unwrap().len(), 1);
            assert_eq!(
                card["resting_comparison_line"],
                "Watch 58 bpm · Oura (API) 56 bpm"
            );
        }

        #[test]
        fn ring_only_resting_hr_has_no_comparison_line() {
            let mut ring = simple_row(
                OURA_SLEEP,
                "20260101",
                "2026-01-01T23:00:00+00:00",
                Some(json!(1)),
            );
            ring.metadata = FieldState::Present(json!({"lowest_heart_rate":56}));
            let card = heart_analysis(&[ring], &BTreeMap::new()).unwrap();
            assert_eq!(card["facts"][0]["value"], "56 bpm · Oura's measurement");
            assert!(card["resting_comparison_line"].is_null());
        }

        #[test]
        fn blood_pressure_pairs_and_unpaired_rows_follow_the_narrow_fallback() {
            let systolic = |start: &str, value: i64| {
                let mut row = simple_row(
                    "HKQuantityTypeIdentifierBloodPressureSystolic",
                    "20260101",
                    start,
                    Some(json!(value)),
                );
                row.unit = FieldState::Present(json!("mmHg"));
                row
            };
            let diastolic = |start: &str, value: i64| {
                let mut row = simple_row(
                    "HKQuantityTypeIdentifierBloodPressureDiastolic",
                    "20260101",
                    start,
                    Some(json!(value)),
                );
                row.unit = FieldState::Present(json!("mmHg"));
                row
            };
            let unpaired = heart_analysis(
                &[systolic("2026-01-01T08:00:00+00:00", 122)],
                &BTreeMap::new(),
            )
            .unwrap();
            assert!(unpaired["blood_pressure"].is_null());
            assert_eq!(unpaired["facts"][0]["label"], "Blood pressure (systolic)");
            let paired = heart_analysis(
                &[
                    systolic("2026-01-01T08:30:00+00:00", 122),
                    diastolic("2026-01-01T08:30:00+00:00", 78),
                ],
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(paired["blood_pressure"]["mode"], "readings");
            assert_eq!(paired["blood_pressure"]["unit"], "mmHg");
            assert_eq!(
                paired["blood_pressure"]["readings"][0]["label"],
                "122/78 mmHg"
            );
            assert!(paired["facts"].as_array().unwrap().is_empty());
        }

        #[test]
        fn paired_blood_pressure_hides_leftover_and_many_pairs_use_ranges() {
            let bp = |kind: &str, start: String, value: i64| {
                let mut row = simple_row(kind, "20260101", &start, Some(json!(value)));
                row.unit = FieldState::Present(json!("mmHg"));
                row
            };
            let leftover = heart_analysis(
                &[
                    bp(
                        "HKQuantityTypeIdentifierBloodPressureSystolic",
                        "2026-01-01T08:00:00+00:00".to_owned(),
                        120,
                    ),
                    bp(
                        "HKQuantityTypeIdentifierBloodPressureDiastolic",
                        "2026-01-01T08:00:00+00:00".to_owned(),
                        80,
                    ),
                    bp(
                        "HKQuantityTypeIdentifierBloodPressureSystolic",
                        "2026-01-01T09:00:00+00:00".to_owned(),
                        130,
                    ),
                ],
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(leftover["blood_pressure"]["count"], 1);
            assert!(leftover["facts"].as_array().unwrap().is_empty());
            let mut rows = Vec::new();
            for hour in 8..15 {
                let start = format!("2026-01-01T{hour:02}:00:00+00:00");
                rows.push(bp(
                    "HKQuantityTypeIdentifierBloodPressureSystolic",
                    start.clone(),
                    110 + hour,
                ));
                rows.push(bp(
                    "HKQuantityTypeIdentifierBloodPressureDiastolic",
                    start,
                    70 + hour,
                ));
            }
            let ranges = heart_analysis(&rows, &BTreeMap::new()).unwrap()["blood_pressure"].clone();
            assert_eq!(ranges["mode"], "range");
            assert!(ranges["readings"].as_array().unwrap().is_empty());
            assert_eq!(
                ranges["range_label"],
                "systolic 118–124 mmHg · diastolic 78–84 mmHg"
            );
            let mut identical_systolic = Vec::new();
            for hour in 8..15 {
                let start = format!("2026-01-01T{hour:02}:00:00+00:00");
                identical_systolic.push(bp(
                    "HKQuantityTypeIdentifierBloodPressureSystolic",
                    start.clone(),
                    120,
                ));
                identical_systolic.push(bp(
                    "HKQuantityTypeIdentifierBloodPressureDiastolic",
                    start,
                    70 + hour,
                ));
            }
            let identical =
                heart_analysis(&identical_systolic, &BTreeMap::new()).unwrap()["blood_pressure"]
                    .clone();
            assert_eq!(
                identical["range_label"],
                "systolic 120–120 mmHg · diastolic 78–84 mmHg"
            );
        }

        #[test]
        fn rhythm_events_and_burdens_preserve_empty_and_valued_cases() {
            let mut event = simple_row(
                "HKCategoryTypeIdentifierIrregularHeartRhythmEvent",
                "20260101",
                "2026-01-01T08:00:00+00:00",
                None,
            );
            event.source_name = FieldState::Present(json!("Watch"));
            let rhythm = heart_analysis(&[event], &BTreeMap::new()).unwrap()["rhythm"].clone();
            assert_eq!(rhythm["events"][0]["detail"], "1 event · reported by Watch");
            let mut plain_hr = simple_row(
                HEART_RATE,
                "20260101",
                "2026-01-01T08:00:00+00:00",
                Some(json!(60)),
            );
            plain_hr.unit = FieldState::Present(json!("count/min"));
            assert!(heart_analysis(&[plain_hr], &BTreeMap::new()).unwrap()["rhythm"].is_null());
            let burden = |record_type: &str, time: &str, value: Option<Value>, unit_name: &str| {
                let mut row = simple_row(record_type, "20260101", time, value);
                row.unit = FieldState::Present(json!(unit_name));
                row.source_name = FieldState::Present(json!("Watch"));
                row
            };
            let valued = heart_analysis(
                &[
                    burden(
                        "HKQuantityTypeIdentifierAtrialFibrillationBurden",
                        "2026-01-01T08:00:00+00:00",
                        Some(json!(1)),
                        "count/min",
                    ),
                    burden(
                        "oura.AtrialFibrillationBurdenHeartRateEstimate",
                        "2026-01-01T09:00:00+00:00",
                        Some(json!(2)),
                        "count/min",
                    ),
                ],
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(valued["rhythm"]["burden"]["label"], "AFib burden");
            assert_eq!(
                valued["rhythm"]["burden"]["detail"],
                "latest 2 count/min · 2 entries · reported by Watch"
            );
            let empty = heart_analysis(
                &[
                    burden(
                        "HKQuantityTypeIdentifierAtrialFibrillationBurden",
                        "2026-01-01T08:00:00+00:00",
                        Some(json!("unknown")),
                        "%",
                    ),
                    burden(
                        "HKQuantityTypeIdentifierAtrialFibrillationBurden",
                        "2026-01-01T09:00:00+00:00",
                        None,
                        "%",
                    ),
                ],
                &BTreeMap::new(),
            )
            .unwrap();
            assert!(empty["rhythm"]["burden"]["value"].is_null());
            assert_eq!(
                empty["rhythm"]["burden"]["detail"],
                "2 entries · reported by Watch"
            );
        }

        #[test]
        fn raw_heart_comparison_requires_ring_and_cross_device() {
            let sample = |source: &str, value: i64| {
                let mut row = simple_row(
                    HEART_RATE,
                    "20260101",
                    "2026-01-01T08:00:00+00:00",
                    Some(json!(value)),
                );
                row.source_name = FieldState::Present(json!(source));
                row
            };
            let both = heart_analysis(
                &[sample("Watch", 60), sample("Oura Ring", 51)],
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(both["comparison_line"], "Watch 60 bpm · Oura Ring 51 bpm");
            let one = heart_analysis(&[sample("Oura Ring", 51)], &BTreeMap::new()).unwrap();
            assert!(one["comparison_line"].is_null());
        }

        #[test]
        fn heart_series_segments_by_bucket_center_and_refuses_mixed_units() {
            let sample = |minute: usize, value: i64, unit_name: &str| {
                let mut row = simple_row(
                    HEART_RATE,
                    "20260101",
                    &format!("2026-01-01T{:02}:{:02}:00+00:00", minute / 60, minute % 60),
                    Some(json!(value)),
                );
                row.unit = FieldState::Present(json!(unit_name));
                row
            };
            let mut clustered = (0..6)
                .map(|index| sample(360 + index, 60 + index as i64, "count/min"))
                .collect::<Vec<_>>();
            clustered
                .extend((0..6).map(|index| sample(480 + index, 70 + index as i64, "count/min")));
            let series = heart_analysis(&clustered, &BTreeMap::new()).unwrap()["series"].clone();
            assert_eq!(series["svg"]["paths"].as_array().unwrap().len(), 2);
            assert_eq!(series["svg"]["band_paths"].as_array().unwrap().len(), 2);
            assert!(series["svg"]["dots"].as_array().unwrap().is_empty());
            let mut dot_rows = (0..11)
                .map(|index| sample(360 + index, 60 + index as i64, "count/min"))
                .collect::<Vec<_>>();
            dot_rows.push(sample(600, 80, "count/min"));
            let dots =
                heart_analysis(&dot_rows, &BTreeMap::new()).unwrap()["series"]["svg"]["dots"]
                    .clone();
            assert_eq!(dots.as_array().unwrap().len(), 1);
            let mixed = (0..12)
                .map(|index| {
                    sample(
                        360 + index,
                        60,
                        if index == 0 { "mph" } else { "count/min" },
                    )
                })
                .collect::<Vec<_>>();
            assert!(heart_analysis(&mixed, &BTreeMap::new()).unwrap()["series"].is_null());
        }
        #[test]
        fn cardiovascular_age_is_in_card_and_audit_appendix() {
            let mut row = simple_row(
                OURA_CARDIOVASCULAR_AGE,
                "20260101",
                "2026-01-01T04:00:00+00:00",
                Some(json!(42)),
            );
            row.source_family = FieldState::Present(json!(OURA_API));
            row.metadata = FieldState::Present(json!({"pulse_wave_velocity":8.2}));
            assert_eq!(
                heart_analysis(&[row.clone()], &BTreeMap::new()).unwrap()["facts"][0]["label"],
                "Vascular age"
            );
            assert_eq!(oura_appendix(&[row]).len(), 1);
        }

        #[test]
        fn cardiovascular_age_without_velocity_has_no_audit_appendix() {
            let mut row = simple_row(
                OURA_CARDIOVASCULAR_AGE,
                "20260101",
                "2026-01-01T04:00:00+00:00",
                Some(json!(42)),
            );
            row.source_family = FieldState::Present(json!(OURA_API));
            assert!(oura_appendix(&[row]).is_empty());
        }

        #[test]
        fn seeded_audit_appendix_requires_a_parseable_pulse_wave_velocity() {
            let root = TempDir::new();
            let row = oura_row(
                OURA_CARDIOVASCULAR_AGE,
                "20260101",
                None,
                None,
                Some(json!(42)),
                None,
                "daily_summary",
                Some(json!({"pulse_wave_velocity":8.2})),
                None,
            );
            seed_fixture(
                root.path(),
                vec![bundle("20260102_090000", OURA_API, vec![row])],
            );
            let mut reader = MonthReader::new(root.path());
            let payload = build_day(
                root.path(),
                parse_day("20260101").unwrap(),
                None,
                &mut reader,
            )
            .unwrap();
            assert_eq!(
                payload["audit"]["oura_appendix"],
                json!([{"label":"Pulse-wave velocity","detail":"8.2 m/s · Oura's measurement"}])
            );
            let mut unparseable = simple_row(
                OURA_CARDIOVASCULAR_AGE,
                "20260101",
                "2026-01-01T04:00:00+00:00",
                Some(json!(42)),
            );
            unparseable.metadata = FieldState::Present(json!({"pulse_wave_velocity":"unknown"}));
            assert!(oura_appendix(&[unparseable]).is_empty());
        }
        #[test]
        fn glucose_stays_partitioned_by_unit() {
            let mut a = simple_row(
                "HKQuantityTypeIdentifierBloodGlucose",
                "20260101",
                "2026-01-01T08:00:00+00:00",
                Some(json!(90)),
            );
            a.unit = FieldState::Present(json!("mg/dL"));
            let mut b = a.clone();
            b.unit = FieldState::Present(json!("mmol/L"));
            assert_eq!(glucose_series(&[a, b]).len(), 2);
        }
    }

    mod availability_and_failures {
        use super::*;
        use axum::body::{Body, to_bytes};
        use axum::http::Request;
        use tower::ServiceExt;
        async fn get(root: &Path, path: &str) -> (StatusCode, Value) {
            let response = crate::api_router(root)
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            (status, serde_json::from_slice(&bytes).unwrap())
        }
        #[tokio::test]
        async fn malformed_shard_is_a_store_unavailable_response() {
            let root = TempDir::new();
            seed_populated_body_journal(root.path());
            fs::write(
                root.path()
                    .join("imports/20260810_080000/normalized/2026-08.jsonl"),
                "not json\n",
            )
            .unwrap();
            let (status, body) = get(root.path(), "/app/body/api/day/20260801").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["reason_code"], "body_store_shard_unreadable");
        }
        #[tokio::test]
        async fn missing_and_unreadable_aggregates_have_specific_reasons() {
            let root = TempDir::new();
            seed_populated_body_journal(root.path());
            let db = root.path().join("imports/health-dedupe.sqlite");
            fs::remove_file(&db).unwrap();
            let (status, missing) = get(root.path(), "/app/body/api/day/20260801").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(missing["reason_code"], "body_store_aggregate_missing");
            rusqlite::Connection::open(&db)
                .unwrap()
                .execute_batch("CREATE TABLE wrong_table (id INTEGER)")
                .unwrap();
            let (status, unreadable) = get(root.path(), "/app/body/api/day/20260801").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(unreadable["reason_code"], "body_store_aggregate_unreadable");
        }
        #[tokio::test]
        async fn absent_summary_is_an_empty_string() {
            let root = TempDir::new();
            seed_populated_body_journal(root.path());
            fs::remove_dir_all(root.path().join("chronicle")).unwrap();
            let (_, body) = get(root.path(), "/app/body/api/day/20260801").await;
            assert_eq!(body["summary_markdown"], "");
        }
        #[tokio::test]
        async fn unreadable_summary_is_an_internal_error() {
            let root = TempDir::new();
            seed_populated_body_journal(root.path());
            let summary = root.path().join(
                "chronicle/20260801/import.apple_health/000000_300/day_summary_transcript.md",
            );
            fs::remove_file(&summary).unwrap();
            fs::create_dir(&summary).unwrap();
            let (status, body) = get(root.path(), "/app/body/api/day/20260801").await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body["reason_code"], "internal_error");
        }

        #[tokio::test]
        async fn empty_day_keeps_nearest_navigation() {
            let root = TempDir::new();
            let before = health_row(
                "HKQuantityTypeIdentifierBodyMass",
                "2026-01-01T08:00:00+00:00".to_owned(),
                Some(json!(170)),
                Some("lb"),
                "Scale",
                None,
                None,
                None,
            );
            let after = health_row(
                "HKQuantityTypeIdentifierBodyMass",
                "2026-01-03T08:00:00+00:00".to_owned(),
                Some(json!(171)),
                Some("lb"),
                "Scale",
                None,
                None,
                None,
            );
            seed_fixture(
                root.path(),
                vec![bundle("20260104_080000", APPLE, vec![before, after])],
            );
            let (status, body) = get(root.path(), "/app/body/api/day/20260102").await;
            assert_eq!(status, StatusCode::OK);
            assert!(!body["has_data"].as_bool().unwrap());
            assert_eq!(body["entry_total"], 0);
            assert!(body["sleep"].is_null());
            assert!(body["glucose"].is_null());
            assert!(body["glucose_series"].as_array().unwrap().is_empty());
            assert!(body["activity"].is_null());
            assert!(body["heart"].is_null());
            assert!(body["recovery"].is_null());
            assert!(body["sources"].is_null());
            assert!(body["mind_sound"].is_null());
            assert!(body["walking"].is_null());
            assert!(body["body_measurements"].is_null());
            assert!(body["other_signals"].is_null());
            assert!(body["prompts"].as_array().unwrap().is_empty());
            assert_eq!(body["nearest"]["prev"], "20260101");
            assert_eq!(body["nearest"]["next"], "20260103");
        }
    }

    mod corpus_replay {
        use super::*;
        use crate::corpus_test::{assert_recorded_payload, recorded};

        fn baseline_fields_are(payload: &Value, present: bool) {
            for (container, key) in [
                (&payload["heart"]["facts"][0], "typical"),
                (&payload["heart"]["facts"][0], "typical_label"),
                (&payload["recovery"]["facts"][0], "typical"),
                (&payload["recovery"]["facts"][0], "typical_label"),
                (&payload["sleep"], "asleep_typical"),
                (&payload["sleep"], "asleep_typical_label"),
                (&payload["sleep"], "score_typical"),
                (&payload["sleep"], "score_typical_label"),
            ] {
                assert_eq!(
                    container.as_object().unwrap().contains_key(key),
                    present,
                    "{key} presence"
                );
            }
        }

        #[test]
        fn first_run_day_payload_matches_recorded_corpus() {
            let root = TempDir::new();
            seed_populated_body_journal(root.path());
            let stats = read_health_dedupe_stats(root.path()).expect("aggregate stats");
            let mut reader = MonthReader::new(root.path());
            let actual = build_day(
                root.path(),
                parse_day("20260801").expect("valid anchor"),
                stats.as_deref(),
                &mut reader,
            )
            .expect("day builds");
            baseline_fields_are(&actual, false);
            assert_recorded_payload("first_run", "/app/body/api/day/<day>", root.path(), &actual);
        }

        #[test]
        fn fixed_day_payload_matches_recorded_warmed_baselines() {
            let root = TempDir::new();
            seed_populated_body_journal(root.path());
            let trends = recorded("fixed", "/app/body/api/trends");
            let signals = trends["signals"]
                .as_array()
                .expect("signals")
                .iter()
                .map(|signal| TrendSignal {
                    key: signal["key"].as_str().expect("key").to_owned(),
                    label: signal["label"].as_str().expect("label").to_owned(),
                    unit_label: signal["unit_label"].as_str().expect("unit").to_owned(),
                    daily: signal["daily"]
                        .as_array()
                        .expect("daily")
                        .iter()
                        .map(|pair| {
                            (
                                pair[0].as_str().expect("day").to_owned(),
                                if let Some(value) = pair[1].as_i64() {
                                    TrendValue::Integer(value)
                                } else {
                                    TrendValue::Real(pair[1].as_f64().expect("value"))
                                },
                            )
                        })
                        .collect(),
                    coverage: TrendCoverage {
                        first_day: signal["coverage"]["first_day"]
                            .as_str()
                            .expect("first")
                            .to_owned(),
                        last_day: signal["coverage"]["last_day"]
                            .as_str()
                            .expect("last")
                            .to_owned(),
                        days: signal["coverage"]["days"].as_u64().expect("days") as usize,
                    },
                })
                .collect();
            let annotations = trends["annotations"]
                .as_array()
                .expect("annotations")
                .iter()
                .map(|item| TrendAnnotation {
                    day: item["day"].as_str().expect("day").to_owned(),
                    label: item["label"].as_str().expect("label").to_owned(),
                })
                .collect();
            replace_trends_cache(
                trends_db_path(root.path()),
                trends_signature(root.path()).expect("signature"),
                TrendsPayload {
                    signals,
                    annotations,
                    generated_at_day: trends["generated_at_day"]
                        .as_str()
                        .expect("generated")
                        .to_owned(),
                },
            )
            .expect("cache warms");
            let stats = read_health_dedupe_stats(root.path()).expect("stats");
            let mut reader = MonthReader::new(root.path());
            let actual = build_day(
                root.path(),
                parse_day("20260801").expect("day"),
                stats.as_deref(),
                &mut reader,
            )
            .expect("day builds");
            baseline_fields_are(&actual, true);
            assert_recorded_payload("fixed", "/app/body/api/day/<day>", root.path(), &actual);
        }

        #[test]
        fn seeded_journal_metrics_match_corpus() {
            let root = TempDir::new();
            let report = seed_populated_body_journal(root.path());
            let corpus: Value =
                serde_json::from_str(include_str!("../../../fixtures/convey_body_corpus.json"))
                    .expect("corpus");
            let journal = &corpus["journal"];
            let inventory = crate::read_body_import_inventory(root.path()).expect("inventory");
            let entries = inventory
                .entries
                .iter()
                .map(|entry| {
                    (
                        entry.import_id.clone(),
                        match entry.entry_count {
                            crate::ManifestEntryCount::Present(value) => value,
                            _ => 0,
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let valid_import_ids = entries.keys().cloned().collect::<BTreeSet<_>>();
            let expected_valid_import_ids = journal["valid_import_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>();
            assert_eq!(valid_import_ids, expected_valid_import_ids);
            let excluded_import_ids = inventory
                .skipped
                .iter()
                .map(|skip| match skip {
                    crate::BodyImportSkip::UnreadableManifest { path, .. }
                    | crate::BodyImportSkip::UnknownSourceFamily { path, .. } => path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .unwrap()
                        .to_owned(),
                })
                .collect::<BTreeSet<_>>();
            let expected_excluded_import_ids = journal["excluded_import_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>();
            assert_eq!(excluded_import_ids, expected_excluded_import_ids);
            assert_eq!(
                report.dates,
                BTreeSet::from([journal["anchor_day"].as_str().unwrap().to_owned()])
            );
            let expected_raw_rows = journal["raw_row_counts_by_start_date"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(day, count)| (day.clone(), count.as_u64().unwrap()))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(report.rows_by_start_date_day, expected_raw_rows);
            assert_eq!(entries.get("20260810_080000"), Some(&206));
            assert_eq!(entries.get("20260810_090000"), Some(&365));
            assert_eq!(entries.get("20260810_100000"), Some(&1));
            let raw = entries.values().sum::<u64>();
            assert_eq!(
                raw,
                journal["raw_normalized_total"].as_u64().expect("raw total")
            );
            let stats = read_health_dedupe_stats(root.path())
                .expect("stats")
                .expect("aggregate");
            assert_eq!(
                stats.by_day.get("20260801"),
                Some(
                    &journal["dedupe_day_counts_by_start_date"]["20260801"]
                        .as_u64()
                        .expect("anchor count")
                )
            );
            assert_eq!(
                stats.by_day.values().sum::<u64>(),
                journal["sqlite_normalized_total"]
                    .as_u64()
                    .expect("dedupe total")
            );
            for expected in journal["import_bundles"].as_array().unwrap() {
                let import_id = expected["import_id"].as_str().unwrap();
                let entry = inventory
                    .entries
                    .iter()
                    .find(|entry| entry.import_id == import_id)
                    .unwrap();
                let expected_months = expected["months"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                assert_eq!(entry.normalized_months, expected_months);
                let expected_days = expected["days_affected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .collect::<BTreeSet<_>>();
                assert_eq!(bundle_days(root.path(), import_id), expected_days);
            }
        }

        fn bundle_days(root: &Path, import_id: &str) -> BTreeSet<String> {
            let mut paths = fs::read_dir(root.join("imports").join(import_id).join("normalized"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            paths.sort();
            paths
                .into_iter()
                .flat_map(|path| crate::read_normalized_shard(path).unwrap())
                .filter_map(|row| string_field(&row.day).map(str::to_owned))
                .collect()
        }
    }
}
