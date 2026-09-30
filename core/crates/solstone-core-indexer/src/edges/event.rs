// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use serde_json::Value;

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

        let ts = event_timestamp(&context.day, event.get("start"), resolver.owner_timezone()?)?;
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
fn event_base_ts(day: &str, timezone: Tz) -> i64 {
    if day.is_empty() {
        return 0;
    }
    let Ok(date) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return 0;
    };
    let Some(midnight) = date.and_hms_opt(0, 0, 0) else {
        return 0;
    };
    resolve_midnight(timezone, midnight)
}

fn event_timestamp(day: &str, start_time: Option<&Value>, timezone: Tz) -> Result<i64, EdgeError> {
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

fn resolve_midnight(timezone: Tz, midnight: NaiveDateTime) -> i64 {
    match select_local_result(timezone.from_local_datetime(&midnight)) {
        LocalSelection::EpochMillis(ts) => ts,
        LocalSelection::Gap => first_valid_after_gap(timezone, midnight),
    }
}

pub(super) fn select_local_result<Tz: TimeZone>(
    result: LocalResult<DateTime<Tz>>,
) -> LocalSelection {
    match result {
        LocalResult::Single(value) => LocalSelection::EpochMillis(value.timestamp_millis()),
        LocalResult::Ambiguous(left, right) => {
            LocalSelection::EpochMillis(left.timestamp_millis().min(right.timestamp_millis()))
        }
        LocalResult::None => LocalSelection::Gap,
    }
}

fn first_valid_after_gap(timezone: Tz, midnight: NaiveDateTime) -> i64 {
    for hours in 1..=3 {
        let Some(candidate) = midnight.checked_add_signed(Duration::hours(hours)) else {
            return 0;
        };
        if let LocalSelection::EpochMillis(ts) =
            select_local_result(timezone.from_local_datetime(&candidate))
        {
            return ts;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use serde_json::json;

    #[test]
    fn local_result_selection_chooses_earliest_utc_and_handles_gap() {
        let offset = FixedOffset::west_opt(5 * 3600).expect("fixed offset");
        let late = offset
            .with_ymd_and_hms(2000, 10, 29, 0, 0, 0)
            .single()
            .expect("late candidate");
        let early = FixedOffset::west_opt(4 * 3600)
            .expect("fixed offset")
            .with_ymd_and_hms(2000, 10, 29, 0, 0, 0)
            .single()
            .expect("early candidate");

        match select_local_result(LocalResult::Ambiguous(late, early)) {
            LocalSelection::EpochMillis(ts) => assert_eq!(ts, early.timestamp_millis()),
            LocalSelection::Gap => panic!("ambiguous should select an epoch"),
        }
        assert!(matches!(
            select_local_result::<FixedOffset>(LocalResult::None),
            LocalSelection::Gap
        ));
    }

    #[test]
    fn a_day_starts_at_the_owner_zones_midnight_not_this_computers() {
        let midnight = NaiveDate::from_ymd_opt(2026, 4, 30)
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .expect("midnight");
        let at = |zone: Tz| {
            zone.from_local_datetime(&midnight)
                .earliest()
                .expect("midnight exists")
                .timestamp_millis()
        };
        let host = at(solstone_core_journal_config::host_zone());
        let zone = [Tz::Pacific__Kiritimati, Tz::Etc__GMTPlus12]
            .into_iter()
            .find(|zone| at(*zone) != host)
            .expect("UTC+14 and UTC-12 never share a midnight");
        assert_eq!(event_base_ts("20260430", zone), at(zone));
        assert_ne!(event_base_ts("20260430", zone), host);
    }

    #[test]
    fn a_day_whose_midnight_is_skipped_starts_at_the_first_hour_that_exists() {
        // Havana springs forward at midnight, so 2026-03-08 has no 00:00.
        let first = Tz::America__Havana
            .with_ymd_and_hms(2026, 3, 8, 1, 0, 0)
            .single()
            .expect("01:00 exists")
            .timestamp_millis();
        assert_eq!(event_base_ts("20260308", Tz::America__Havana), first);
    }

    #[test]
    fn event_timestamp_matches_python_fallback_and_overflow_boundaries() {
        let base = event_base_ts("20260430", Tz::Asia__Tokyo);
        assert_eq!(base, 1_777_474_800_000);
        assert_eq!(event_timestamp("20260430", None, Tz::Asia__Tokyo), Ok(base));
        assert_eq!(
            event_timestamp("20260430", Some(&json!("")), Tz::Asia__Tokyo),
            Ok(base)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("abc")), Tz::Asia__Tokyo),
            Ok(base)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("01:abc")), Tz::Asia__Tokyo),
            Ok(base)
        );
        assert_eq!(
            event_timestamp("20260430", Some(&json!("01:02:03")), Tz::Asia__Tokyo),
            Ok(base + 3_723_000)
        );
        assert_eq!(
            event_timestamp(
                "20260430",
                Some(&json!("01:02:03:ignored")),
                Tz::Asia__Tokyo
            ),
            Ok(base + 3_723_000)
        );
        assert_eq!(
            event_timestamp(
                "20260430",
                Some(&json!("999999999999999999999")),
                Tz::Asia__Tokyo
            ),
            Err(EdgeError::EventTimestampOutOfRange)
        );
    }
}
