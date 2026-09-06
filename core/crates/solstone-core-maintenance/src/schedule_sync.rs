// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only maintenance schedule status and additive schedule synchronization.

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{MalformedPolicy, read_json};
use solstone_core_system::schedule::{
    ScheduleMutation, initialize_schedule_config, mutate_schedule_entries,
};

use crate::registry::RoutineDescriptor;

const MAINTENANCE_PREFIX: &str = "maintenance:";
const RETIRED_ENTRIES: &[&str] = &[
    "maintenance:health:release-raw",
    "maintenance:timeline:rollup",
    "maintenance:timeline:rollup-day",
    "maintenance:timeline:rollup-master",
];

/// A routine's relationship to its generated schedule entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStatus {
    Missing,
    Synced,
    Divergent,
    Disabled,
}

impl ScheduleStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Synced => "synced",
            Self::Divergent => "divergent",
            Self::Disabled => "disabled",
        }
    }
}

/// The list/sync status for one registered routine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutineStatus<'a> {
    pub descriptor: &'a RoutineDescriptor,
    pub status: ScheduleStatus,
}

/// Summary emitted by maintenance schedule synchronization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub removed: Vec<String>,
    pub added: Vec<String>,
    pub synced: Vec<String>,
    pub divergent: Vec<String>,
    pub disabled: Vec<String>,
}

/// The journal-local schedules configuration path.
pub fn schedules_path(journal: &Path) -> std::path::PathBuf {
    journal.join("config/schedules.json")
}

/// Read and classify every routine against the raw schedule map.
pub fn statuses<'a>(
    path: &Path,
    routines: &'a [RoutineDescriptor],
) -> Result<Vec<RoutineStatus<'a>>, String> {
    let raw = read_raw_schedules(path)?;
    Ok(classify(routines, &raw))
}

/// Render the reference maintenance routine list.
pub fn render_list(path: &Path, routines: &[RoutineDescriptor]) -> Result<String, String> {
    if routines.is_empty() {
        return Ok("No maintenance routines found.\n".to_owned());
    }
    let statuses = statuses(path, routines)?;
    let id_width = routines
        .iter()
        .map(|descriptor| descriptor.id.len())
        .max()
        .unwrap_or(2)
        .max(2);
    let mut output = format!(
        "  {:<id_width$}  {:<8}  {:<9}  {:<11}  DESCRIPTION\n",
        "ID", "EVERY", "STATUS", "MAX RUNTIME"
    );
    for item in statuses {
        let descriptor = item.descriptor;
        output.push_str(&format!(
            "  {:<id_width$}  {:<8}  {:<9}  {:<11}  {}\n",
            descriptor.id,
            descriptor.cadence.as_str(),
            item.status.as_str(),
            descriptor.max_runtime.unwrap_or("-"),
            descriptor.description,
        ));
    }
    Ok(output)
}

/// Prune retired schedule entries and add missing generated entries in one
/// locked whole-file transaction.
pub fn sync(path: &Path, routines: &[RoutineDescriptor]) -> Result<SyncSummary, String> {
    initialize_schedule_config(path).map_err(|error| error.to_string())?;
    mutate_schedule_entries(path, |raw| {
        let mut next = raw.clone();
        let summary = plan_sync(&mut next, routines);
        let changed = next != *raw;
        if changed {
            *raw = next;
        }
        ScheduleMutation {
            changed,
            value: summary,
        }
    })
    .map_err(|error| error.to_string())
}

fn plan_sync(raw: &mut Map<String, Value>, routines: &[RoutineDescriptor]) -> SyncSummary {
    let mut summary = SyncSummary::default();
    for entry in RETIRED_ENTRIES {
        if raw.remove(*entry).is_some() {
            summary.removed.push((*entry).to_owned());
        }
    }

    let classified = classify(routines, raw)
        .into_iter()
        .filter(|item| !is_retired_schedule_entry(item.descriptor.id));
    for item in classified {
        match item.status {
            ScheduleStatus::Missing => {
                summary.added.push(item.descriptor.id.to_owned());
                raw.insert(
                    schedule_name(item.descriptor.id),
                    expected_entry(item.descriptor),
                );
            }
            ScheduleStatus::Synced => summary.synced.push(item.descriptor.id.to_owned()),
            ScheduleStatus::Divergent => summary.divergent.push(item.descriptor.id.to_owned()),
            ScheduleStatus::Disabled => summary.disabled.push(item.descriptor.id.to_owned()),
        }
    }
    summary
}

