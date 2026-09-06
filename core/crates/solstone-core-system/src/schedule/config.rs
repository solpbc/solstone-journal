// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, MalformedPolicy, ReadError, atomic_replace, hold_lock, read_json,
};

use super::ScheduleError;

pub(crate) const RESERVED_METADATA_KEYS: [&str; 3] = ["daily_time", "weekly_day", "weekly_time"];

const DEFAULT_DAILY_TIME: &str = "00:15";
const DEFAULT_WEEKLY_TIME: &str = "03:15";

/// A validated enabled schedule entry. `every` intentionally retains its raw form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleEntry {
    pub cmd: Vec<String>,
    pub every: String,
    pub max_runtime: Option<Duration>,
}

/// Runtime configuration metadata and enabled entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleConfig {
    pub entries: BTreeMap<String, ScheduleEntry>,
    pub daily_time: Option<String>,
    pub weekly_day: Option<String>,
    pub weekly_time: Option<String>,
}

/// A non-fatal runtime configuration problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    pub message: String,
}

/// Tolerant runtime-load result. Whole-file defects remain observable here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConfigLoad {
    pub config: ScheduleConfig,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

pub(crate) fn load_runtime(path: &Path) -> Result<ConfigLoad, ScheduleError> {
    if !path.exists() {
        return Ok(ConfigLoad::default());
    }
    let raw = match read_json::<Value>(path, Value::Null, MalformedPolicy::Raise) {
        Ok(raw) => raw,
        Err(ReadError::Malformed(_)) => {
            return Ok(ConfigLoad {
                config: ScheduleConfig::default(),
                diagnostics: vec![diagnostic("malformed schedules config")],
            });
        }
        Err(error) => return Err(io_error(error)),
    };
    let Value::Object(raw) = raw else {
        return Ok(ConfigLoad {
            config: ScheduleConfig::default(),
            diagnostics: vec![diagnostic("schedules config must be a JSON object")],
        });
    };
    Ok(validate(raw))
}

/// Read one enabled entry from current configuration without changing scheduler state.
pub fn read_enabled_schedule_entry(
    path: &Path,
    name: &str,
) -> Result<(Option<ScheduleEntry>, Vec<ConfigDiagnostic>), ScheduleError> {
    let mut loaded = load_runtime(path)?;
    Ok((loaded.config.entries.remove(name), loaded.diagnostics))
}

/// Add the built-in schedule entries that are absent from the configuration,
/// creating the configuration first when it does not exist. Existing entries,
/// including disabled or edited built-ins, are left untouched. Returns the
/// names that were added.
pub fn register_default_entries(path: &Path) -> Result<Vec<String>, ScheduleError> {
    initialize_schedule_config(path)?;
    let defaults = BTreeMap::from(default_entries().map(|(name, value)| (name.to_owned(), value)));
    add_missing_schedule_entries(path, &defaults)
}

/// Seed timing metadata only when the schedule configuration is absent.
///
/// Existing schedule configurations remain byte-for-byte unchanged so initialization
/// never becomes a metadata migration.
pub fn initialize_schedule_config(path: &Path) -> Result<bool, ScheduleError> {
    let _lock = hold_lock(path, Default::default()).map_err(io_error)?;
    if path.exists() {
        return Ok(false);
    }
    let mut raw = Map::new();
    raw.insert(
        "daily_time".to_owned(),
        Value::String(DEFAULT_DAILY_TIME.to_owned()),
    );
    raw.insert(
        "weekly_time".to_owned(),
        Value::String(DEFAULT_WEEKLY_TIME.to_owned()),
    );
    write_raw(path, raw)?;
    Ok(true)
}

/// Add schedule entries that are absent from the raw schedules map.
///
/// The mutation holds the schedules lock for the complete read-modify-write
/// transaction. Existing entries, including disabled or malformed entries, are
/// deliberately left untouched.
pub fn add_missing_schedule_entries(
    path: &Path,
    entries: &BTreeMap<String, Value>,
) -> Result<Vec<String>, ScheduleError> {
    let _lock = hold_lock(path, Default::default()).map_err(io_error)?;
    let mut raw = read_strict_raw(path)?;
    let added = add_missing_entries(&mut raw, entries);
    if !added.is_empty() {
        write_raw(path, raw)?;
    }
    Ok(added)
}

