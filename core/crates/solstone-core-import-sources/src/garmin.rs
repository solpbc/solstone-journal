// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Normalizing Garmin Connect Personal API JSON payloads into body records.
//!
//! All Garmin Personal API JSON categories are stored whole inside the owner's
//! journal (`import.garmin`). Agent egress policies (Women's Health subset
//! gating and `places`-precision workout coordinate gating) run at the MCP
//! boundary, never at ingest.

use std::collections::BTreeMap;
use std::path::Path;

use jiff::civil::{Date, DateTime};
use jiff::tz::{Offset, TimeZone};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Map, Value, json};
use solstone_core_format::content::body_records::{
    BodyRow, RowKind, encode_rows, parse_source_time, plain_words,
};

use crate::apple_health::{
    DayRows, MAX_EXPERIENCE_SECONDS, MAX_ROW_BYTES, is_session, resolve_key, row_window,
    sha256_hex, tile_experience,
};
use crate::body_stream::GARMIN;
use crate::shared::epoch_millisecond;

/// Default attribution when Garmin returns no device model.
pub(crate) const BY: &str = "Garmin";

/// Canonical Garmin endpoint keys stored in staged `pages/index.json` and
/// coverage ledgers.
pub const ENDPOINTS: [&str; 14] = [
    "activities",
    "health_snapshot",
    "sleep",
    "daily_summary",
    "daily_heart_rate",
    "hrv",
    "spo2",
    "weight",
    "hydration",
    "vo2_max",
    "fitness_age",
    "body_battery",
    "menstrual_cycle_summary",
    "menstrual_cycle_dayview",
];

/// Nutrition is fetched alongside the endpoints above (separate constant kept
/// in sync with `ALL_ENDPOINTS` so callers can iterate every endpoint).
pub const ALL_ENDPOINTS: [&str; 15] = [
    "activities",
    "health_snapshot",
    "sleep",
    "daily_summary",
    "daily_heart_rate",
    "hrv",
    "spo2",
    "weight",
    "hydration",
    "vo2_max",
    "fitness_age",
    "body_battery",
    "menstrual_cycle_summary",
    "menstrual_cycle_dayview",
    "nutrition",
];

/// One normalized Garmin row ready to be staged into the shared body-segments placer.
#[derive(Debug, Clone)]
pub struct ReadRow {
    pub identity: String,
    pub row: BodyRow,
    /// Calendar date stated by Garmin (`YYYY-MM-DD`), when present.
    pub garmin_day: Option<Date>,
}

/// Deterministic Garmin identity string.
#[must_use]
pub fn identity(endpoint: &str, key: &str) -> String {
    format!("{}\u{1f}{endpoint}\u{1f}{key}", GARMIN.source)
}

/// Deterministic `BodyRow.id` matching the shared body placer's SHA-256 scheme.
#[must_use]
pub fn row_id(identity: &str) -> String {
    sha256_hex(&format!("{identity}\u{1f}0"))
}

fn new_row(kind: RowKind, identity: String, record_type: &str, label: String) -> BodyRow {
    let mut row = BodyRow::new(kind, row_id(&identity), GARMIN.source, record_type, label);
    row.by = Some(BY.to_owned());
    row
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn i64_field(obj: &Map<String, Value>, key: &str) -> Option<i64> {
    obj.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|u| i64::try_from(u).ok()))
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f.round() as i64)
            })
    })
}