/// Render the reference schedule synchronization summary and warnings.
pub fn render_summary(summary: &SyncSummary) -> String {
    let mut output = String::new();
    for (name, ids) in [
        ("removed", &summary.removed),
        ("added", &summary.added),
        ("synced", &summary.synced),
        ("divergent", &summary.divergent),
        ("disabled", &summary.disabled),
    ] {
        let suffix = if ids.is_empty() {
            String::new()
        } else {
            format!(": {}", ids.join(", "))
        };
        output.push_str(&format!("{name}: {}{suffix}\n", ids.len()));
    }
    for id in &summary.divergent {
        output.push_str(&format!(
            "WARNING: {id} schedule is divergent; preserved unchanged\n"
        ));
    }
    for id in &summary.disabled {
        output.push_str(&format!(
            "WARNING: {id} schedule is disabled; preserved disabled\n"
        ));
    }
    output
}

/// The generated config entry name for a routine identifier.
pub fn schedule_name(id: &str) -> String {
    format!("{MAINTENANCE_PREFIX}{id}")
}

/// Build the generated raw config entry for one routine.
pub fn expected_entry(descriptor: &RoutineDescriptor) -> Value {
    let command = vec!["journal", "maintenance", "run", descriptor.id];
    let mut entry = Map::from_iter([
        ("cmd".to_owned(), json!(command)),
        (
            "every".to_owned(),
            Value::String(descriptor.cadence.as_str().to_owned()),
        ),
        ("enabled".to_owned(), Value::Bool(true)),
    ]);
    if let Some(max_runtime) = descriptor.max_runtime {
        entry.insert(
            "max_runtime".to_owned(),
            Value::String(max_runtime.to_owned()),
        );
    }
    Value::Object(entry)
}

fn is_retired_schedule_entry(id: &str) -> bool {
    let name = schedule_name(id);
    RETIRED_ENTRIES.contains(&name.as_str())
}

fn read_raw_schedules(path: &Path) -> Result<Map<String, Value>, String> {
    let raw = read_json(path, Value::Object(Map::new()), MalformedPolicy::Raise)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    raw.as_object()
        .cloned()
        .ok_or_else(|| format!("{}: schedules config must be a JSON object", path.display()))
}

fn classify<'a>(
    routines: &'a [RoutineDescriptor],
    raw: &Map<String, Value>,
) -> Vec<RoutineStatus<'a>> {
    let mut sorted = routines.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|descriptor| descriptor.id);
    sorted
        .into_iter()
        .map(|descriptor| RoutineStatus {
            descriptor,
            status: classify_one(descriptor, raw.get(&schedule_name(descriptor.id))),
        })
        .collect()
}

fn classify_one(descriptor: &RoutineDescriptor, raw: Option<&Value>) -> ScheduleStatus {
    let Some(Value::Object(entry)) = raw else {
        return if raw.is_none() {
            ScheduleStatus::Missing
        } else {
            ScheduleStatus::Divergent
        };
    };
    if entry.get("enabled") == Some(&Value::Bool(false)) {
        return ScheduleStatus::Disabled;
    }
    let expected = expected_entry(descriptor);
    let Value::Object(expected) = expected else {
        unreachable!("expected entry is an object")
    };
    if entry.get("cmd") != expected.get("cmd") || entry.get("every") != expected.get("every") {
        return ScheduleStatus::Divergent;
    }
    match descriptor.max_runtime {
        Some(max_runtime)
            if entry.get("max_runtime") == Some(&Value::String(max_runtime.to_owned())) =>
        {
            ScheduleStatus::Synced
        }
        Some(_) => ScheduleStatus::Divergent,
        None if matches!(entry.get("max_runtime"), None | Some(Value::Null)) => {
            ScheduleStatus::Synced
        }
        None => ScheduleStatus::Divergent,
    }
}

#[cfg(test)]
mod tests {
    use super::{ScheduleStatus, classify_one, expected_entry, sync};
    use crate::registry::{Cadence, RoutineDescriptor, routines};
    use serde_json::{Value, json};
    use std::fs;

    const CAPPED: RoutineDescriptor = RoutineDescriptor {
        id: "app:capped",
        description: "capped",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
    };
    const UNCAPPED: RoutineDescriptor = RoutineDescriptor {
        id: "app:uncapped",
        description: "uncapped",
        cadence: Cadence::Weekly,
        max_runtime: None,
    };