fn add_missing_entries(
    raw: &mut Map<String, Value>,
    entries: &BTreeMap<String, Value>,
) -> Vec<String> {
    let missing = entries
        .iter()
        .filter(|(name, _)| !raw.contains_key(name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Vec<_>>();
    missing
        .into_iter()
        .map(|(name, value)| {
            raw.insert(name.clone(), value);
            name
        })
        .collect()
}

/// Remove one schedule entry, returning whether the raw map changed.
///
/// A missing entry is intentionally a no-op and does not rewrite the file.
pub fn remove_schedule_entry(path: &Path, name: &str) -> Result<bool, ScheduleError> {
    let _lock = hold_lock(path, Default::default()).map_err(io_error)?;
    let mut raw = read_strict_raw(path)?;
    let removed = raw.remove(name).is_some();
    if removed {
        write_raw(path, raw)?;
    }
    Ok(removed)
}

/// Mutate the raw schedules map while holding the schedule authority's stable
/// sidecar lock for the complete read-modify-write transaction.
///
/// Migration callers use this instead of opening `schedules.json` themselves.
/// Returning `changed: false` preserves an existing file byte-for-byte and does
/// not materialize a missing file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleMutation<T> {
    pub changed: bool,
    pub value: T,
}

pub fn mutate_schedule_entries<T, F>(path: &Path, mutator: F) -> Result<T, ScheduleError>
where
    F: FnOnce(&mut Map<String, Value>) -> ScheduleMutation<T>,
{
    let _lock = hold_lock(path, Default::default()).map_err(io_error)?;
    let mut raw = read_strict_raw(path)?;
    let mutation = mutator(&mut raw);
    if mutation.changed {
        write_raw(path, raw)?;
    }
    Ok(mutation.value)
}

/// Set reserved schedule metadata in one locked read-modify-write transaction.
///
/// This preserves `solstone/think/schedule_config.py:38-50`: supplied keys
/// replace raw metadata values and always mark the transaction changed.
pub fn set_schedule_metadata(
    path: &Path,
    updates: &Map<String, Value>,
) -> Result<(), ScheduleError> {
    let unknown = updates
        .keys()
        .filter(|key| !RESERVED_METADATA_KEYS.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(ScheduleError::UnknownMetadataKeys { keys: unknown });
    }
    mutate_schedule_entries(path, |raw| {
        raw.extend(updates.clone());
        ScheduleMutation {
            changed: true,
            value: (),
        }
    })
}

fn read_strict_raw(path: &Path) -> Result<Map<String, Value>, ScheduleError> {
    let raw = read_json::<Value>(path, Value::Object(Map::new()), MalformedPolicy::Raise).map_err(
        |error| match error {
            ReadError::Malformed(_) => ScheduleError::MalformedConfig {
                path: path.to_path_buf(),
            },
            error => io_error(error),
        },
    )?;
    match raw {
        Value::Object(raw) => Ok(raw),
        _ => Err(ScheduleError::MalformedConfig {
            path: path.to_path_buf(),
        }),
    }
}

fn write_raw(path: &Path, raw: Map<String, Value>) -> Result<(), ScheduleError> {
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(raw))
        .map_err(|error| ScheduleError::Io(error.to_string()))?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(io_error)
}

fn validate(raw: Map<String, Value>) -> ConfigLoad {
    let mut result = ConfigLoad {
        config: metadata_from_raw(&raw),
        ..ConfigLoad::default()
    };

    for (name, value) in raw {
        if RESERVED_METADATA_KEYS.contains(&name.as_str()) {
            continue;
        }
        let Value::Object(entry) = value else {
            result
                .diagnostics
                .push(diagnostic(&format!("schedule '{name}' must be an object")));
            continue;
        };
        if !entry.get("enabled").map(json_truthy).unwrap_or(true) {
            continue;
        }
        let Some(cmd) = entry.get("cmd").and_then(Value::as_array) else {
            result
                .diagnostics
                .push(diagnostic(&format!("schedule '{name}' has invalid cmd")));
            continue;
        };
        if cmd.is_empty() || cmd.iter().any(|value| !value.is_string()) {
            result
                .diagnostics
                .push(diagnostic(&format!("schedule '{name}' has invalid cmd")));
            continue;
        }
        let cmd = cmd
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let Some(every) = entry.get("every").and_then(Value::as_str) else {
            result.diagnostics.push(diagnostic(&format!(
                "schedule '{name}' has invalid interval"
            )));
            continue;
        };
        if !is_interval(every) {
            result.diagnostics.push(diagnostic(&format!(
                "schedule '{name}' has invalid interval"
            )));
            continue;
        }
        let max_runtime = entry.get("max_runtime").and_then(parse_duration);
        if entry.contains_key("max_runtime") && max_runtime.is_none() {
            result.diagnostics.push(diagnostic(&format!(
                "schedule '{name}' has invalid max_runtime"
            )));
        }
        result.config.entries.insert(
            name,
            ScheduleEntry {
                cmd,
                every: every.to_owned(),
                max_runtime,
            },
        );
    }
    result
}