fn f64_field(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    obj.get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn parse_date(text: &str) -> Option<Date> {
    let prefix = text.trim().get(..10)?;
    Date::strptime("%Y-%m-%d", prefix).ok()
}

fn parse_civil_datetime(text: &str) -> Option<DateTime> {
    let trimmed = text.trim().trim_end_matches('Z').trim_end_matches('z');
    let normalized = trimmed.replace(' ', "T");
    let no_frac = normalized.split('.').next().unwrap_or(&normalized);
    DateTime::strptime("%Y-%m-%dT%H:%M:%S", no_frac).ok()
}

fn format_rfc3339(local: DateTime, offset_secs: i32) -> Option<String> {
    if !(-18 * 3_600..=18 * 3_600).contains(&offset_secs) {
        return None;
    }
    let base = local.strftime("%Y-%m-%dT%H:%M:%S").to_string();
    if offset_secs == 0 {
        return Some(format!("{base}Z"));
    }
    let sign = if offset_secs >= 0 { '+' } else { '-' };
    let abs = offset_secs.unsigned_abs();
    let hours = abs / 3_600;
    let minutes = (abs % 3_600) / 60;
    Some(format!("{base}{sign}{hours:02}:{minutes:02}"))
}

/// Reconstruct an exact RFC 3339 timestamp with UTC offset from Garmin's paired
/// `*Local` and `*GMT` strings (e.g., `"2026-11-24 07:15:00"` and `"2026-11-24 14:15:00"`).
#[must_use]
pub fn rfc3339_from_local_and_gmt(local: Option<&str>, gmt: Option<&str>) -> Option<String> {
    if let Some(local_str) = local
        && parse_source_time(local_str).is_some()
        && (local_str.ends_with('Z')
            || local_str.len() >= 25 && matches!(local_str.as_bytes().get(19), Some(b'+' | b'-')))
    {
        return Some(local_str.trim().to_owned());
    }
    match (
        local.and_then(parse_civil_datetime),
        gmt.and_then(parse_civil_datetime),
    ) {
        (Some(local_dt), Some(gmt_dt)) => {
            let local_ts = Offset::UTC.to_timestamp(local_dt).ok()?;
            let gmt_ts = Offset::UTC.to_timestamp(gmt_dt).ok()?;
            let raw_offset = local_ts.as_second() - gmt_ts.as_second();
            // Round to nearest minute to absorb sub-second skew between fields.
            let rounded = ((raw_offset + if raw_offset >= 0 { 30 } else { -30 }) / 60) * 60;
            let offset_secs = i32::try_from(rounded).ok()?;
            format_rfc3339(local_dt, offset_secs)
        }
        (None, Some(gmt_dt)) => format_rfc3339(gmt_dt, 0),
        (Some(local_dt), None) => format_rfc3339(local_dt, 0),
        (None, None) => None,
    }
}

/// Reconstruct an exact RFC 3339 timestamp from Garmin's paired epoch-millisecond
/// GMT and local fields (where Garmin encodes local wall time as fake-UTC epoch ms).
#[must_use]
pub fn rfc3339_from_epoch_ms(gmt_ms: i64, local_ms: Option<i64>) -> Option<String> {
    let gmt_secs = gmt_ms.div_euclid(1_000);
    let offset_secs = match local_ms {
        Some(local_ms) => {
            let diff_secs = local_ms.div_euclid(1_000) - gmt_secs;
            let rounded = ((diff_secs + if diff_secs >= 0 { 30 } else { -30 }) / 60) * 60;
            i32::try_from(rounded)
                .ok()
                .filter(|o| (-18 * 3_600..=18 * 3_600).contains(o))?
        }
        None => 0,
    };
    rfc3339_from_gmt_ms_with_offset(gmt_ms, offset_secs)
}

/// Format a GMT epoch-millisecond instant at a known fixed UTC offset.
#[must_use]
pub fn rfc3339_from_gmt_ms_with_offset(gmt_ms: i64, offset_secs: i32) -> Option<String> {
    let offset = Offset::from_seconds(offset_secs).ok()?;
    let ts = Timestamp::from_millisecond(gmt_ms).ok()?;
    let local = offset.to_datetime(ts);
    format_rfc3339(local, offset_secs)
}

fn add_seconds_rfc3339(start_rfc3339: &str, duration_secs: f64) -> Option<String> {
    if !duration_secs.is_finite() || duration_secs < 0.0 {
        return None;
    }
    let whole = duration_secs.round() as i64;
    if whole > MAX_EXPERIENCE_SECONDS {
        return None;
    }
    let start = parse_source_time(start_rfc3339)?;
    let offset_secs = start.offset().seconds();
    let end_ts = start
        .timestamp()
        .checked_add(SignedDuration::from_secs(whole))
        .ok()?;
    let offset = Offset::from_seconds(offset_secs).ok()?;
    format_rfc3339(offset.to_datetime(end_ts), offset_secs)
}

fn object_fields(obj: &Map<String, Value>) -> BTreeMap<String, Value> {
    obj.iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Ensure a row serializes within `MAX_ROW_BYTES` (`32 KiB`). If an unexpected
/// nested array/blob pushes a row past the cap, trim oversized non-essential
/// sub-arrays while preserving all scalar metrics and location fields.
fn ensure_row_within_byte_limit(row: &mut BodyRow) {
    if encode_rows(std::slice::from_ref(row)).len() <= MAX_ROW_BYTES {
        return;
    }
    // Drop the largest array/object fields first until under the cap.
    while encode_rows(std::slice::from_ref(row)).len() > MAX_ROW_BYTES {
        let largest = row
            .fields
            .iter()
            .filter(|(_, value)| value.is_array() || value.is_object())
            .max_by_key(|(_, value)| value.to_string().len())
            .map(|(key, _)| key.clone());
        match largest {
            Some(key) => {
                row.fields.remove(&key);
            }
            None => break,
        }
    }
}

/// Parse one staged Garmin payload for `endpoint` into zero or more [`ReadRow`]s.
/// `context_day` is the date requested for single-day endpoints (`YYYY-MM-DD`).
#[must_use]
pub fn read_page(endpoint: &str, context_day: Option<&str>, payload: &Value) -> Vec<ReadRow> {
    match endpoint {
        "activities" => read_activities(payload),
        "health_snapshot" => read_health_snapshots(payload),
        "sleep" => read_sleep(context_day, payload),
        "daily_summary" => read_daily_summaries(payload),
        "daily_heart_rate" => read_daily_heart_rate(context_day, payload),
        "hrv" => read_hrv(context_day, payload),
        "spo2" => read_spo2(context_day, payload),
        "weight" => read_weight(payload),
        "hydration" => read_hydration(payload),
        "vo2_max" => read_vo2_max(payload),
        "fitness_age" => read_fitness_age(context_day, payload),
        "body_battery" => read_body_battery(payload),
        "menstrual_cycle_summary" => read_menstrual_cycle(
            "menstrual_cycle_summary",
            "menstrual cycle summary",
            context_day,
            payload,
        ),
        "menstrual_cycle_dayview" => read_menstrual_cycle(
            "menstrual_cycle_dayview",
            "menstrual cycle day view",
            context_day,
            payload,
        ),
        "nutrition" => read_nutrition(context_day, payload),
        _ => Vec::new(),
    }
}

fn read_activities(payload: &Value) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => {
            if let Some(list) = obj.get("activities").and_then(Value::as_array) {
                list.iter().filter_map(Value::as_object).collect()
            } else if obj.contains_key("activityId") {
                vec![obj]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(activity_id) = i64_field(item, "activityId") else {
            continue;
        };
        // Both search/activities (`startTimeLocal`, `startTimeGMT`, `duration`, `activityType`)
        // and activity/{id} (`summaryDTO.*`, `activityTypeDTO`) are handled cleanly.
        let summary = item.get("summaryDTO").and_then(Value::as_object);
        let type_key = item
            .get("activityType")
            .or_else(|| item.get("activityTypeDTO"))
            .and_then(|v| v.get("typeKey"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("workout");
        let start = rfc3339_from_local_and_gmt(
            str_field(item, "startTimeLocal")
                .or_else(|| summary.and_then(|s| str_field(s, "startTimeLocal"))),
            str_field(item, "startTimeGMT")
                .or_else(|| summary.and_then(|s| str_field(s, "startTimeGMT"))),
        )
        .or_else(|| {
            i64_field(item, "beginTimestamp").and_then(|ms| rfc3339_from_epoch_ms(ms, None))
        });
        let Some(start) = start else {
            continue;
        };
        let duration = f64_field(item, "duration")
            .or_else(|| summary.and_then(|s| f64_field(s, "duration")))
            .or_else(|| f64_field(item, "elapsedDuration"))
            .or_else(|| summary.and_then(|s| f64_field(s, "elapsedDuration")))
            .unwrap_or(0.0);
        let Some(end) = add_seconds_rfc3339(&start, duration) else {
            continue;
        };
        let id = identity("activity", &activity_id.to_string());
        let mut row = new_row(
            RowKind::Experience,
            id.clone(),
            type_key,
            plain_words(type_key),
        );
        row.experience = Some("workout".to_owned());
        row.start = Some(start.clone());
        row.end = Some(end);

        // Preserve Garmin workout titles (activityName) and descriptions searchable in row.words.
        for key in ["activityName", "description"] {
            if let Some(text) = str_field(item, key)
                && !row.words.iter().any(|existing| existing == text)
            {
                row.words.push(text.to_owned());
            }
        }
        // Merge summaryDTO scalars onto top-level fields if present from activity/{id} detail.
        let mut fields = object_fields(item);
        if let Some(summary) = summary {
            for (k, v) in summary {
                if !v.is_null() {
                    fields.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        if let Some(dist) = fields.get("distance").and_then(Value::as_f64)
            && dist > 0.0
        {
            row.values
                .insert("distance".to_owned(), format!("{:.0} m", dist));
        }
        if duration > 0.0 {
            row.values
                .insert("duration".to_owned(), format!("{:.0} s", duration));
        }
        if let Some(hr) = fields.get("averageHR").and_then(Value::as_f64)
            && hr > 0.0
        {
            row.values
                .insert("average heart rate".to_owned(), format!("{:.0} bpm", hr));
        }
        if let Some(cal) = fields.get("calories").and_then(Value::as_f64)
            && cal > 0.0
        {
            row.values
                .insert("calories".to_owned(), format!("{:.0} kcal", cal));
        }
        row.fields = fields;
        ensure_row_within_byte_limit(&mut row);
        let garmin_day = parse_date(&start);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day,
        });
    }
    rows
}

fn read_health_snapshots(payload: &Value) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => {
            if let Some(list) = obj
                .get("healthSnapshotSummaries")
                .or_else(|| obj.get("summaries"))
                .and_then(Value::as_array)
            {
                list.iter().filter_map(Value::as_object).collect()
            } else if obj.contains_key("startTime") || obj.contains_key("summaryId") {
                vec![obj]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let start = str_field(item, "startTime")
            .and_then(|s| rfc3339_from_local_and_gmt(Some(s), str_field(item, "startTimeGMT")))
            .or_else(|| {
                i64_field(item, "startTimeMillis").and_then(|ms| {
                    rfc3339_from_epoch_ms(ms, i64_field(item, "startTimeLocalMillis"))
                })
            });
        let Some(start) = start else {
            continue;
        };
        let end = str_field(item, "endTime")
            .and_then(|s| rfc3339_from_local_and_gmt(Some(s), str_field(item, "endTimeGMT")))
            .or_else(|| {
                f64_field(item, "durationSeconds")
                    .or_else(|| f64_field(item, "duration"))
                    .and_then(|d| add_seconds_rfc3339(&start, d))
            })
            .unwrap_or_else(|| start.clone());
        let key = str_field(item, "summaryId")
            .or_else(|| str_field(item, "uuid"))
            .map(str::to_owned)
            .unwrap_or_else(|| start.clone());
        let id = identity("health_snapshot", &key);
        let mut row = new_row(
            RowKind::Experience,
            id.clone(),
            "health_snapshot",
            "health snapshot".to_owned(),
        );
        row.experience = Some("health snapshot".to_owned());
        row.start = Some(start.clone());
        row.end = Some(end);
        // Preserve Garmin device model attribution when present (the one endpoint that returns it).
        if let Some(device_name) = item
            .get("deviceMetaData")
            .and_then(|meta| meta.get("deviceName"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            row.by = Some(
                if device_name.starts_with("Garmin ") || device_name == "Garmin" {
                    device_name.to_owned()
                } else {
                    format!("Garmin {device_name}")
                },
            );
        }
        row.fields = object_fields(item);
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: parse_date(&start),
        });
    }
    rows
}

fn read_sleep(context_day: Option<&str>, payload: &Value) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => vec![obj],
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let dto = item
            .get("dailySleepDTO")
            .and_then(Value::as_object)
            .unwrap_or(item);
        let calendar_date = str_field(dto, "calendarDate")
            .or_else(|| str_field(item, "calendarDate"))
            .or(context_day);
        let has_sleep_id = dto.get("id").is_some_and(|v| !v.is_null());
        let start_gmt = i64_field(dto, "sleepStartTimestampGMT");
        let end_gmt = i64_field(dto, "sleepEndTimestampGMT");
        let start_local = i64_field(dto, "sleepStartTimestampLocal");
        let end_local = i64_field(dto, "sleepEndTimestampLocal");

        if has_sleep_id
            && let (Some(start_ms), Some(end_ms)) = (start_gmt, end_gmt)
            && end_ms >= start_ms
            && let (Some(start), Some(end)) = (
                rfc3339_from_epoch_ms(start_ms, start_local),
                rfc3339_from_epoch_ms(end_ms, end_local),
            )
            && (end_ms - start_ms) / 1_000 <= MAX_EXPERIENCE_SECONDS
        {
            let sleep_key = dto
                .get("id")
                .and_then(|v| {
                    v.as_i64()
                        .map(|n| n.to_string())
                        .or_else(|| v.as_str().map(str::to_owned))
                })
                .or_else(|| calendar_date.map(str::to_owned))
                .unwrap_or_else(|| start.clone());
            let id = identity("sleep", &sleep_key);
            let sleep_row_id = row_id(&id);
            let mut row = new_row(RowKind::Experience, id.clone(), "sleep", "sleep".to_owned());
            row.experience = Some("sleep".to_owned());
            row.start = Some(start.clone());
            row.end = Some(end);

            let mut fields = object_fields(dto);
            // Separate proprietary sleep scores onto their own Dated row so raw
            // sleep timing and stages remain unmixed.
            let scores = fields.remove("sleepScores");
            let feedback = fields.remove("sleepScoreFeedback");
            let insight = fields.remove("sleepScoreInsight");
            fields.remove("dailyNapDTOS");
            if let Some(sleep_secs) = fields.get("sleepTimeSeconds").and_then(Value::as_u64) {
                row.values
                    .insert("asleep".to_owned(), format!("{sleep_secs} s"));
            }
            row.fields = fields;
            ensure_row_within_byte_limit(&mut row);
            rows.push(ReadRow {
                identity: id,
                row,
                garmin_day: calendar_date
                    .and_then(parse_date)
                    .or_else(|| parse_date(&start)),
            });

            // Sleep score Dated row (if scores/feedback present).
            let has_sleep_score = scores.is_some() || feedback.is_some() || insight.is_some();
            if has_sleep_score && let Some(date_str) = calendar_date {
                let score_id = identity("sleep_scores", date_str);
                let mut score_row = new_row(
                    RowKind::Dated,
                    score_id.clone(),
                    "sleep_scores",
                    "sleep scores".to_owned(),
                );
                score_row.date = Some(date_str.to_owned());
                if let Some(scores) = scores {
                    score_row.fields.insert("sleepScores".to_owned(), scores);
                }
                if let Some(feedback) = feedback {
                    score_row
                        .fields
                        .insert("sleepScoreFeedback".to_owned(), feedback);
                }
                if let Some(insight) = insight {
                    score_row
                        .fields
                        .insert("sleepScoreInsight".to_owned(), insight);
                }
                ensure_row_within_byte_limit(&mut score_row);
                rows.push(ReadRow {
                    identity: score_id,
                    row: score_row,
                    garmin_day: parse_date(date_str),
                });
            }

            // Sleep stage rows if `sleepLevels` array is included in the payload.
            if let Some(levels) = item.get("sleepLevels").and_then(Value::as_array) {
                let offset_secs = match start_local {
                    Some(local_ms) => {
                        let diff = local_ms.div_euclid(1_000) - start_ms.div_euclid(1_000);
                        i32::try_from(((diff + if diff >= 0 { 30 } else { -30 }) / 60) * 60)
                            .unwrap_or(0)
                    }
                    None => 0,
                };
                for level in levels.iter().filter_map(Value::as_object) {
                    let level_start = str_field(level, "startGMT")
                        .and_then(parse_civil_datetime)
                        .and_then(|dt| Offset::UTC.to_timestamp(dt).ok())
                        .and_then(|ts| {
                            rfc3339_from_gmt_ms_with_offset(ts.as_millisecond(), offset_secs)
                        });
                    let level_end = str_field(level, "endGMT")
                        .and_then(parse_civil_datetime)
                        .and_then(|dt| Offset::UTC.to_timestamp(dt).ok())
                        .and_then(|ts| {
                            rfc3339_from_gmt_ms_with_offset(ts.as_millisecond(), offset_secs)
                        });
                    let Some((stage_start, stage_end)) = level_start.zip(level_end) else {
                        continue;
                    };
                    let activity_level = level
                        .get("activityLevel")
                        .and_then(Value::as_f64)
                        .map(|n| n.round() as i64)
                        .unwrap_or(-1);
                    let stage_label = match activity_level {
                        0 => "asleep deep",
                        1 => "asleep light",
                        2 => "asleep rem",
                        3 => "awake",
                        _ => continue,
                    };
                    let stage_id = identity("sleep_stage", &format!("{sleep_key}:{stage_start}"));
                    let mut stage_row = new_row(
                        RowKind::Stage,
                        stage_id.clone(),
                        "sleep_stage",
                        stage_label.to_owned(),
                    );
                    stage_row.of = Some(sleep_row_id.clone());
                    stage_row.start = Some(stage_start);
                    stage_row.end = Some(stage_end);
                    rows.push(ReadRow {
                        identity: stage_id,
                        row: stage_row,
                        garmin_day: calendar_date.and_then(parse_date),
                    });
                }
            }
        }

        // Daytime naps (`dailyNapDTOS`) can exist even on days with empty overnight sleep.
        let naps = dto
            .get("dailyNapDTOS")
            .or_else(|| item.get("dailyNapDTOS"))
            .and_then(Value::as_array);
        if let Some(naps) = naps {
            for (idx, nap) in naps.iter().filter_map(Value::as_object).enumerate() {
                let start = rfc3339_from_local_and_gmt(
                    str_field(nap, "napStartTimestampLocal"),
                    str_field(nap, "napStartTimestampGMT"),
                )
                .or_else(|| {
                    i64_field(nap, "napStartTimestampGMT").and_then(|ms| {
                        rfc3339_from_epoch_ms(ms, i64_field(nap, "napStartTimestampLocal"))
                    })
                });
                let end = rfc3339_from_local_and_gmt(
                    str_field(nap, "napEndTimestampLocal"),
                    str_field(nap, "napEndTimestampGMT"),
                )
                .or_else(|| {
                    i64_field(nap, "napEndTimestampGMT").and_then(|ms| {
                        rfc3339_from_epoch_ms(ms, i64_field(nap, "napEndTimestampLocal"))
                    })
                })
                .or_else(|| {
                    start.as_deref().and_then(|s| {
                        f64_field(nap, "napTimeSec").and_then(|d| add_seconds_rfc3339(s, d))
                    })
                });
                let Some((nap_start, nap_end)) = start.zip(end) else {
                    continue;
                };
                let nap_key = str_field(nap, "napId")
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!("{}:{nap_start}:{idx}", calendar_date.unwrap_or("nap"))
                    });
                let id = identity("nap", &nap_key);
                let mut row = new_row(RowKind::Experience, id.clone(), "nap", "nap".to_owned());
                row.experience = Some("nap".to_owned());
                row.start = Some(nap_start.clone());
                row.end = Some(nap_end);
                row.fields = object_fields(nap);
                ensure_row_within_byte_limit(&mut row);
                rows.push(ReadRow {
                    identity: id,
                    row,
                    garmin_day: calendar_date
                        .and_then(parse_date)
                        .or_else(|| parse_date(&nap_start)),
                });
            }
        }
    }
    rows
}

