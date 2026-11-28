// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, hold_lock};

use super::ScheduleError;

pub(crate) const RESERVED_METADATA_KEYS: [&str; 4] = [
    "daily_time",
    "maintenance_time",
    "weekly_day",
    "weekly_time",
];

pub(crate) const DEFAULT_MAINTENANCE_TIME: &str = "02:00";
const DEFAULT_WEEKLY_TIME: &str = "03:15";

/// A validated enabled schedule entry. `every` intentionally retains its raw form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleEntry {
    /// First registration instant of a shipped entry. Legacy and owner-authored entries may omit it.
    pub registered_at_ms: Option<i64>,
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
    use solstone_core_journal_io::durability::{
        ArtifactId, DurableRead, read_json_durable_validated,
    };
    match read_json_durable_validated::<Value>(ArtifactId::SchedulesConfig, path, |val| {
        if val.is_object() {
            Ok(())
        } else {
            Err("schedules config must be a JSON object".to_owned())
        }
    }) {
        Ok(DurableRead::Present(Value::Object(raw))) => Ok(validate(raw)),
        Ok(DurableRead::Present(_)) => unreachable!(),
        Ok(DurableRead::Absent) => Ok(ConfigLoad::default()),
        Ok(DurableRead::SetAside(_)) => Ok(ConfigLoad {
            config: ScheduleConfig::default(),
            diagnostics: vec![diagnostic("malformed schedules config")],
        }),
        Ok(DurableRead::Unreadable { .. }) => Ok(ConfigLoad {
            config: ScheduleConfig::default(),
            diagnostics: vec![diagnostic("unreadable schedules config")],
        }),
        Err(error) => Err(ScheduleError::Io(error.to_string())),
    }
}

/// Read one enabled entry from current configuration without changing scheduler state.
pub fn read_enabled_schedule_entry(
    path: &Path,
    name: &str,
) -> Result<(Option<ScheduleEntry>, Vec<ConfigDiagnostic>), ScheduleError> {
    let mut loaded = load_runtime(path)?;
    Ok((loaded.config.entries.remove(name), loaded.diagnostics))
}

/// Validate one proposed entry with the same rules used by runtime loading.
/// Disabled proposals are checked as enabled so invalid dormant work is refused.
pub fn validate_schedule_entry(value: &Value) -> Result<ScheduleEntry, Vec<ConfigDiagnostic>> {
    let mut candidate = value.clone();
    if let Some(entry) = candidate.as_object_mut() {
        entry.insert("enabled".into(), Value::Bool(true));
    }
    let mut loaded = validate(Map::from_iter([("proposed".into(), candidate)]));
    if !loaded.diagnostics.is_empty() {
        return Err(loaded.diagnostics);
    }
    loaded
        .config
        .entries
        .remove("proposed")
        .ok_or_else(|| vec![diagnostic("invalid schedule entry")])
}

/// Project one raw entry with the runtime's enabled, cadence and argv rules.
pub fn enabled_schedule_entry(value: &Value) -> Option<ScheduleEntry> {
    validate(Map::from_iter([("entry".to_owned(), value.clone())]))
        .config
        .entries
        .remove("entry")
}

/// Add the built-in schedule entries that are absent from the configuration,
/// creating the configuration first when it does not exist. Existing entries,
/// including disabled or edited built-ins, are left untouched. Returns the
/// names that were added.
pub fn register_default_entries(path: &Path) -> Result<Vec<String>, ScheduleError> {
    register_default_entries_at(path, jiff::Timestamp::now().as_millisecond())
}

