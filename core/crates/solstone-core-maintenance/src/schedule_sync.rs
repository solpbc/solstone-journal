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
    "maintenance:timeline:rollup-day",
    "maintenance:timeline:rollup-master",
];
const LEGACY_TIMELINE_DAY: &str = "maintenance:timeline:rollup-day";
const LEGACY_TIMELINE_MASTER: &str = "maintenance:timeline:rollup-master";
const ORCHESTRATED_TIMELINE: &str = "maintenance:timeline:rollup";

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

/// Summary emitted by additive maintenance schedule synchronization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSummary {
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

/// Migrate retired timeline entries and add missing generated entries in one
/// locked whole-file transaction.
pub fn sync(path: &Path, routines: &[RoutineDescriptor]) -> Result<SyncSummary, String> {
    initialize_schedule_config(path).map_err(|error| error.to_string())?;
    mutate_schedule_entries(path, |raw| {
        let mut next = raw.clone();
        let result = plan_sync(&mut next, routines);
        match result {
            Ok(summary) => {
                let changed = next != *raw;
                if changed {
                    *raw = next;
                }
                ScheduleMutation {
                    changed,
                    value: Ok(summary),
                }
            }
            Err(error) => ScheduleMutation {
                changed: false,
                value: Err(error),
            },
        }
    })
    .map_err(|error| error.to_string())?
}

fn plan_sync(
    raw: &mut Map<String, Value>,
    routines: &[RoutineDescriptor],
) -> Result<SyncSummary, String> {
    let timeline_migrated = migrate_timeline_entries(raw, routines)?;
    for entry in RETIRED_ENTRIES {
        if *entry != LEGACY_TIMELINE_DAY && *entry != LEGACY_TIMELINE_MASTER {
            raw.remove(*entry);
        }
    }

    let classified = classify(routines, raw)
        .into_iter()
        .filter(|item| !is_retired_schedule_entry(item.descriptor.id));
    let mut summary = SyncSummary::default();
    for item in classified {
        if item.descriptor.id == "timeline:rollup" {
            match (timeline_migrated, item.status) {
                (TimelineMigration::Created, ScheduleStatus::Synced) => {
                    summary.added.push(item.descriptor.id.to_owned());
                    continue;
                }
                (TimelineMigration::Consolidated, ScheduleStatus::Synced) => {
                    summary.synced.push(item.descriptor.id.to_owned());
                    continue;
                }
                _ => {}
            }
        }
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
    Ok(summary)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimelineMigration {
    None,
    Created,
    Consolidated,
}

fn migrate_timeline_entries(
    raw: &mut Map<String, Value>,
    routines: &[RoutineDescriptor],
) -> Result<TimelineMigration, String> {
    let day = raw.get(LEGACY_TIMELINE_DAY).cloned();
    let master = raw.get(LEGACY_TIMELINE_MASTER).cloned();
    match (day, master) {
        (None, None) => Ok(TimelineMigration::None),
        (Some(_), None) | (None, Some(_)) => Err(
            "timeline schedule migration found only one legacy stage; preserved schedules unchanged"
                .to_owned(),
        ),
        (Some(day), Some(master)) => {
            let day_descriptor = descriptor(routines, "timeline:rollup-day")?;
            let master_descriptor = descriptor(routines, "timeline:rollup-master")?;
            let orchestrated_descriptor = descriptor(routines, "timeline:rollup")?;
            let (day_enabled, mut operator_fields) =
                legacy_entry(&day, day_descriptor, LEGACY_TIMELINE_DAY)?;
            let (master_enabled, master_fields) =
                legacy_entry(&master, master_descriptor, LEGACY_TIMELINE_MASTER)?;
            if day_enabled != master_enabled {
                return Err(
                    "timeline schedule migration found conflicting legacy enabled states; preserved schedules unchanged"
                        .to_owned(),
                );
            }
            merge_operator_fields(&mut operator_fields, master_fields)?;

            let had_orchestrated = raw.contains_key(ORCHESTRATED_TIMELINE);
            if let Some(existing) = raw.get(ORCHESTRATED_TIMELINE) {
                let (enabled, fields) =
                    legacy_entry(existing, orchestrated_descriptor, ORCHESTRATED_TIMELINE)?;
                if enabled != day_enabled {
                    return Err(
                        "timeline schedule migration found conflicting unified enabled state; preserved schedules unchanged"
                            .to_owned(),
                    );
                }
                merge_operator_fields(&mut operator_fields, fields)?;
            }

            let mut replacement = expected_entry(orchestrated_descriptor);
            replacement["enabled"] = Value::Bool(day_enabled);
            replacement
                .as_object_mut()
                .expect("expected schedule entry is an object")
                .extend(operator_fields);
            raw.remove(LEGACY_TIMELINE_DAY);
            raw.remove(LEGACY_TIMELINE_MASTER);
            raw.insert(ORCHESTRATED_TIMELINE.to_owned(), replacement);
            Ok(if had_orchestrated {
                TimelineMigration::Consolidated
            } else {
                TimelineMigration::Created
            })
        }
    }
}

fn descriptor<'a>(
    routines: &'a [RoutineDescriptor],
    id: &str,
) -> Result<&'a RoutineDescriptor, String> {
    routines
        .iter()
        .find(|descriptor| descriptor.id == id)
        .ok_or_else(|| format!("timeline schedule migration lacks descriptor {id}"))
}