fn read_daily_summaries(payload: &Value) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => {
            if let Some(list) = obj
                .get("data")
                .or_else(|| obj.get("dailySummaries"))
                .or_else(|| obj.get("summaries"))
                .and_then(Value::as_array)
            {
                list.iter().filter_map(Value::as_object).collect()
            } else {
                vec![obj]
            }
        }
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(date_str) = str_field(item, "calendarDate") else {
            continue;
        };
        let mut fields = object_fields(item);
        // Strip Garmin's negative sentinel stress codes (-1 unmeasured, -2 off-wrist)
        // across both flat v1 keys and nested v2 `stress` objects.
        for sentinel_key in [
            "averageStressLevel",
            "maxStressLevel",
            "stressPercentage",
            "restStressPercentage",
            "activityStressPercentage",
            "uncategorizedStressPercentage",
            "lowStressPercentage",
            "mediumStressPercentage",
            "highStressPercentage",
        ] {
            if fields
                .get(sentinel_key)
                .and_then(Value::as_f64)
                .is_some_and(|v| v < 0.0)
            {
                fields.remove(sentinel_key);
            }
        }
        if let Some(Value::Object(stress)) = fields.get_mut("stress") {
            stress.retain(|_, v| !v.as_f64().is_some_and(|n| n < 0.0));
        }

        let v2_steps = fields
            .get("movement")
            .and_then(|m| m.get("steps"))
            .and_then(|s| s.get("value"))
            .and_then(Value::as_f64);
        let v2_active_kcal = fields
            .get("calories")
            .and_then(|c| c.get("burnedActive"))
            .and_then(Value::as_f64);
        let v2_total_kcal = fields
            .get("calories")
            .and_then(|c| c.get("burnedTotal"))
            .and_then(Value::as_f64);
        let v2_rhr = fields
            .get("heartRate")
            .and_then(Value::as_object)
            .and_then(|hr| {
                hr.get("resting")
                    .or_else(|| hr.get("restingValue"))
                    .or_else(|| hr.get("restingHeartRate"))
                    .and_then(Value::as_i64)
            });
        let v2_has_hr = fields
            .get("heartRate")
            .and_then(Value::as_object)
            .is_some_and(|hr| hr.values().any(|v| v.as_f64().is_some_and(|n| n > 0.0)));
        let v2_has_stress = fields
            .get("stress")
            .and_then(Value::as_object)
            .and_then(|s| s.get("avgLevel"))
            .and_then(Value::as_f64)
            .is_some_and(|n| n >= 0.0);
        let v2_has_bb = fields
            .get("bodyBattery")
            .and_then(Value::as_object)
            .is_some_and(|bb| bb.values().any(|v| v.as_f64().is_some_and(|n| n > 0.0)));

        // Skip totally empty summary shells with no measurements.
        let has_data = fields
            .get("totalSteps")
            .and_then(Value::as_f64)
            .is_some_and(|v| v > 0.0)
            || v2_steps.is_some_and(|v| v > 0.0)
            || fields
                .get("restingHeartRate")
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
            || fields
                .get("minHeartRate")
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
            || v2_has_hr
            || fields
                .get("totalKilocalories")
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
            || v2_total_kcal.is_some_and(|v| v > 0.0)
            || fields
                .get("bodyBatteryMostRecentValue")
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
            || v2_has_bb
            || fields.contains_key("averageStressLevel")
            || v2_has_stress;
        if !has_data {
            continue;
        }
        // Provide `steps` and `active_calories` aliases so `week_totals` summarizes
        // Garmin daily summaries cleanly across both flat and nested v2 shapes.
        if let Some(total_steps) = fields
            .get("totalSteps")
            .cloned()
            .or_else(|| v2_steps.map(|s| json!(s)))
        {
            fields.entry("steps".to_owned()).or_insert(total_steps);
        }
        if let Some(active_kcal) = fields
            .get("activeKilocalories")
            .cloned()
            .or_else(|| v2_active_kcal.filter(|v| *v > 0.0).map(|k| json!(k)))
        {
            fields
                .entry("active_calories".to_owned())
                .or_insert(active_kcal);
        }
        let id = identity("daily_summary", date_str);
        let mut row = new_row(
            RowKind::Dated,
            id.clone(),
            "daily_activity",
            "daily summary".to_owned(),
        );
        row.date = Some(date_str.to_owned());
        row.fields = fields;
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: parse_date(date_str),
        });

        // Separate resting HR dated row when present.
        if let Some(rhr) = i64_field(item, "restingHeartRate").or(v2_rhr)
            && rhr > 0
        {
            let rhr_id = identity("resting_heart_rate", date_str);
            let mut rhr_row = new_row(
                RowKind::Dated,
                rhr_id.clone(),
                "resting_heart_rate",
                "resting heart rate".to_owned(),
            );
            rhr_row.date = Some(date_str.to_owned());
            rhr_row.value = Some(rhr.to_string());
            rhr_row.unit = Some("bpm".to_owned());
            rows.push(ReadRow {
                identity: rhr_id,
                row: rhr_row,
                garmin_day: parse_date(date_str),
            });
        }
    }
    rows
}