/// Add only missing shipped entries, atomically publishing each with its first cadence boundary.
pub fn register_default_entries_at(path: &Path, now_ms: i64) -> Result<Vec<String>, ScheduleError> {
    initialize_schedule_config(path)?;
    let defaults = BTreeMap::from(default_entries().map(|(name, mut value)| {
        value["registered_at_ms"] = Value::from(now_ms);
        (name.to_owned(), value)
    }));
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
    use solstone_core_journal_io::durability::{
        ArtifactId, DurableRead, read_json_durable_validated,
    };
    match read_json_durable_validated::<Value>(ArtifactId::SchedulesConfig, path, |val| {
        if val.is_object() {
            Ok(())
        } else {
            Err("schedules config must be a JSON object".to_owned())
        }
    }) {
        Ok(DurableRead::Present(Value::Object(raw))) => Ok(raw),
        Ok(DurableRead::Present(_)) => unreachable!(),
        Ok(DurableRead::Absent) => Ok(Map::new()),
        Ok(DurableRead::SetAside(_)) | Ok(DurableRead::Unreadable { .. }) => {
            Err(ScheduleError::MalformedConfig {
                path: path.to_path_buf(),
            })
        }
        Err(error) => Err(ScheduleError::Io(error.to_string())),
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
                registered_at_ms: entry.get("registered_at_ms").and_then(Value::as_i64),
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
        daily_time: Some(effective_maintenance_time(raw).to_owned()),
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

// Older journals used daily_time for the model's inferred window. Only the
// dedicated owner setting can select a different time now.
fn effective_maintenance_time(raw: &Map<String, Value>) -> &str {
    raw.get("maintenance_time")
        .and_then(Value::as_str)
        .filter(|value| valid_maintenance_time(value))
        .unwrap_or(DEFAULT_MAINTENANCE_TIME)
}

pub fn valid_maintenance_time(value: &str) -> bool {
    matches!(
        value.as_bytes(),
        [b'0'..=b'1', b'0'..=b'9', b':', b'0'..=b'5', b'0'..=b'9']
            | [b'2', b'0'..=b'3', b':', b'0'..=b'5', b'0'..=b'9']
    )
}

pub fn maintenance_time(journal: &Path) -> Result<String, ScheduleError> {
    let path = journal.join("config/schedules.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DEFAULT_MAINTENANCE_TIME.to_owned());
        }
        Err(error) => return Err(io_error(error)),
    };
    let raw =
        serde_json::from_slice(&bytes).map_err(|_| ScheduleError::MalformedConfig { path })?;
    Ok(effective_maintenance_time(&raw).to_owned())
}

pub fn set_maintenance_time(journal: &Path, value: &str) -> Result<(), ScheduleError> {
    set_maintenance_time_at(
        journal,
        value,
        jiff::Timestamp::now().as_millisecond() as f64 / 1000.0,
    )
}

/// Persist `maintenance_time` and immediately rebase `start_after` in
/// `health/scheduler.json` for active daily schedules at `now_secs` when the
/// effective maintenance time changes, so a stopped journal or immediate
/// restart never treats the shifted window as a missed catch-up run.
pub fn set_maintenance_time_at(
    journal: &Path,
    value: &str,
    now_secs: f64,
) -> Result<(), ScheduleError> {
    if !valid_maintenance_time(value) {
        return Err(ScheduleError::InvalidMaintenanceTime);
    }
    let root = solstone_core_journal_io::JournalRoot::open(journal).map_err(io_error)?;
    let (time_changed, daily_names) = super::mutate_schedule_entries_at_root_checked(
        std::sync::Arc::new(root),
        &|| Ok(()),
        |raw| {
            let saved = Value::String(value.to_owned());
            let time_changed = effective_maintenance_time(raw) != value;
            let changed = raw.get("maintenance_time") != Some(&saved);
            raw.insert("maintenance_time".to_owned(), saved);
            let daily_names: std::collections::BTreeSet<String> = raw
                .iter()
                .filter(|(name, entry)| {
                    !RESERVED_METADATA_KEYS.contains(&name.as_str())
                        && entry.get("every").and_then(Value::as_str) == Some("daily")
                        && entry.get("enabled").map(json_truthy).unwrap_or(true)
                })
                .map(|(name, _)| name.clone())
                .collect();
            ScheduleMutation {
                changed,
                value: (time_changed, daily_names),
            }
        },
    )?;
    if time_changed && !daily_names.is_empty() {
        super::completion::rebase_cadence_for_edit(
            &journal.join("health/scheduler.json"),
            &daily_names,
            now_secs,
        )?;
    }
    Ok(())
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

/// The longest minute interval a schedule accepts: whole minutes of at most
/// `i64::MAX` milliseconds.
const MAX_INTERVAL_MINUTES: u64 = i64::MAX as u64 / 1_000 / 60;

pub(crate) fn minute_interval(every: &str) -> Option<u64> {
    every
        .strip_suffix('m')
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|digits| digits.parse::<u64>().ok())
        .filter(|minutes| *minutes <= MAX_INTERVAL_MINUTES)
}

pub(crate) fn parse_duration(value: &Value) -> Option<Duration> {
    if let Some(seconds) = value.as_u64().filter(|seconds| *seconds > 0) {
        let duration = Duration::from_secs(seconds);
        return std::time::Instant::now()
            .checked_add(duration)
            .map(|_| duration);
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
    amount
        .checked_mul(multiplier)
        .map(Duration::from_secs)
        .filter(|duration| std::time::Instant::now().checked_add(*duration).is_some())
}

fn default_entries() -> [(&'static str, Value); 7] {
    [
        (
            "search-catch-all",
            json!({"cmd": crate::partition::canonical_journal_command(["indexer", "--rescan"]), "every": "daily", "enabled": true, "max_runtime": "2h"}),
        ),
        (
            "heartbeat",
            json!({"cmd": crate::partition::canonical_journal_command(["heartbeat"]), "every": "daily", "enabled": true, "max_runtime": "10m"}),
        ),
        (
            "weekly-agents",
            json!({"cmd": crate::partition::canonical_journal_command(["think", "--weekly", "-v"]), "every": "weekly", "enabled": true, "max_runtime": "30m"}),
        ),
        (
            "cadence",
            json!({"cmd": crate::partition::canonical_journal_command(["think", "--cadence"]), "every": "5m", "enabled": true, "max_runtime": "10m"}),
        ),
        (
            "brain",
            json!({"cmd": crate::partition::canonical_journal_command(["brain", "refresh"]), "every": "daily", "enabled": true, "max_runtime": "5m"}),
        ),
        (
            "facet-candidates",
            json!({"cmd": crate::partition::canonical_journal_command(["facet-candidates"]), "every": "weekly", "enabled": true, "max_runtime": "10m"}),
        ),
        (
            "rebuild-edges",
            json!({"cmd": crate::partition::canonical_journal_command(["indexer", "--rebuild-edges"]), "every": "weekly", "enabled": true, "max_runtime": "10m"}),
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

pub fn configured_weekly_day_name(journal: &Path) -> String {
    let path = journal.join("config/schedules.json");
    let day_num = match load_runtime(&path) {
        Ok(config_load) => config_load
            .config
            .weekly_day
            .as_deref()
            .and_then(parse_weekly_day),
        Err(_) => None,
    };
    match day_num {
        Some(0) => "monday".to_string(),
        Some(1) => "tuesday".to_string(),
        Some(2) => "wednesday".to_string(),
        Some(3) => "thursday".to_string(),
        Some(4) => "friday".to_string(),
        Some(5) => "saturday".to_string(),
        _ => "sunday".to_string(),
    }
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

    #[test]
    fn configured_weekly_day_name_maps_spelling_and_defaults_to_sunday() {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("schedules.json");

        // missing file -> sunday
        assert_eq!(configured_weekly_day_name(root.path()), "sunday");

        // Mon -> monday
        std::fs::write(&config_path, r#"{"weekly_day":"Mon"}"#).unwrap();
        assert_eq!(configured_weekly_day_name(root.path()), "monday");

        // monday -> monday
        std::fs::write(&config_path, r#"{"weekly_day":"monday"}"#).unwrap();
        assert_eq!(configured_weekly_day_name(root.path()), "monday");

        // Funday -> sunday
        std::fs::write(&config_path, r#"{"weekly_day":"Funday"}"#).unwrap();
        assert_eq!(configured_weekly_day_name(root.path()), "sunday");
    }

    #[test]
    fn schedule_load_path_preserves_on_disk_bytes_without_migration() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("schedules.json");

        let fixture = serde_json::json!({
            "daily_time": "00:15",
            "weekly_time": "03:15",
            "heartbeat": {
                "cmd": ["journal", "heartbeat"],
                "every": "hourly"
            },
            "disabled_think": {
                "cmd": ["journal", "think", "--weekly", "-v"],
                "every": "weekly",
                "enabled": false
            },
            "edited_task": {
                "cmd": ["journal", "custom", "--not-a-writer-emitted-form"],
                "every": "30m"
            },
            "unknown_metadata_field": {
                "custom_prop": 42
            }
        });

        let mut bytes = serde_json::to_vec_pretty(&fixture).unwrap();
        bytes.push(b'\n');
        std::fs::write(&path, &bytes).unwrap();

        let loaded = load_runtime(&path).expect("load schedules");
        assert!(loaded.config.entries.contains_key("heartbeat"));
        assert!(loaded.config.entries.contains_key("edited_task"));
        assert!(!loaded.config.entries.contains_key("disabled_think"));

        let bytes_after = std::fs::read(&path).expect("read schedules after load");
        assert_eq!(
            bytes, bytes_after,
            "schedules.json bytes were mutated by load_runtime"
        );
    }
}
