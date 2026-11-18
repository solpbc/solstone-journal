// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure derived facts for already-loaded journal entity records.

use jiff::civil::Time;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use solstone_core_time::{day_key, from_local, parse_day, parse_rfc3339};

/// Fields that record when something happened to an entity record.
const ACTIVITY_FIELDS: [&str; 3] = ["updated_at", "attached_at", "created_at"];

/// Return the latest activity recorded on one entity record, in epoch
/// milliseconds.
///
/// Activity is the latest of a `last_seen` journal day and the `updated_at`,
/// `attached_at` and `created_at` timestamps. Journals hold those timestamps
/// as epoch milliseconds or as RFC 3339 text, depending on which release
/// wrote them, and both count. A record with no usable activity returns
/// `None`, so no surface shows a date the journal does not have.
pub fn entity_last_active_ts(entity: &Value, zone: &TimeZone) -> Option<i64> {
    let last_seen = valid_last_seen(entity).and_then(|day| journal_day_start_ms(day, zone));
    ACTIVITY_FIELDS
        .iter()
        .filter_map(|field| timestamp_ms(entity.get(*field)))
        .chain(last_seen)
        .max()
}

/// Convert an epoch-millisecond timestamp to its day on the given time zone.
pub fn last_active_day_for_ts(ts_ms: i64, zone: &TimeZone) -> Option<String> {
    Timestamp::from_millisecond(ts_ms)
        .ok()
        .map(|value| day_key(value.to_zoned_datetime(zone.clone()).date()))
}

/// Return the entity's activity day on the given time zone, when it has one.
pub fn entity_last_active_day(entity: &Value, zone: &TimeZone) -> Option<String> {
    entity_last_active_ts(entity, zone).and_then(|ts| last_active_day_for_ts(ts, zone))
}

/// Read a stored timestamp written as epoch milliseconds or RFC 3339 text.
pub(crate) fn timestamp_ms(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().filter(|value| *value > 0),
        Value::String(text) => parse_rfc3339(text)
            .ok()
            .map(|value| value.timestamp().as_millisecond())
            .filter(|value| *value > 0),
        _ => None,
    }
}

/// The first instant of a `YYYYMMDD` day on the given time zone, in epoch milliseconds.
///
/// A day whose midnight a daylight-saving change skips starts an hour later.
pub fn journal_day_start_ms(day: &str, zone: &TimeZone) -> Option<i64> {
    let midnight = parse_day(day)?.to_datetime(Time::midnight());
    from_local(zone, midnight)
        .earliest()
        .or_else(|| {
            let hour_later = midnight.checked_add(SignedDuration::from_hours(1)).ok()?;
            from_local(zone, hour_later).earliest()
        })
        .map(|value| value.timestamp().as_millisecond())
}

/// Validate the raw entity type spelling accepted by the Rust entity reader.
pub fn is_valid_entity_type(entity_type: &str) -> bool {
    entity_type.trim().chars().count() >= 3
        && entity_type
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == ' ')
        && entity_type
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
}

/// Whether a configured identity name matches this entity's name or aliases.
pub fn entity_matches_identity_name(
    name: &str,
    aka: Option<&[String]>,
    identity_names: &[String],
) -> bool {
    let name = name.to_lowercase();
    identity_names.iter().any(|identity_name| {
        let identity_name = identity_name.to_lowercase();
        identity_name == name
            || aka.is_some_and(|aka| {
                aka.iter()
                    .any(|alias| identity_name == alias.to_lowercase())
            })
    })
}

fn valid_last_seen(entity: &Value) -> Option<&str> {
    let last_seen = entity.get("last_seen")?.as_str()?;
    (last_seen.len() == 8 && parse_day(last_seen).is_some()).then_some(last_seen)
}