fn read_daily_heart_rate(context_day: Option<&str>, payload: &Value) -> Vec<ReadRow> {
    let Some(obj) = payload.as_object() else {
        return Vec::new();
    };
    let date_str = str_field(obj, "calendarDate").or(context_day);
    let offset_secs = match (
        str_field(obj, "startTimestampLocal").and_then(parse_civil_datetime),
        str_field(obj, "startTimestampGMT").and_then(parse_civil_datetime),
    ) {
        (Some(local_dt), Some(gmt_dt)) => {
            let local_ts = Offset::UTC.to_timestamp(local_dt).ok();
            let gmt_ts = Offset::UTC.to_timestamp(gmt_dt).ok();
            match (local_ts, gmt_ts) {
                (Some(l), Some(g)) => i32::try_from(l.as_second() - g.as_second()).unwrap_or(0),
                _ => 0,
            }
        }
        _ => 0,
    };
    let mut rows = Vec::new();
    if let Some(samples) = obj.get("heartRateValues").and_then(Value::as_array) {
        for sample in samples {
            let Some(pair) = sample.as_array() else {
                continue;
            };
            let Some(ts_ms) = pair.first().and_then(Value::as_i64) else {
                continue;
            };
            let Some(bpm) = pair
                .get(1)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v > 0.0)
            else {
                continue;
            };
            let Some(at) = rfc3339_from_gmt_ms_with_offset(ts_ms, offset_secs) else {
                continue;
            };
            let id = identity("heart_rate", &ts_ms.to_string());
            let mut row = new_row(
                RowKind::Reading,
                id.clone(),
                "heart_rate",
                "heart rate".to_owned(),
            );
            row.at = Some(at);
            row.value = Some(format!("{}", bpm.round() as i64));
            row.unit = Some("bpm".to_owned());
            rows.push(ReadRow {
                identity: id,
                row,
                garmin_day: date_str.and_then(parse_date),
            });
        }
    }
    rows
}

