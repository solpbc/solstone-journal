// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure derived facts for already-loaded journal entity records.

use chrono::{Local, LocalResult, NaiveDate, TimeZone};
use serde_json::Value;

/// Fallback activity timestamp for entities without activity metadata.
pub const DEFAULT_ACTIVITY_TS: i64 = 1_767_225_600_000;

/// Return the entity's activity timestamp using its journal-record fallback chain.
pub fn entity_last_active_ts(entity: &Value) -> i64 {
    if let Some(last_seen) = valid_last_seen(entity)
        && let Ok(date) = NaiveDate::parse_from_str(last_seen, "%Y%m%d")
    {
        let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
        match Local.from_local_datetime(&midnight) {
            LocalResult::Single(value) => return value.timestamp_millis(),
            LocalResult::Ambiguous(first, second) => {
                return first.timestamp_millis().min(second.timestamp_millis());
            }
            LocalResult::None => {}
        }
    }
    positive_integer(entity.get("updated_at"))
        .or_else(|| positive_integer(entity.get("attached_at")))
        .unwrap_or(DEFAULT_ACTIVITY_TS)
}

/// Convert an epoch-millisecond timestamp to its journal-local day.
pub fn last_active_day_for_ts(ts_ms: i64) -> String {
    Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .map(|value| value.format("%Y%m%d").to_string())
        .unwrap_or_else(|| {
            Local
                .timestamp_millis_opt(DEFAULT_ACTIVITY_TS)
                .single()
                .expect("default timestamp is valid")
                .format("%Y%m%d")
                .to_string()
        })
}

/// Return the entity's journal-local activity day.
pub fn entity_last_active_day(entity: &Value) -> String {
    valid_last_seen(entity)
        .map(str::to_owned)
        .unwrap_or_else(|| last_active_day_for_ts(entity_last_active_ts(entity)))
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
    (last_seen.len() == 8 && NaiveDate::parse_from_str(last_seen, "%Y%m%d").is_ok())
        .then_some(last_seen)
}

fn positive_integer(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|value| *value > 0)
}