pub(crate) fn metadata_from_raw(raw: &Map<String, Value>) -> ScheduleConfig {
    ScheduleConfig {
        entries: BTreeMap::new(),
        daily_time: raw
            .get("daily_time")
            .and_then(Value::as_str)
            .map(str::to_owned),
        weekly_time: raw
            .get("weekly_time")
            .and_then(Value::as_str)
            .map(str::to_owned),
        weekly_day: raw
            .get("weekly_day")
            .and_then(Value::as_str)
            .filter(|value| parse_weekly_day(value).is_some())
            .map(str::to_owned),
    }
}

pub(crate) fn parse_weekly_day(raw: &str) -> Option<u32> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "monday" | "mon" => Some(0),
        "tuesday" | "tue" => Some(1),
        "wednesday" | "wed" => Some(2),
        "thursday" | "thu" => Some(3),
        "friday" | "fri" => Some(4),
        "saturday" | "sat" => Some(5),
        "sunday" | "sun" => Some(6),
        _ => None,
    }
}

pub(crate) fn is_interval(every: &str) -> bool {
    matches!(every, "hourly" | "daily" | "weekly") || minute_interval(every).is_some()
}

pub(crate) fn minute_interval(every: &str) -> Option<u64> {
    every
        .strip_suffix('m')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

pub(crate) fn parse_duration(value: &Value) -> Option<Duration> {
    if let Some(seconds) = value.as_u64().filter(|seconds| *seconds > 0) {
        return Some(Duration::from_secs(seconds));
    }
    let raw = value.as_str()?;
    if !raw.is_ascii() {
        return None;
    }
    let (digits, unit) = raw.split_at(raw.len().checked_sub(1)?);
    let amount: u64 = digits.parse().ok()?;
    if amount == 0 {
        return None;
    }
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        _ => return None,
    };
    amount.checked_mul(multiplier).map(Duration::from_secs)
}

fn default_entries() -> [(&'static str, Value); 6] {
    [
        (
            "heartbeat",
            json!({"cmd": ["journal", "heartbeat"], "every": "daily", "enabled": true, "max_runtime": "10m"}),
        ),
        (
            "weekly-agents",
            json!({"cmd": ["journal", "think", "--weekly", "-v"], "every": "weekly", "enabled": true, "max_runtime": "30m"}),
        ),
        (
            "cadence",
            json!({"cmd": ["journal", "think", "--cadence"], "every": "5m", "enabled": true, "max_runtime": "10m"}),
        ),
        (
            "brain",
            json!({"cmd": ["journal", "brain", "refresh"], "every": "daily", "enabled": true, "max_runtime": "5m"}),
        ),
        (
            "facet-candidates",
            json!({"cmd": ["journal", "facet-candidates"], "every": "weekly", "enabled": true, "max_runtime": "10m"}),
        ),
        (
            "rebuild-edges",
            json!({"cmd": ["journal", "indexer", "--rebuild-edges"], "every": "weekly", "enabled": true, "max_runtime": "10m"}),
        ),
    ]
}

fn diagnostic(message: &str) -> ConfigDiagnostic {
    ConfigDiagnostic {
        message: message.to_owned(),
    }
}

pub(crate) fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn io_error(error: impl std::fmt::Display) -> ScheduleError {
    ScheduleError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_metadata_updates_only_reserved_values_through_the_locked_door() {
        // Derived from solstone/think/schedule_config.py:38-50; Python is not runnable here.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("schedules.json");
        std::fs::write(&path, r#"{"job":{"enabled":true},"daily_time":"08:00"}"#).unwrap();
        set_schedule_metadata(
            &path,
            &Map::from_iter([("daily_time".to_owned(), Value::String("09:30".to_owned()))]),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["daily_time"], "09:30");
        assert_eq!(value["job"]["enabled"], true);
    }
}