fn read_hrv(context_day: Option<&str>, payload: &Value) -> Vec<ReadRow> {
    let mut rows = Vec::new();
    let Some(obj) = payload.as_object() else {
        return rows;
    };
    // #10 `hrv/daily/{start}/{end}` returns `hrvSummaries: [...]`.
    if let Some(summaries) = obj.get("hrvSummaries").and_then(Value::as_array) {
        for summary in summaries.iter().filter_map(Value::as_object) {
            push_hrv_summary(summary, context_day, &mut rows);
        }
    }
    // #9 `hrv/{date}` returns `hrvSummary: {...}` + `hrvReadings: [...]`.
    if let Some(summary) = obj.get("hrvSummary").and_then(Value::as_object) {
        push_hrv_summary(summary, context_day, &mut rows);
    } else if obj.contains_key("calendarDate") && obj.contains_key("lastNightAvg") {
        push_hrv_summary(obj, context_day, &mut rows);
    }
    if let Some(readings) = obj.get("hrvReadings").and_then(Value::as_array) {
        for reading in readings.iter().filter_map(Value::as_object) {
            let Some(val) = f64_field(reading, "hrvValue").filter(|v| *v > 0.0) else {
                continue;
            };
            let Some(at) = rfc3339_from_local_and_gmt(
                str_field(reading, "readingTimeLocal"),
                str_field(reading, "readingTimeGMT"),
            ) else {
                continue;
            };
            let id = identity("hrv_reading", &at);
            let mut row = new_row(
                RowKind::Reading,
                id.clone(),
                "hrv_reading",
                "HRV".to_owned(),
            );
            row.at = Some(at.clone());
            row.value = Some(format!("{}", val.round() as i64));
            row.unit = Some("ms".to_owned());
            rows.push(ReadRow {
                identity: id,
                row,
                garmin_day: context_day.and_then(parse_date).or_else(|| parse_date(&at)),
            });
        }
    }
    rows
}

fn push_hrv_summary(
    summary: &Map<String, Value>,
    context_day: Option<&str>,
    rows: &mut Vec<ReadRow>,
) {
    let Some(date_str) = str_field(summary, "calendarDate").or(context_day) else {
        return;
    };
    let avg = f64_field(summary, "lastNightAvg").filter(|v| *v > 0.0);
    if avg.is_none() && summary.get("status").is_none_or(Value::is_null) {
        return;
    }
    let id = identity("hrv", date_str);
    let mut row = new_row(
        RowKind::Dated,
        id.clone(),
        "hrv",
        "overnight HRV".to_owned(),
    );
    row.date = Some(date_str.to_owned());
    if let Some(avg) = avg {
        row.value = Some(format!("{}", avg.round() as i64));
        row.unit = Some("ms".to_owned());
    }
    row.fields = object_fields(summary);
    ensure_row_within_byte_limit(&mut row);
    rows.push(ReadRow {
        identity: id,
        row,
        garmin_day: parse_date(date_str),
    });
}

fn read_spo2(context_day: Option<&str>, payload: &Value) -> Vec<ReadRow> {
    let Some(obj) = payload.as_object() else {
        return Vec::new();
    };
    let Some(date_str) = str_field(obj, "calendarDate").or(context_day) else {
        return Vec::new();
    };
    let avg = f64_field(obj, "averageSpO2").filter(|v| *v > 0.0);
    let low = f64_field(obj, "lowestSpO2").filter(|v| *v > 0.0);
    if avg.is_none() && low.is_none() {
        return Vec::new();
    }
    let id = identity("spo2", date_str);
    let mut row = new_row(RowKind::Dated, id.clone(), "spo2", "SpO2".to_owned());
    row.date = Some(date_str.to_owned());
    if let Some(avg) = avg {
        row.value = Some(format!("{avg:.1}"));
        row.unit = Some("%".to_owned());
    }
    row.fields = object_fields(obj);
    ensure_row_within_byte_limit(&mut row);
    vec![ReadRow {
        identity: id,
        row,
        garmin_day: parse_date(date_str),
    }]
}

fn read_weight(payload: &Value) -> Vec<ReadRow> {
    let mut metrics: Vec<&Map<String, Value>> = Vec::new();
    if let Some(obj) = payload.as_object() {
        if let Some(summaries) = obj.get("dailyWeightSummaries").and_then(Value::as_array) {
            for summary in summaries.iter().filter_map(Value::as_object) {
                if let Some(all) = summary.get("allWeightMetrics").and_then(Value::as_array) {
                    metrics.extend(all.iter().filter_map(Value::as_object));
                } else if let Some(latest) = summary.get("latestWeight").and_then(Value::as_object)
                {
                    metrics.push(latest);
                }
            }
        }
        if let Some(list) = obj.get("dateWeightList").and_then(Value::as_array) {
            metrics.extend(list.iter().filter_map(Value::as_object));
        }
    } else if let Some(list) = payload.as_array() {
        metrics.extend(list.iter().filter_map(Value::as_object));
    }
    let mut rows = Vec::new();
    for item in metrics {
        let Some(raw_weight) = f64_field(item, "weight").filter(|w| *w > 0.0) else {
            continue;
        };
        // Garmin returns weight in grams (e.g. 75200.0). Guard with a 500.0 threshold
        // so kg values are never divided by 1000.
        let kg = if raw_weight > 500.0 {
            raw_weight / 1000.0
        } else {
            raw_weight
        };
        let date_str = str_field(item, "calendarDate");
        let ts_gmt = i64_field(item, "timestampGMT").or_else(|| i64_field(item, "date"));
        let ts_local = i64_field(item, "date");
        let at = ts_gmt.and_then(|gmt| rfc3339_from_epoch_ms(gmt, ts_local.filter(|&l| l != gmt)));
        let sample_key = i64_field(item, "samplePk")
            .map(|pk| pk.to_string())
            .or_else(|| at.clone())
            .or_else(|| date_str.map(str::to_owned));
        let Some(sample_key) = sample_key else {
            continue;
        };
        let id = identity("weight", &sample_key);
        let mut fields = object_fields(item);
        fields.insert("weightKg".to_owned(), json!((kg * 100.0).round() / 100.0));
        let mut row = if let Some(at) = at {
            let mut r = new_row(
                RowKind::Reading,
                id.clone(),
                "body_mass",
                "weight".to_owned(),
            );
            r.at = Some(at);
            r
        } else if let Some(date_str) = date_str {
            let mut r = new_row(RowKind::Dated, id.clone(), "body_mass", "weight".to_owned());
            r.date = Some(date_str.to_owned());
            r
        } else {
            continue;
        };
        row.value = Some(format!("{kg:.2}"));
        row.unit = Some("kg".to_owned());
        row.fields = fields;
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: date_str.and_then(parse_date),
        });
    }
    rows
}

