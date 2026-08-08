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

const RESERVED_METADATA_KEYS: [&str; 3] = ["daily_time", "weekly_day", "weekly_time"];

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
pub struct ConfigLoad {
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

pub(crate) fn register_defaults(
    path: &Path,
    loaded: &ScheduleConfig,
) -> Result<bool, ScheduleError> {
    let needs = default_entries()
        .into_iter()
        .filter(|(name, _)| !loaded.entries.contains_key(*name))
        .collect::<Vec<_>>();
    if needs.is_empty() {
        return Ok(false);
    }

    let _lock = hold_lock(path, Default::default()).map_err(io_error)?;
    let mut raw = read_strict_raw(path)?;
    let mut changed = false;
    for (name, value) in needs {
        if !raw.contains_key(name) {
            raw.insert(name.to_owned(), value);
            changed = true;
        }
    }
    if changed {
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(raw))
            .map_err(|error| ScheduleError::Io(error.to_string()))?;
        bytes.push(b'\n');
        atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(io_error)?;
    }
    Ok(changed)
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

fn validate(raw: Map<String, Value>) -> ConfigLoad {
    let mut result = ConfigLoad::default();
    result.config.daily_time = raw
        .get("daily_time")
        .and_then(Value::as_str)
        .map(str::to_owned);
    result.config.weekly_time = raw
        .get("weekly_time")
        .and_then(Value::as_str)
        .map(str::to_owned);
    result.config.weekly_day = raw
        .get("weekly_day")
        .and_then(Value::as_str)
        .filter(|value| parse_weekly_day(value).is_some())
        .map(str::to_owned);

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

fn json_truthy(value: &Value) -> bool {
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
