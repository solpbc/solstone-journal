// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::SignedDuration;
use jiff::civil::{DateTime, Time};
use jiff::tz::TimeZone;
use serde_json::Value;
use solstone_core_time::{LocalResult, from_local};

use super::candidates::EdgeResolver;
use super::{
    EdgeContext, EdgeError, EdgeRow, EdgeValue, JsonObject, PythonIntParse, json_truthy,
    parse_python_int_literal, python_str,
};
use solstone_core_journal::python_strip;

pub(super) enum LocalSelection {
    EpochMillis(i64),
    Gap,
}

pub(crate) fn extract_event_edges(
    entries: &[JsonObject],
    context: &EdgeContext,
    resolver: &mut EdgeResolver,
) -> Result<Vec<EdgeRow>, EdgeError> {
    let mut rows = Vec::new();

    for event in entries {
        if solstone_core_format::content::is_retired_event_plan(event) {
            continue;
        }
        let title = event.get("title");
        if !json_truthy(title) {
            continue;
        }
        let Some(title) = title else {
            continue;
        };
        let label = python_strip(&python_str(title, "title")?).to_string();
        let Some(Value::Array(participants)) = event.get("participants") else {
            continue;
        };

        let mut resolved = Vec::new();
        for name in participants {
            let Value::String(name) = name else {
                continue;
            };
            let stripped = python_strip(name);
            if stripped.is_empty() {
                continue;
            }
            let Some(entity_id) = resolver.resolve(context, name)? else {
                continue;
            };
            if !resolved
                .iter()
                .any(|(seen, _): &(String, String)| seen == &entity_id)
            {
                resolved.push((entity_id, stripped.to_string()));
            }
        }

        let ts = event_timestamp(
            &context.day,
            event.get("start"),
            &resolver.owner_timezone()?,
        )?;
        for left_index in 0..resolved.len() {
            for (dst_id, dst_name) in resolved.iter().skip(left_index + 1) {
                let (src_id, src_name) = &resolved[left_index];
                if src_id == dst_id {
                    continue;
                }
                rows.push(EdgeRow {
                    src: src_id.clone(),
                    dst: dst_id.clone(),
                    kind: "attended-with".to_string(),
                    src_name: EdgeValue::Text(src_name.clone()),
                    dst_name: EdgeValue::Text(dst_name.clone()),
                    day: Some(context.day.clone()),
                    facet: Some(context.facet.clone()),
                    source: "event-legacy".to_string(),
                    path: context.path.clone(),
                    anchor: Some(String::new()),
                    label: EdgeValue::Text(label.clone()),
                    ts: EdgeValue::Int(ts),
                    weight: 1,
                });
            }
        }
    }

    Ok(rows)
}

/// Midnight of `day` in the owner's zone: the clock an event's `start` is
/// written on.
fn event_base_ts(day: &str, timezone: &TimeZone) -> i64 {
    if day.is_empty() {
        return 0;
    }
    let Some(date) = solstone_core_time::parse_day(day) else {
        return 0;
    };
    resolve_midnight(timezone, date.to_datetime(Time::midnight()))
}

fn event_timestamp(
    day: &str,
    start_time: Option<&Value>,
    timezone: &TimeZone,
) -> Result<i64, EdgeError> {
    let base_ts = event_base_ts(day, timezone);
    if base_ts == 0 {
        return Ok(0);
    }
    let Some(Value::String(start_time)) = start_time else {
        return Ok(base_ts);
    };
    if start_time.is_empty() {
        return Ok(base_ts);
    }

    let parts: Vec<&str> = start_time.split(':').collect();
    let Some(hours) = parse_time_part(parts.first().copied())? else {
        return Ok(base_ts);
    };
    let minutes = if parts.len() > 1 {
        let Some(minutes) = parse_time_part(parts.get(1).copied())? else {
            return Ok(base_ts);
        };
        minutes
    } else {
        0
    };
    let seconds = if parts.len() > 2 {
        let Some(seconds) = parse_time_part(parts.get(2).copied())? else {
            return Ok(base_ts);
        };
        seconds
    } else {
        0
    };

    let offset_seconds = (hours as i128)
        .checked_mul(3600)
        .and_then(|value| value.checked_add((minutes as i128).checked_mul(60)?))
        .and_then(|value| value.checked_add(seconds as i128))
        .ok_or(EdgeError::EventTimestampOutOfRange)?;
    let offset_ms = offset_seconds
        .checked_mul(1000)
        .ok_or(EdgeError::EventTimestampOutOfRange)?;
    let ts = (base_ts as i128)
        .checked_add(offset_ms)
        .ok_or(EdgeError::EventTimestampOutOfRange)?;
    i64::try_from(ts).map_err(|_error| EdgeError::EventTimestampOutOfRange)
}