fn read_hydration(payload: &Value) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => vec![obj],
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(date_str) = str_field(item, "calendarDate") else {
            continue;
        };
        // Drop Garmin's zero-value placeholder hydration entries.
        let Some(ml) = f64_field(item, "valueInML").filter(|v| *v > 0.0) else {
            continue;
        };
        let id = identity("hydration", date_str);
        let mut row = new_row(
            RowKind::Dated,
            id.clone(),
            "hydration",
            "hydration".to_owned(),
        );
        row.date = Some(date_str.to_owned());
        row.value = Some(format!("{}", ml.round() as i64));
        row.unit = Some("mL".to_owned());
        row.fields = object_fields(item);
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: parse_date(date_str),
        });
    }
    rows
}

fn read_vo2_max(payload: &Value) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => vec![obj],
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let generic = item.get("generic").and_then(Value::as_object);
        let cycling = item.get("cycling").and_then(Value::as_object);
        let date_str = str_field(item, "calendarDate")
            .or_else(|| generic.and_then(|g| str_field(g, "calendarDate")))
            .or_else(|| cycling.and_then(|c| str_field(c, "calendarDate")));
        let Some(date_str) = date_str else {
            continue;
        };
        let has_value = generic
            .and_then(|g| {
                f64_field(g, "vo2MaxPreciseValue").or_else(|| f64_field(g, "vo2MaxValue"))
            })
            .is_some()
            || cycling
                .and_then(|c| {
                    f64_field(c, "vo2MaxPreciseValue").or_else(|| f64_field(c, "vo2MaxValue"))
                })
                .is_some();
        if !has_value {
            continue;
        }
        let id = identity("vo2_max", date_str);
        let mut row = new_row(RowKind::Dated, id.clone(), "vo2_max", "VO2 max".to_owned());
        row.date = Some(date_str.to_owned());
        row.fields = object_fields(item);
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: parse_date(date_str),
        });
    }
    rows
}

fn read_fitness_age(context_day: Option<&str>, payload: &Value) -> Vec<ReadRow> {
    // Garmin's fitnessage/v2 endpoint can return a single JSON object OR an array.
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => vec![obj],
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(date_str) = str_field(item, "calendarDate")
            .or_else(|| str_field(item, "date"))
            .or_else(|| str_field(item, "lastUpdated").and_then(|s| s.get(..10)))
            .or(context_day)
        else {
            continue;
        };
        if f64_field(item, "fitnessAge").is_none() && f64_field(item, "chronologicalAge").is_none()
        {
            continue;
        }
        let id = identity("fitness_age", date_str);
        let mut row = new_row(
            RowKind::Dated,
            id.clone(),
            "fitness_age",
            "fitness age".to_owned(),
        );
        row.date = Some(date_str.to_owned());
        row.fields = object_fields(item);
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: parse_date(date_str),
        });
    }
    rows
}

fn read_body_battery(payload: &Value) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => vec![obj],
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(date_str) = str_field(item, "date").or_else(|| str_field(item, "calendarDate"))
        else {
            continue;
        };
        let non_null_values: Vec<Value> = item
            .get("bodyBatteryValuesArray")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter(|pair| {
                        pair.as_array()
                            .and_then(|p| p.get(1))
                            .and_then(Value::as_f64)
                            .is_some()
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let has_data = i64_field(item, "charged").is_some()
            || i64_field(item, "drained").is_some()
            || !non_null_values.is_empty();
        if !has_data {
            continue;
        }
        let id = identity("body_battery", date_str);
        let mut row = new_row(
            RowKind::Dated,
            id.clone(),
            "body_battery",
            "Body Battery".to_owned(),
        );
        row.date = Some(date_str.to_owned());
        row.fields = object_fields(item);
        if non_null_values.is_empty() {
            row.fields.remove("bodyBatteryValuesArray");
        } else {
            row.fields.insert(
                "bodyBatteryValuesArray".to_owned(),
                Value::Array(non_null_values),
            );
        }

        // If an unusually dense day exceeds MAX_ROW_BYTES, split bodyBatteryValuesArray
        // deterministically across continuation rows so no intraday points are lost.
        if encode_rows(std::slice::from_ref(&row)).len() <= MAX_ROW_BYTES {
            rows.push(ReadRow {
                identity: id,
                row,
                garmin_day: parse_date(date_str),
            });
            continue;
        }
        let points = row
            .fields
            .remove("bodyBatteryValuesArray")
            .and_then(|v| match v {
                Value::Array(arr) => Some(arr),
                _ => None,
            })
            .unwrap_or_default();
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: parse_date(date_str),
        });
        for (part_idx, chunk) in points.chunks(250).enumerate() {
            let part_id = identity(
                "body_battery_values",
                &format!("{date_str}#{}", part_idx + 1),
            );
            let mut part_row = new_row(
                RowKind::Dated,
                part_id.clone(),
                "body_battery",
                "Body Battery".to_owned(),
            );
            part_row.date = Some(date_str.to_owned());
            part_row
                .fields
                .insert("part".to_owned(), json!(part_idx + 1));
            part_row.fields.insert(
                "bodyBatteryValuesArray".to_owned(),
                Value::Array(chunk.to_vec()),
            );
            ensure_row_within_byte_limit(&mut part_row);
            rows.push(ReadRow {
                identity: part_id,
                row: part_row,
                garmin_day: parse_date(date_str),
            });
        }
    }
    rows
}

fn read_menstrual_cycle(
    record_type: &str,
    label: &str,
    context_day: Option<&str>,
    payload: &Value,
) -> Vec<ReadRow> {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => vec![obj],
        _ => Vec::new(),
    };
    let mut rows = Vec::new();
    for item in items {
        let fields = object_fields(item);
        // Ignore empty / unconfigured shells (only nulls or empty nested maps/arrays).
        let meaningful = fields.values().any(|value| match value {
            Value::Null => false,
            Value::Array(arr) => !arr.is_empty(),
            Value::Object(map) => map.values().any(|v| !v.is_null()),
            Value::String(s) => !s.trim().is_empty(),
            Value::Bool(_) | Value::Number(_) => true,
        });
        if !meaningful {
            continue;
        }
        let date_str = str_field(item, "calendarDate")
            .or_else(|| str_field(item, "startDate"))
            .or_else(|| {
                item.get("daySummary")
                    .and_then(Value::as_object)
                    .and_then(|s| str_field(s, "calendarDate"))
            })
            .or(context_day);
        let Some(date_str) = date_str else {
            continue;
        };
        let id = identity(record_type, date_str);
        let mut row = new_row(RowKind::Dated, id.clone(), record_type, label.to_owned());
        row.date = Some(date_str.to_owned());
        row.fields = fields;
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: parse_date(date_str),
        });
    }
    rows
}