    #[test]
    fn sync_initializes_staggered_metadata_for_a_missing_schedule_config() {
        let root = tempfile::tempdir().expect("temporary journal");
        let config = root.path().join("config/schedules.json");
        std::fs::create_dir_all(config.parent().expect("config directory"))
            .expect("config directory");

        let summary = sync(&config, &[CAPPED, UNCAPPED]).expect("sync");
        assert_eq!(summary.added, vec![CAPPED.id, UNCAPPED.id]);
        let raw: Value =
            serde_json::from_slice(&std::fs::read(config).expect("schedule config")).expect("json");
        assert_eq!(raw["daily_time"], "00:15");
        assert_eq!(raw["weekly_time"], "03:15");
    }

    #[test]
    fn status_classification_preserves_operator_owned_fields_and_special_cases() {
        assert_eq!(classify_one(&CAPPED, None), ScheduleStatus::Missing);
        assert_eq!(
            classify_one(&CAPPED, Some(&json!(false))),
            ScheduleStatus::Divergent
        );
        assert_eq!(
            classify_one(&CAPPED, Some(&json!({"enabled": false}))),
            ScheduleStatus::Disabled
        );
        for disabled_like in [json!(null), json!(0), json!(""), json!([])] {
            let mut entry = expected_entry(&CAPPED);
            entry["enabled"] = disabled_like;
            assert_eq!(classify_one(&CAPPED, Some(&entry)), ScheduleStatus::Synced);
        }
        let mut extra = expected_entry(&CAPPED);
        extra["operator_note"] = json!("keep");
        assert_eq!(classify_one(&CAPPED, Some(&extra)), ScheduleStatus::Synced);
        let mut cap_missing = expected_entry(&CAPPED);
        cap_missing
            .as_object_mut()
            .expect("object")
            .remove("max_runtime");
        assert_eq!(
            classify_one(&CAPPED, Some(&cap_missing)),
            ScheduleStatus::Divergent
        );
        let uncapped = expected_entry(&UNCAPPED);
        assert_eq!(
            classify_one(&UNCAPPED, Some(&uncapped)),
            ScheduleStatus::Synced
        );
        let mut null_cap = expected_entry(&UNCAPPED);
        null_cap["max_runtime"] = json!(null);
        assert_eq!(
            classify_one(&UNCAPPED, Some(&null_cap)),
            ScheduleStatus::Synced
        );
    }

    fn write_config(path: &std::path::Path, value: &Value) {
        fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
        fs::write(path, format!("{value}\n")).expect("schedule config");
    }

    #[test]
    fn sync_prunes_retired_timeline_entries_and_preserves_operator_added_entries() {
        let root = tempfile::tempdir().expect("temporary journal");
        let config = root.path().join("config/schedules.json");
        write_config(
            &config,
            &json!({
                "maintenance:timeline:rollup": {
                    "cmd": ["journal", "maintenance", "run", "timeline:rollup", "--commit"],
                    "every": "daily",
                    "enabled": true,
                },
                "maintenance:timeline:rollup-day": {
                    "cmd": ["journal", "maintenance", "run", "timeline:rollup-day"],
                    "every": "daily",
                    "enabled": true,
                },
                "maintenance:timeline:rollup-master": {
                    "cmd": ["journal", "maintenance", "run", "timeline:rollup-master"],
                    "every": "daily",
                    "enabled": true,
                },
                "maintenance:operator:custom": {
                    "cmd": ["operator", "custom"],
                    "every": "daily",
                    "enabled": true,
                },
            }),
        );

        let summary = sync(&config, routines()).expect("sync");
        assert_eq!(
            summary.removed,
            vec![
                "maintenance:timeline:rollup".to_owned(),
                "maintenance:timeline:rollup-day".to_owned(),
                "maintenance:timeline:rollup-master".to_owned(),
            ]
        );
        assert!(
            !summary
                .added
                .iter()
                .chain(&summary.synced)
                .any(|id| id.starts_with("timeline:"))
        );
        let raw: Value = serde_json::from_slice(&fs::read(&config).expect("config")).expect("json");
        assert!(raw.get("maintenance:timeline:rollup").is_none());
        assert!(raw.get("maintenance:timeline:rollup-day").is_none());
        assert!(raw.get("maintenance:timeline:rollup-master").is_none());
        assert_eq!(
            raw["maintenance:operator:custom"]["cmd"],
            json!(["operator", "custom"])
        );
    }
}