fn parse_time_part(value: Option<&str>) -> Result<Option<i64>, EdgeError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match parse_python_int_literal(value) {
        PythonIntParse::Value(value) => Ok(Some(value)),
        PythonIntParse::Invalid => Ok(None),
        PythonIntParse::OutOfRange => Err(EdgeError::EventTimestampOutOfRange),
    }
}

fn resolve_midnight(timezone: &TimeZone, midnight: DateTime) -> i64 {
    match select_local_result(from_local(timezone, midnight)) {
        LocalSelection::EpochMillis(ts) => ts,
        LocalSelection::Gap => first_valid_after_gap(timezone, midnight),
    }
}

pub(super) fn select_local_result(result: LocalResult) -> LocalSelection {
    match result {
        LocalResult::Single(value) => {
            LocalSelection::EpochMillis(value.timestamp().as_millisecond())
        }
        LocalResult::Ambiguous(left, right) => LocalSelection::EpochMillis(
            left.timestamp()
                .as_millisecond()
                .min(right.timestamp().as_millisecond()),
        ),
        LocalResult::None => LocalSelection::Gap,
    }
}

fn first_valid_after_gap(timezone: &TimeZone, midnight: DateTime) -> i64 {
    for hours in 1..=3 {
        let Ok(candidate) = midnight.checked_add(SignedDuration::from_hours(hours)) else {
            return 0;
        };
        if let LocalSelection::EpochMillis(ts) =
            select_local_result(from_local(timezone, candidate))
        {
            return ts;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;
    use jiff::tz::offset;
    use serde_json::json;

    fn zone(name: &str) -> TimeZone {
        solstone_core_time::zone(name).expect("known zone")
    }

    #[test]
    fn local_result_selection_chooses_earliest_utc_and_handles_gap() {
        let wall = date(2000, 10, 29).at(0, 0, 0, 0);
        let late = from_local(&TimeZone::fixed(offset(-5)), wall)
            .single()
            .expect("late candidate");
        let early = from_local(&TimeZone::fixed(offset(-4)), wall)
            .single()
            .expect("early candidate");
        let early_ms = early.timestamp().as_millisecond();

        match select_local_result(LocalResult::Ambiguous(late, early)) {
            LocalSelection::EpochMillis(ts) => assert_eq!(ts, early_ms),
            LocalSelection::Gap => panic!("ambiguous should select an epoch"),
        }
        assert!(matches!(
            select_local_result(LocalResult::None),
            LocalSelection::Gap
        ));
    }

    #[test]
    fn a_day_starts_at_the_owner_zones_midnight_not_this_computers() {
        let midnight = date(2026, 4, 30).at(0, 0, 0, 0);
        let at = |zone: &TimeZone| {
            from_local(zone, midnight)
                .earliest()
                .expect("midnight exists")
                .timestamp()
                .as_millisecond()
        };
        let host = at(&solstone_core_journal_config::host_zone());
        let zone = [zone("Pacific/Kiritimati"), zone("Etc/GMT+12")]
            .into_iter()
            .find(|zone| at(zone) != host)
            .expect("UTC+14 and UTC-12 never share a midnight");
        assert_eq!(event_base_ts("20260430", &zone), at(&zone));
        assert_ne!(event_base_ts("20260430", &zone), host);
    }

    #[test]
    fn a_day_whose_midnight_is_skipped_starts_at_the_first_hour_that_exists() {
        // Havana springs forward at midnight, so 2026-03-08 has no 00:00.
        let havana = zone("America/Havana");
        let first = from_local(&havana, date(2026, 3, 8).at(1, 0, 0, 0))
            .single()
            .expect("01:00 exists")
            .timestamp()
            .as_millisecond();
        assert_eq!(event_base_ts("20260308", &havana), first);
    }

    #[test]
    fn event_timestamp_matches_python_fallback_and_overflow_boundaries() {
        let tokyo = zone("Asia/Tokyo");
        let base = event_base_ts("20260430", &tokyo);
        assert_eq!(base, 1_777_474_800_000);
        assert_eq!(event_timestamp("20260430", None, &tokyo), Ok(base));
        assert_eq!(
            event_timestamp("20260430", Some(&json!("")), &tokyo),
            Ok(base)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("abc")), &tokyo),
            Ok(base)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("01:abc")), &tokyo),
            Ok(base)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("01:02:03")), &tokyo),
            Ok(base + 3_723_000)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("01:02:03:ignored")), &tokyo),
            Ok(base + 3_723_000)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("999999999999999999999")), &tokyo),
            Err(EdgeError::EventTimestampOutOfRange)
        );
    }
}