fn read_nutrition(context_day: Option<&str>, payload: &Value) -> Vec<ReadRow> {
    let mut entries: Vec<&Map<String, Value>> = Vec::new();
    let mut envelope_day = context_day;
    match payload {
        Value::Array(list) => entries.extend(list.iter().filter_map(Value::as_object)),
        Value::Object(obj) => {
            envelope_day = str_field(obj, "mealDate")
                .or_else(|| str_field(obj, "calendarDate"))
                .or(context_day);
            let mut found_list = false;
            for key in [
                "loggedFoodsWithServingSizes",
                "mealDetails",
                "foodLogs",
                "meals",
                "entries",
            ] {
                if let Some(list) = obj.get(key).and_then(Value::as_array) {
                    found_list = true;
                    entries.extend(list.iter().filter_map(Value::as_object));
                }
            }
            if !found_list
                && obj.values().any(|v| !v.is_null())
                && (obj.contains_key("calories")
                    || obj.contains_key("foodName")
                    || obj.contains_key("mealName"))
            {
                entries.push(obj);
            }
        }
        _ => {}
    }
    let mut rows = Vec::new();
    for (idx, item) in entries.into_iter().enumerate() {
        let date_str = str_field(item, "calendarDate")
            .or_else(|| str_field(item, "mealDate"))
            .or_else(|| str_field(item, "date"))
            .or(envelope_day);
        let start = rfc3339_from_local_and_gmt(
            str_field(item, "loggedTimeLocal").or_else(|| str_field(item, "timestampLocal")),
            str_field(item, "loggedTimeGMT").or_else(|| str_field(item, "timestampGMT")),
        );
        let key = str_field(item, "id")
            .or_else(|| str_field(item, "foodLogId"))
            .map(str::to_owned)
            .or_else(|| i64_field(item, "id").map(|n| n.to_string()))
            .unwrap_or_else(|| format!("{}:{idx}", date_str.unwrap_or("food")));
        let id = identity("nutrition", &key);
        let mut row = if let Some(start) = start {
            let mut r = new_row(
                RowKind::Logged,
                id.clone(),
                "nutrition",
                "nutrition".to_owned(),
            );
            r.start = Some(start.clone());
            r.end = Some(start);
            r
        } else if let Some(date_str) = date_str {
            let mut r = new_row(
                RowKind::Dated,
                id.clone(),
                "nutrition",
                "nutrition".to_owned(),
            );
            r.date = Some(date_str.to_owned());
            r
        } else {
            continue;
        };
        for word_key in ["foodName", "mealName", "name", "brandName"] {
            if let Some(word) = str_field(item, word_key)
                && !row.words.iter().any(|existing| existing == word)
            {
                row.words.push(word.to_owned());
            }
        }
        row.fields = object_fields(item);
        ensure_row_within_byte_limit(&mut row);
        rows.push(ReadRow {
            identity: id,
            row,
            garmin_day: date_str.and_then(parse_date),
        });
    }
    rows
}