fn legacy_entry(
    value: &Value,
    descriptor: &RoutineDescriptor,
    name: &str,
) -> Result<(bool, Map<String, Value>), String> {
    let Value::Object(actual) = value else {
        return Err(format!(
            "timeline schedule {name} is not an object; preserved schedules unchanged"
        ));
    };
    let Value::Object(expected) = expected_entry(descriptor) else {
        unreachable!("expected schedule entry is an object")
    };
    let enabled = actual.get("enabled").is_none_or(json_truthy);
    let has_schedule_fields = ["cmd", "every", "max_runtime"]
        .iter()
        .any(|field| actual.contains_key(*field));
    if enabled || has_schedule_fields {
        for field in ["cmd", "every", "max_runtime"] {
            if actual.get(field) != expected.get(field) {
                return Err(format!(
                    "timeline schedule {name} has a divergent {field}; preserved schedules unchanged"
                ));
            }
        }
    }
    let operator_fields = actual
        .iter()
        .filter(|(key, _)| !["cmd", "every", "enabled", "max_runtime"].contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Ok((enabled, operator_fields))
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

fn merge_operator_fields(
    target: &mut Map<String, Value>,
    incoming: Map<String, Value>,
) -> Result<(), String> {
    for (key, value) in incoming {
        if let Some(existing) = target.get(&key)
            && existing != &value
        {
            return Err(format!(
                "timeline schedule migration found conflicting operator field {key}; preserved schedules unchanged"
            ));
        }
        target.insert(key, value);
    }
    Ok(())
}

/// Render the reference schedule synchronization summary and warnings.
pub fn render_summary(summary: &SyncSummary) -> String {
    let mut output = String::new();
    for (name, ids) in [
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
    let mut command = vec!["journal", "maintenance", "run", descriptor.id];
    command.extend(descriptor.args);
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
    use super::{
        ScheduleStatus, classify_one, expected_entry, render_summary, schedule_name, sync,
    };
    use crate::registry::{Cadence, RoutineDescriptor, routines};
    use serde_json::{Value, json};
    use std::fs;

    const CAPPED: RoutineDescriptor = RoutineDescriptor {
        id: "app:capped",
        description: "capped",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
        args: &[],
    };
    const UNCAPPED: RoutineDescriptor = RoutineDescriptor {
        id: "app:uncapped",
        description: "uncapped",
        cadence: Cadence::Weekly,
        max_runtime: None,
        args: &[],
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

    #[test]
    fn sync_retires_independent_timeline_entries_and_adds_the_atomic_rollup() {
        let root = tempfile::tempdir().expect("temporary journal");
        let config = root.path().join("config/schedules.json");
        std::fs::create_dir_all(config.parent().expect("config directory"))
            .expect("config directory");
        let timeline_day = routines()
            .iter()
            .find(|descriptor| descriptor.id == "timeline:rollup-day")
            .expect("day descriptor");
        let timeline_master = routines()
            .iter()
            .find(|descriptor| descriptor.id == "timeline:rollup-master")
            .expect("master descriptor");
        std::fs::write(
            &config,
            json!({
                schedule_name(timeline_day.id): expected_entry(timeline_day),
                schedule_name(timeline_master.id): expected_entry(timeline_master),
            })
            .to_string(),
        )
        .expect("schedule config");

        let summary = sync(&config, routines()).expect("sync");
        assert!(summary.added.contains(&"timeline:rollup".to_owned()));
        assert!(!summary.added.contains(&"timeline:rollup-day".to_owned()));
        assert!(!summary.added.contains(&"timeline:rollup-master".to_owned()));
        let raw: Value = serde_json::from_slice(&std::fs::read(&config).expect("schedule config"))
            .expect("json");
        assert_eq!(
            raw["maintenance:timeline:rollup"]["cmd"],
            json!([
                "journal",
                "maintenance",
                "run",
                "timeline:rollup",
                "--commit"
            ])
        );
        assert!(raw.get("maintenance:timeline:rollup-day").is_none());
        assert!(raw.get("maintenance:timeline:rollup-master").is_none());
    }

    fn timeline_legacy(enabled: bool) -> (Value, Value) {
        let day = routines()
            .iter()
            .find(|descriptor| descriptor.id == "timeline:rollup-day")
            .expect("day descriptor");
        let master = routines()
            .iter()
            .find(|descriptor| descriptor.id == "timeline:rollup-master")
            .expect("master descriptor");
        let mut day_entry = expected_entry(day);
        let mut master_entry = expected_entry(master);
        day_entry["enabled"] = json!(enabled);
        master_entry["enabled"] = json!(enabled);
        (day_entry, master_entry)
    }

    fn write_config(path: &std::path::Path, value: &Value) {
        fs::create_dir_all(path.parent().expect("config parent")).expect("config parent");
        fs::write(path, format!("{value}\n")).expect("schedule config");
    }

    #[test]
    fn timeline_migration_preserves_disabled_intent_and_operator_fields() {
        let root = tempfile::tempdir().expect("temporary journal");
        let config = root.path().join("config/schedules.json");
        let (mut day, mut master) = timeline_legacy(false);
        day["operator_note"] = json!("keep");
        master["operator_note"] = json!("keep");
        master["ticket"] = json!(42);
        write_config(
            &config,
            &json!({
                "maintenance:timeline:rollup-day": day,
                "maintenance:timeline:rollup-master": master,
            }),
        );

        let summary = sync(&config, routines()).expect("sync");
        assert!(summary.disabled.contains(&"timeline:rollup".to_owned()));
        assert!(
            render_summary(&summary)
                .contains("timeline:rollup schedule is disabled; preserved disabled")
        );
        let raw: Value = serde_json::from_slice(&fs::read(&config).expect("config")).expect("json");
        assert_eq!(raw["maintenance:timeline:rollup"]["enabled"], false);
        assert_eq!(raw["maintenance:timeline:rollup"]["operator_note"], "keep");
        assert_eq!(raw["maintenance:timeline:rollup"]["ticket"], 42);
        assert!(raw.get("maintenance:timeline:rollup-day").is_none());
        assert!(raw.get("maintenance:timeline:rollup-master").is_none());

        let once = fs::read(&config).expect("first sync bytes");
        sync(&config, routines()).expect("second sync");
        assert_eq!(fs::read(&config).expect("second sync bytes"), once);
    }

    #[test]
    fn timeline_migration_accepts_runtime_enabled_default_and_minimal_disabled_entries() {
        for (enabled, minimal) in [(true, false), (false, true)] {
            let root = tempfile::tempdir().expect("temporary journal");
            let config = root.path().join("config/schedules.json");
            let (mut day, mut master) = timeline_legacy(enabled);
            if enabled {
                day.as_object_mut().expect("day object").remove("enabled");
                master
                    .as_object_mut()
                    .expect("master object")
                    .remove("enabled");
            } else if minimal {
                day = json!({"enabled": false, "operator_note": "keep"});
                master = json!({"enabled": false, "operator_note": "keep"});
            }
            write_config(
                &config,
                &json!({
                    "maintenance:timeline:rollup-day": day,
                    "maintenance:timeline:rollup-master": master,
                }),
            );

            let summary = sync(&config, routines()).expect("sync");
            let raw: Value =
                serde_json::from_slice(&fs::read(&config).expect("config")).expect("json");
            assert_eq!(
                raw["maintenance:timeline:rollup"]["enabled"],
                Value::Bool(enabled)
            );
            if enabled {
                assert!(summary.added.contains(&"timeline:rollup".to_owned()));
            } else {
                assert!(summary.disabled.contains(&"timeline:rollup".to_owned()));
                assert_eq!(raw["maintenance:timeline:rollup"]["operator_note"], "keep");
            }
        }
    }

    #[test]
    fn unsafe_timeline_migrations_fail_without_changing_bytes() {
        let cases = [
            {
                let (day, _) = timeline_legacy(true);
                json!({"maintenance:timeline:rollup-day": day})
            },
            {
                let (day, mut master) = timeline_legacy(true);
                master["enabled"] = json!(false);
                json!({
                    "maintenance:timeline:rollup-day": day,
                    "maintenance:timeline:rollup-master": master,
                })
            },
            {
                let (day, mut master) = timeline_legacy(true);
                master["cmd"] = json!(["custom", "timeline"]);
                json!({
                    "maintenance:timeline:rollup-day": day,
                    "maintenance:timeline:rollup-master": master,
                })
            },
            {
                let (mut day, mut master) = timeline_legacy(true);
                day["operator_note"] = json!("day");
                master["operator_note"] = json!("master");
                json!({
                    "maintenance:timeline:rollup-day": day,
                    "maintenance:timeline:rollup-master": master,
                })
            },
            {
                let (day, master) = timeline_legacy(true);
                let unified = routines()
                    .iter()
                    .find(|descriptor| descriptor.id == "timeline:rollup")
                    .expect("unified descriptor");
                let mut unified_entry = expected_entry(unified);
                unified_entry["cmd"] = json!(["custom", "timeline"]);
                json!({
                    "maintenance:timeline:rollup-day": day,
                    "maintenance:timeline:rollup-master": master,
                    "maintenance:timeline:rollup": unified_entry,
                })
            },
        ];
        for (index, value) in cases.into_iter().enumerate() {
            let root = tempfile::tempdir().expect("temporary journal");
            let config = root.path().join(format!("config/{index}/schedules.json"));
            write_config(&config, &value);
            let before = fs::read(&config).expect("before");
            let error = sync(&config, routines()).expect_err("unsafe migration must fail");
            assert!(error.contains("preserved schedules unchanged"), "{error}");
            assert_eq!(fs::read(&config).expect("after"), before);
        }
    }

    #[test]
    fn compatible_preexisting_unified_entry_is_merged_atomically() {
        let root = tempfile::tempdir().expect("temporary journal");
        let config = root.path().join("config/schedules.json");
        let (mut day, master) = timeline_legacy(true);
        day["legacy_note"] = json!("preserve");
        let unified = routines()
            .iter()
            .find(|descriptor| descriptor.id == "timeline:rollup")
            .expect("unified descriptor");
        let mut unified_entry = expected_entry(unified);
        unified_entry["owner"] = json!("operator");
        write_config(
            &config,
            &json!({
                "maintenance:timeline:rollup-day": day,
                "maintenance:timeline:rollup-master": master,
                "maintenance:timeline:rollup": unified_entry,
            }),
        );

        let summary = sync(&config, routines()).expect("sync");
        assert!(summary.synced.contains(&"timeline:rollup".to_owned()));
        assert!(!summary.added.contains(&"timeline:rollup".to_owned()));
        let raw: Value = serde_json::from_slice(&fs::read(&config).expect("config")).expect("json");
        assert_eq!(
            raw["maintenance:timeline:rollup"]["legacy_note"],
            "preserve"
        );
        assert_eq!(raw["maintenance:timeline:rollup"]["owner"], "operator");
    }

    #[cfg(unix)]
    #[test]
    fn timeline_migration_publication_failure_keeps_the_legacy_pair() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary journal");
        let config = root.path().join("config/schedules.json");
        let (day, master) = timeline_legacy(true);
        write_config(
            &config,
            &json!({
                "maintenance:timeline:rollup-day": day,
                "maintenance:timeline:rollup-master": master,
            }),
        );
        let before = fs::read(&config).expect("before");
        let parent = config.parent().expect("config parent");
        fs::write(parent.join(".schedules.json.lock"), b"").expect("persistent lock sidecar");
        fs::set_permissions(parent, fs::Permissions::from_mode(0o500))
            .expect("make publication fail");

        let result = sync(&config, routines());
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .expect("restore config directory");
        assert!(result.is_err(), "publication failure must fail sync");
        assert_eq!(fs::read(&config).expect("after"), before);
    }
}