/// Place normalized Garmin rows onto the shared body civil-day window grid,
/// tiling experiences and placing workout-covered readings on the 300s grid.
pub fn place_rows_by_day(
    journal: &Path,
    zone: &TimeZone,
    read_rows: &[ReadRow],
) -> Result<BTreeMap<String, DayRows>, String> {
    let mut by_day: BTreeMap<String, BTreeMap<String, BTreeMap<String, BodyRow>>> = BTreeMap::new();
    let millis = |text: Option<&str>| {
        text.and_then(parse_source_time)
            .map(|time| epoch_millisecond(time.timestamp()))
    };
    let mut sessions = Vec::new();
    for entry in read_rows
        .iter()
        .filter(|entry| entry.row.kind == RowKind::Experience)
    {
        let tiles = tile_experience(journal, &GARMIN, &entry.row, zone)?;
        if is_session(&entry.row)
            && let (Some(start), Some(end)) = (
                millis(entry.row.start.as_deref()),
                millis(tiles[0].1.end.as_deref()),
            )
        {
            sessions.push((entry.row.by.clone(), start, end));
        }
        for (window, row) in tiles {
            let key = resolve_key(journal, &GARMIN, &window.0, &window.1);
            by_day
                .entry(window.0)
                .or_default()
                .entry(key)
                .or_default()
                .insert(row.id.clone(), row);
        }
    }
    for entry in read_rows
        .iter()
        .filter(|entry| entry.row.kind != RowKind::Experience)
    {
        let row = &entry.row;
        let covered = row.kind == RowKind::Reading
            && row.date.is_none()
            && millis(row.begins()).is_some_and(|at| {
                sessions
                    .iter()
                    .any(|(by, start, end)| *by == row.by && *start <= at && at < *end)
            });
        let window = row_window(row, zone, covered)?;
        let key = resolve_key(journal, &GARMIN, &window.0, &window.1);
        by_day
            .entry(window.0)
            .or_default()
            .entry(key)
            .or_default()
            .insert(row.id.clone(), row.clone());
    }
    Ok(by_day
        .into_iter()
        .map(|(day, windows)| {
            let day_rows = windows
                .into_iter()
                .map(|(key, rows)| ((day.clone(), key), rows))
                .collect();
            (day, day_rows)
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use solstone_core_format::content::body_records::{is_womens_health_row, user_entered};

    #[test]
    fn reconstructs_exact_offset_from_garmin_local_and_gmt_pairs() {
        assert_eq!(
            rfc3339_from_local_and_gmt(Some("2026-11-24 07:15:00"), Some("2026-11-24 14:15:00")),
            Some("2026-11-24T07:15:00-07:00".to_owned())
        );
        assert_eq!(
            rfc3339_from_local_and_gmt(Some("2026-11-24 09:00:00"), Some("2026-11-24 03:30:00")),
            Some("2026-11-24T09:00:00+05:30".to_owned())
        );
        assert_eq!(
            rfc3339_from_epoch_ms(
                1_764_000_000_000,
                Some(1_764_000_000_000 - 7 * 3_600 * 1_000)
            ),
            Some("2025-11-24T09:00:00-07:00".to_owned())
        );
    }

    #[test]
    fn normalizes_activity_keeping_gps_and_searchable_title_and_i64_id() {
        let payload = json!([{
            "activityId": 21_481_920_331_i64,
            "activityName": "Boulder Running",
            "activityType": {"typeKey": "trail_running"},
            "startTimeLocal": "2026-11-24 07:15:00",
            "startTimeGMT": "2026-11-24 14:15:00",
            "duration": 2400.0,
            "distance": 8045.5,
            "averageHR": 152.0,
            "calories": 580.0,
            "startLatitude": 40.014984,
            "startLongitude": -105.270546,
            "endLatitude": 40.021100,
            "endLongitude": -105.280100,
            "locationName": "Boulder",
            "manualActivity": true
        }]);
        let rows = read_page("activities", None, &payload);
        assert_eq!(rows.len(), 1);
        let row = &rows[0].row;
        assert_eq!(row.kind, RowKind::Experience);
        assert_eq!(row.experience.as_deref(), Some("workout"));
        assert_eq!(row.record_type, "trail_running");
        assert_eq!(row.words, vec!["Boulder Running"]);
        assert_eq!(row.start.as_deref(), Some("2026-11-24T07:15:00-07:00"));
        assert_eq!(row.end.as_deref(), Some("2026-11-24T07:55:00-07:00"));
        assert_eq!(row.fields["startLatitude"], json!(40.014984));
        assert_eq!(row.fields["locationName"], json!("Boulder"));
        assert!(user_entered(row));
    }

    #[test]
    fn normalizes_sleep_empty_placeholder_scores_and_naps() {
        let empty = json!({"dailySleepDTO": {"id": null, "calendarDate": "2026-11-24"}});
        assert!(read_page("sleep", Some("2026-11-24"), &empty).is_empty());

        let payload = json!({
            "dailySleepDTO": {
                "id": 99_887_766_554_i64,
                "calendarDate": "2026-11-24",
                "sleepStartTimestampGMT": 1_763_964_000_000_i64,
                "sleepStartTimestampLocal": 1_763_938_800_000_i64,
                "sleepEndTimestampGMT": 1_763_992_800_000_i64,
                "sleepEndTimestampLocal": 1_763_967_600_000_i64,
                "sleepTimeSeconds": 27_600,
                "sleepScores": {"overall": {"value": 84}},
                "sleepScoreFeedback": "POSITIVE_LONG_AND_REFRESHING",
                "dailyNapDTOS": [{
                    "napId": "nap-1",
                    "napStartTimestampLocal": "2026-11-24 14:00:00",
                    "napStartTimestampGMT": "2026-11-24 21:00:00",
                    "napEndTimestampLocal": "2026-11-24 14:30:00",
                    "napEndTimestampGMT": "2026-11-24 21:30:00",
                    "napTimeSec": 1800
                }]
            }
        });
        let rows = read_page("sleep", Some("2026-11-24"), &payload);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].row.experience.as_deref(), Some("sleep"));
        assert_eq!(rows[0].row.fields["sleepTimeSeconds"], json!(27_600));
        assert!(!rows[0].row.fields.contains_key("sleepScores"));
        assert_eq!(rows[1].row.kind, RowKind::Dated);
        assert_eq!(rows[1].row.record_type, "sleep_scores");
        assert_eq!(rows[2].row.experience.as_deref(), Some("nap"));
    }

    #[test]
    fn converts_weight_grams_with_guard_drops_zero_hydration_and_stress_sentinels() {
        let weight = json!({
            "dailyWeightSummaries": [{
                "allWeightMetrics": [
                    {"samplePk": 1, "calendarDate": "2026-11-24", "weight": 75200.0},
                    {"samplePk": 2, "calendarDate": "2026-11-23", "weight": 74.8}
                ]
            }]
        });
        let rows = read_page("weight", None, &weight);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].row.value.as_deref(), Some("75.20"));
        assert_eq!(rows[0].row.unit.as_deref(), Some("kg"));
        assert_eq!(rows[1].row.value.as_deref(), Some("74.80"));

        let hydration = json!([
            {"calendarDate": "2026-11-23", "valueInML": 0.0},
            {"calendarDate": "2026-11-24", "valueInML": 2100.0}
        ]);
        let h_rows = read_page("hydration", None, &hydration);
        assert_eq!(h_rows.len(), 1);
        assert_eq!(h_rows[0].row.value.as_deref(), Some("2100"));

        let summary = json!([{
            "calendarDate": "2026-11-24",
            "totalSteps": 9123,
            "averageStressLevel": -1,
            "maxStressLevel": -2,
            "restingHeartRate": 54
        }]);
        let s_rows = read_page("daily_summary", None, &summary);
        assert_eq!(s_rows.len(), 2);
        assert!(!s_rows[0].row.fields.contains_key("averageStressLevel"));
        assert!(!s_rows[0].row.fields.contains_key("maxStressLevel"));
        assert_eq!(s_rows[0].row.fields["steps"], json!(9123));
        assert_eq!(s_rows[1].row.record_type, "resting_heart_rate");
    }

    #[test]
    fn fitness_age_accepts_object_or_array_and_health_snapshot_keeps_device_model() {
        let single =
            json!({"calendarDate": "2026-11-24", "fitnessAge": 31.5, "chronologicalAge": 38});
        let arr =
            json!([{"calendarDate": "2026-11-24", "fitnessAge": 31.5, "chronologicalAge": 38}]);
        assert_eq!(
            read_page("fitness_age", Some("2026-11-24"), &single).len(),
            1
        );
        assert_eq!(read_page("fitness_age", Some("2026-11-24"), &arr).len(), 1);

        let snap = json!([{
            "summaryId": "hs-1",
            "startTime": "2026-11-24 08:00:00",
            "startTimeGMT": "2026-11-24 15:00:00",
            "durationSeconds": 120.0,
            "deviceMetaData": {"deviceName": "Forerunner 965"}
        }]);
        let rows = read_page("health_snapshot", None, &snap);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].row.by.as_deref(), Some("Garmin Forerunner 965"));
    }

    #[test]
    fn menstrual_cycle_rows_are_flagged_as_womens_health_and_empty_shells_ignored() {
        let empty = json!({"calendarDate": null, "daySummary": null});
        assert!(read_page("menstrual_cycle_dayview", Some("2026-11-24"), &empty).is_empty());

        let populated = json!({"calendarDate": "2026-11-24", "currentPhase": 2, "cycleLength": 28});
        let rows = read_page("menstrual_cycle_summary", Some("2026-11-24"), &populated);
        assert_eq!(rows.len(), 1);
        assert!(is_womens_health_row(&rows[0].row));
    }

    #[test]
    fn live_v2_daily_summaries_null_body_battery_and_empty_nutrition_parse_cleanly() {
        let v2 = json!({
            "data": [{
                "calendarDate": "2026-11-22",
                "movement": {"steps": {"value": 6420, "goal": 10000}},
                "calories": {"burnedResting": 1800, "burnedActive": 340, "burnedTotal": 2140},
                "heartRate": {"resting": 56, "min": 52, "max": 132},
                "stress": {"avgLevel": -1, "qualifier": "unknown"},
                "pulseOx": {"avgAltitudeInMeters": 1539.0}
            }]
        });
        let rows = read_page("daily_summary", None, &v2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].row.fields["steps"], json!(6420.0));
        assert_eq!(rows[0].row.fields["active_calories"], json!(340.0));
        assert!(
            rows[0].row.fields["stress"]
                .as_object()
                .unwrap()
                .get("avgLevel")
                .is_none()
        );
        assert_eq!(rows[1].row.record_type, "resting_heart_rate");
        assert_eq!(rows[1].row.value.as_deref(), Some("56"));

        let empty_bb = json!([{
            "date": "2026-11-18",
            "charged": null,
            "drained": null,
            "bodyBatteryValuesArray": [[1794960000001_i64, null], [1794960000002_i64, null]]
        }]);
        assert!(read_page("body_battery", None, &empty_bb).is_empty());

        let empty_nutr = json!({
            "mealDate": "2026-11-24",
            "dayStartTime": "07:00:00",
            "dayEndTime": "22:00:00",
            "mealDetails": [],
            "loggedFoodsWithServingSizes": []
        });
        assert!(read_page("nutrition", Some("2026-11-24"), &empty_nutr).is_empty());

        let fa = json!({
            "chronologicalAge": 40,
            "lastUpdated": "2026-11-24T00:00:00.0"
        });
        let fa_rows = read_page("fitness_age", None, &fa);
        assert_eq!(fa_rows.len(), 1);
        assert_eq!(fa_rows[0].row.date.as_deref(), Some("2026-11-24"));
    }
}
