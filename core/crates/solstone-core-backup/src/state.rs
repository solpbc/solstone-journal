// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};

use crate::{
    BackupError, BackupKeys, Destination, HostedBinding, OFFLOAD_STATUSES, RESTORE_SCOPES,
    RESTORE_STATUSES, VERIFICATION_STATUSES, backup_defaults, format_recovery_key_display,
    generate_daily_key, generate_recovery_key, load_hosted_binding, merge_backup_config,
};

const RETENTION_KEYS: [&str; 4] = ["hourly", "daily", "weekly", "monthly"];
const OFFLOAD_KEYS: [&str; 3] = ["enabled", "budget_bytes", "floor_bytes"];

pub fn get_backup_config(journal: &Path) -> Result<Map<String, Value>, BackupError> {
    let read = read_journal_config(journal).map_err(BackupError::ConfigLoad)?;
    Ok(merge_backup_config(&read.config.unwrap_or_default()))
}

pub fn get_destination(journal: &Path) -> Result<Option<Destination>, BackupError> {
    let destination = get_backup_config(journal)?
        .remove("destination")
        .and_then(|value| value.as_object().cloned());
    let Some(destination) = destination else {
        return Ok(None);
    };
    let (Some(repository), Some(backend), Some(credentials)) = (
        destination.get("repository").and_then(Value::as_str),
        destination.get("backend").and_then(Value::as_str),
        destination.get("credentials").and_then(Value::as_object),
    ) else {
        return Ok(None);
    };
    Ok(Some(Destination {
        repository: repository.to_owned(),
        backend: backend.to_owned(),
        credentials: credentials.clone(),
    }))
}

pub fn get_keys(journal: &Path) -> Result<Option<BackupKeys>, BackupError> {
    let config = get_backup_config(journal)?;
    build_keys(config.get("daily_key"), config.get("recovery_key"))
}

pub fn generate_and_store_keys(journal: &Path) -> Result<BackupKeys, BackupError> {
    let generated_daily = generate_daily_key()?;
    let generated_recovery = generate_recovery_key()?;
    mutate_backup_section(journal, move |backup| {
        let daily = backup.get("daily_key").cloned().unwrap_or(Value::Null);
        let recovery = backup.get("recovery_key").cloned().unwrap_or(Value::Null);
        let mut changed = false;
        let daily = if daily.is_null() {
            changed = true;
            Value::String(generated_daily)
        } else {
            daily
        };
        let recovery = if recovery.is_null() {
            changed = true;
            Value::String(generated_recovery)
        } else {
            recovery
        };
        let keys = build_keys(Some(&daily), Some(&recovery))?.expect("non-null keys are present");
        backup.insert("daily_key".to_owned(), daily);
        backup.insert("recovery_key".to_owned(), recovery);
        Ok((changed, keys))
    })
}

pub fn set_destination(journal: &Path, destination: &Destination) -> Result<(), BackupError> {
    let value = Map::from_iter([
        (
            "repository".to_owned(),
            Value::String(destination.repository.clone()),
        ),
        (
            "backend".to_owned(),
            Value::String(destination.backend.clone()),
        ),
        (
            "credentials".to_owned(),
            Value::Object(destination.credentials.clone()),
        ),
    ]);
    mutate_backup_section(journal, |backup| {
        set_value(backup, "destination", Value::Object(value))
    })
}

pub fn set_enabled(journal: &Path, enabled: bool) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(backup, "enabled", Value::Bool(enabled))
    })
}

pub fn set_mode(journal: &Path, mode: &str) -> Result<(), BackupError> {
    if !matches!(mode, "byo" | "operated") {
        return Err(BackupError::InvalidMode);
    }
    mutate_backup_section(journal, |backup| {
        set_value(backup, "mode", Value::String(mode.to_owned()))
    })
}

pub fn set_retention(journal: &Path, retention: &Map<String, Value>) -> Result<(), BackupError> {
    if retention.len() != RETENTION_KEYS.len()
        || !RETENTION_KEYS
            .iter()
            .all(|key| retention.contains_key(*key))
    {
        return Err(BackupError::InvalidRetentionShape);
    }
    if RETENTION_KEYS
        .iter()
        .any(|key| !is_nonnegative_integer(retention.get(*key)))
    {
        return Err(BackupError::InvalidRetentionValue);
    }
    mutate_backup_section(journal, |backup| {
        set_value(backup, "retention", Value::Object(retention.clone()))
    })
}

pub fn set_offload(journal: &Path, offload: &Map<String, Value>) -> Result<(), BackupError> {
    if offload.len() != OFFLOAD_KEYS.len()
        || !OFFLOAD_KEYS.iter().all(|key| offload.contains_key(*key))
    {
        return Err(BackupError::InvalidOffloadShape);
    }
    if !matches!(offload.get("enabled"), Some(Value::Bool(_))) {
        return Err(BackupError::InvalidOffloadEnabled);
    }
    if ["budget_bytes", "floor_bytes"]
        .iter()
        .any(|key| !is_positive_or_null(offload.get(*key)))
    {
        return Err(BackupError::InvalidOffloadBytes);
    }
    mutate_backup_section(journal, |backup| {
        set_value(backup, "offload", Value::Object(offload.clone()))
    })
}

pub fn set_recovery_key(journal: &Path, recovery_key: &str) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(
            backup,
            "recovery_key",
            Value::String(recovery_key.to_owned()),
        )
    })
}

pub fn set_recovery_key_confirmed(journal: &Path, confirmed: bool) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(backup, "confirmed_recovery_key", Value::Bool(confirmed))
    })
}

pub fn clear_backup_config(journal: &Path) -> Result<(), BackupError> {
    let defaults = backup_defaults();
    mutate_backup_section(journal, |backup| {
        let changed = *backup != defaults;
        *backup = defaults;
        Ok((changed, ()))
    })
}

pub fn record_backup_result(
    journal: &Path,
    status: &str,
    time: Value,
    snapshot_id: Value,
    error_reason: Value,
) -> Result<(), BackupError> {
    record(
        journal,
        "last_backup",
        Map::from_iter([
            ("time".into(), time),
            ("snapshot_id".into(), snapshot_id),
            ("status".into(), Value::String(status.into())),
            ("error_reason".into(), error_reason),
        ]),
    )
}

pub fn record_prune_result(
    journal: &Path,
    status: &str,
    time: Value,
    error_reason: Value,
) -> Result<(), BackupError> {
    record(
        journal,
        "last_prune",
        Map::from_iter([
            ("time".into(), time),
            ("status".into(), Value::String(status.into())),
            ("error_reason".into(), error_reason),
        ]),
    )
}

pub fn record_offload_result(
    journal: &Path,
    status: &str,
    time: Value,
    reason: Value,
    files_marked: Value,
    bytes_marked: Value,
    ran_out_of_markable_media: Value,
) -> Result<(), BackupError> {
    if !OFFLOAD_STATUSES.contains(&status) {
        return Err(BackupError::InvalidOffloadStatus);
    }
    mutate_backup_section(journal, |backup| {
        let prior = backup.get("last_offload").and_then(Value::as_object);
        let last_ok = if status == "ok" {
            time.clone()
        } else {
            prior
                .and_then(|value| value.get("last_ok_time"))
                .cloned()
                .unwrap_or(Value::Null)
        };
        set_value(
            backup,
            "last_offload",
            Value::Object(Map::from_iter([
                ("time".into(), time),
                ("status".into(), Value::String(status.into())),
                ("reason".into(), reason),
                ("last_ok_time".into(), last_ok),
                ("files_marked".into(), files_marked),
                ("bytes_marked".into(), bytes_marked),
                (
                    "ran_out_of_markable_media".into(),
                    ran_out_of_markable_media,
                ),
            ])),
        )
    })
}

pub fn record_verification_result(
    journal: &Path,
    status: &str,
    time: Value,
    reason: Value,
    checked_subset: Value,
) -> Result<(), BackupError> {
    if !VERIFICATION_STATUSES.contains(&status) {
        return Err(BackupError::InvalidVerificationStatus);
    }
    mutate_backup_section(journal, |backup| {
        let prior = backup.get("last_verification").and_then(Value::as_object);
        let last_ok = if status == "ok" {
            time.clone()
        } else {
            prior
                .and_then(|value| value.get("last_ok_time"))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let checked = if status == "ok" {
            checked_subset
        } else {
            Value::Null
        };
        set_value(
            backup,
            "last_verification",
            Value::Object(Map::from_iter([
                ("time".into(), time),
                ("status".into(), Value::String(status.into())),
                ("reason".into(), reason),
                ("last_ok_time".into(), last_ok),
                ("checked_subset".into(), checked),
            ])),
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub fn record_restore_result(
    journal: &Path,
    status: &str,
    time: Value,
    reason: Value,
    scope: &str,
    day: Value,
    segments_selected: Value,
    segments_restored: Value,
    files_expected: Value,
    files_restored: Value,
    bytes_expected: Value,
    bytes_restored: Value,
) -> Result<(), BackupError> {
    if !RESTORE_STATUSES.contains(&status) {
        return Err(BackupError::InvalidRestoreStatus);
    }
    if !RESTORE_SCOPES.contains(&scope) {
        return Err(BackupError::InvalidRestoreScope);
    }
    if [&segments_selected, &segments_restored]
        .iter()
        .any(|value| !is_nonnegative_integer(Some(value)))
        || [
            &files_expected,
            &files_restored,
            &bytes_expected,
            &bytes_restored,
        ]
        .iter()
        .any(|value| !is_nonnegative_integer_or_null(Some(value)))
    {
        return Err(BackupError::InvalidRestoreCounters);
    }
    record(
        journal,
        "last_restore",
        Map::from_iter([
            ("time".into(), time),
            ("status".into(), Value::String(status.into())),
            ("reason".into(), reason),
            ("scope".into(), Value::String(scope.into())),
            ("day".into(), day),
            ("segments_selected".into(), segments_selected),
            ("segments_restored".into(), segments_restored),
            ("files_expected".into(), files_expected),
            ("files_restored".into(), files_restored),
            ("bytes_expected".into(), bytes_expected),
            ("bytes_restored".into(), bytes_restored),
        ]),
    )
}

pub fn status_view(journal: &Path) -> Result<Map<String, Value>, BackupError> {
    let config = get_backup_config(journal)?;
    let destination = config
        .get("destination")
        .and_then(Value::as_object)
        .ok_or(BackupError::InvalidDestinationShape)?;
    let hosted = match load_hosted_binding(journal) {
        Some(HostedBinding { bucket, prefix, .. }) => Map::from_iter([
            ("bound".into(), Value::Bool(true)),
            ("bucket".into(), Value::String(bucket)),
            ("prefix".into(), Value::String(prefix)),
        ]),
        None => Map::from_iter([("bound".into(), Value::Bool(false))]),
    };
    Ok(Map::from_iter([
        ("enabled".into(), config["enabled"].clone()),
        ("mode".into(), config["mode"].clone()),
        (
            "destination".into(),
            Value::Object(Map::from_iter([
                (
                    "repository".into(),
                    destination
                        .get("repository")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "backend".into(),
                    destination.get("backend").cloned().unwrap_or(Value::Null),
                ),
                (
                    "credentials_set".into(),
                    Value::Bool(destination.get("credentials").is_some_and(value_truthy)),
                ),
            ])),
        ),
        (
            "daily_key_set".into(),
            Value::Bool(!config["daily_key"].is_null()),
        ),
        (
            "recovery_key_set".into(),
            Value::Bool(!config["recovery_key"].is_null()),
        ),
        (
            "recovery_key_confirmed".into(),
            Value::Bool(config["confirmed_recovery_key"].as_bool().unwrap_or(false)),
        ),
        ("retention".into(), config["retention"].clone()),
        ("offload".into(), config["offload"].clone()),
        ("schedule".into(), config["schedule"].clone()),
        ("last_backup".into(), config["last_backup"].clone()),
        ("last_prune".into(), config["last_prune"].clone()),
        ("last_offload".into(), config["last_offload"].clone()),
        (
            "last_verification".into(),
            config["last_verification"].clone(),
        ),
        ("last_restore".into(), config["last_restore"].clone()),
        ("hosted".into(), Value::Object(hosted)),
    ]))
}

fn build_keys(
    daily: Option<&Value>,
    recovery: Option<&Value>,
) -> Result<Option<BackupKeys>, BackupError> {
    match (daily, recovery) {
        (Some(Value::Null) | None, _) | (_, Some(Value::Null) | None) => Ok(None),
        (Some(Value::String(daily_key)), Some(Value::String(recovery_key))) => {
            format_recovery_key_display(recovery_key)?;
            Ok(Some(BackupKeys {
                daily_key: daily_key.clone(),
                recovery_key: recovery_key.clone(),
            }))
        }
        _ => Err(BackupError::StoredKeys),
    }
}

fn record(journal: &Path, key: &str, value: Map<String, Value>) -> Result<(), BackupError> {
    mutate_backup_section(journal, |backup| {
        set_value(backup, key, Value::Object(value))
    })
}
fn set_value(
    backup: &mut Map<String, Value>,
    key: &str,
    value: Value,
) -> Result<(bool, ()), BackupError> {
    let changed = backup.get(key) != Some(&value);
    backup.insert(key.to_owned(), value);
    Ok((changed, ()))
}
fn is_nonnegative_integer(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_i64)
        .is_some_and(|value| value >= 0)
}
fn is_nonnegative_integer_or_null(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Null)) || is_nonnegative_integer(value)
}
fn is_positive_or_null(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Null))
        || value.and_then(Value::as_i64).is_some_and(|value| value > 0)
}

fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64() != Some(0) && value.as_u64() != Some(0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn mutate_backup_section<T>(
    journal: &Path,
    operation: impl FnOnce(&mut Map<String, Value>) -> Result<(bool, T), BackupError>,
) -> Result<T, BackupError> {
    let transaction = mutate_journal_config(journal, mutation_lock_options(), |config| {
        run_mutation_hook();
        let backup = config
            .entry("backup".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        let replaced_non_object = !backup.is_object();
        if replaced_non_object {
            *backup = Value::Object(Map::new());
        }
        let result = operation(backup.as_object_mut().expect("object assigned"));
        match result {
            Ok((changed, value)) => JournalConfigMutation {
                changed: changed || replaced_non_object,
                value: Ok(value),
            },
            Err(error) => JournalConfigMutation {
                changed: false,
                value: Err(error),
            },
        }
    })
    .map_err(BackupError::ConfigMutation)?;
    transaction.value
}

#[cfg(test)]
use std::{cell::RefCell, rc::Rc};
#[cfg(test)]
thread_local! { static MUTATION_HOOK: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) }; }
#[cfg(test)]
pub(crate) struct MutationHookGuard(Option<Rc<dyn Fn()>>);
#[cfg(test)]
impl Drop for MutationHookGuard {
    fn drop(&mut self) {
        MUTATION_HOOK.with(|hook| *hook.borrow_mut() = self.0.take());
    }
}
#[cfg(test)]
pub(crate) fn install_mutation_hook(hook: Rc<dyn Fn()>) -> MutationHookGuard {
    MutationHookGuard(MUTATION_HOOK.with(|current| current.replace(Some(hook))))
}
#[cfg(test)]
fn run_mutation_hook() {
    MUTATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook();
        }
    });
}
#[cfg(not(test))]
fn run_mutation_hook() {}
fn mutation_lock_options() -> LockOptions {
    LockOptions::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use std::thread;

    fn journal() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn write_config(root: &Path, value: Value) {
        let path = root.join("config/journal.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn config_path(root: &Path) -> std::path::PathBuf {
        root.join("config/journal.json")
    }

    fn assert_rejection_preserves_config(
        journal: &tempfile::TempDir,
        call: impl FnOnce() -> Result<(), BackupError>,
    ) {
        let before = fs::read(config_path(journal.path())).unwrap();
        assert!(call().is_err());
        assert_eq!(fs::read(config_path(journal.path())).unwrap(), before);
    }

    fn restore_with_counters(journal: &Path, counters: [Value; 6]) -> Result<(), BackupError> {
        record_restore_result(
            journal,
            "ok",
            Value::Null,
            Value::Null,
            "day",
            Value::Null,
            counters[0].clone(),
            counters[1].clone(),
            counters[2].clone(),
            counters[3].clone(),
            counters[4].clone(),
            counters[5].clone(),
        )
    }

    #[test]
    fn reads_merged_defaults_without_creating_config() {
        let journal = journal();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config, backup_defaults());
        assert!(!journal.path().join("config").exists());
        assert_eq!(get_destination(journal.path()).unwrap(), None);
        assert_eq!(get_keys(journal.path()).unwrap(), None);
    }
    #[test]
    fn destination_requires_strings_and_object() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup":{"destination":{"repository":1,"backend":"s3","credentials":{}}}}),
        );
        assert_eq!(get_destination(journal.path()).unwrap(), None);
    }

    #[test]
    fn typed_setters_round_trip_without_extra_validation() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup": {"unknown_backup_key": "kept"}, "outside": "kept"}),
        );
        let destination = Destination {
            repository: "any repository".into(),
            backend: "any backend".into(),
            credentials: serde_json::from_value(json!({"unvalidated": [1]})).unwrap(),
        };
        set_destination(journal.path(), &destination).unwrap();
        set_enabled(journal.path(), true).unwrap();
        set_recovery_key(journal.path(), "unvalidated recovery key").unwrap();
        set_recovery_key_confirmed(journal.path(), true).unwrap();
        let raw: Value =
            serde_json::from_slice(&fs::read(config_path(journal.path())).unwrap()).unwrap();
        assert_eq!(raw["outside"], "kept");
        assert_eq!(raw["backup"]["unknown_backup_key"], "kept");
        assert_eq!(raw["backup"]["destination"]["repository"], "any repository");
        assert_eq!(raw["backup"]["enabled"], true);
        assert_eq!(raw["backup"]["recovery_key"], "unvalidated recovery key");
        assert_eq!(raw["backup"]["confirmed_recovery_key"], true);
    }

    #[test]
    fn set_mode_accepts_closed_vocabulary_and_rejects_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        set_mode(journal.path(), "byo").unwrap();
        set_mode(journal.path(), "operated").unwrap();
        assert_rejection_preserves_config(&journal, || set_mode(journal.path(), "invalid"));
    }

    #[test]
    fn set_retention_accepts_exact_valid_shape_and_rejects_invalid_shapes_and_values() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        let valid =
            serde_json::from_value(json!({"hourly": 0, "daily": 1, "weekly": 2, "monthly": 3}))
                .unwrap();
        set_retention(journal.path(), &valid).unwrap();
        for invalid in [
            json!({"hourly": 0, "daily": 1, "weekly": 2}),
            json!({"hourly": 0, "daily": 1, "weekly": 2, "monthly": 3, "extra": 4}),
            json!({"hourly": true, "daily": 1, "weekly": 2, "monthly": 3}),
            json!({"hourly": -1, "daily": 1, "weekly": 2, "monthly": 3}),
        ] {
            let invalid = serde_json::from_value(invalid).unwrap();
            assert_rejection_preserves_config(&journal, || set_retention(journal.path(), &invalid));
        }
    }

    #[test]
    fn set_offload_accepts_null_or_positive_bytes_and_rejects_invalid_values() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        let valid = serde_json::from_value(
            json!({"enabled": true, "budget_bytes": null, "floor_bytes": 1}),
        )
        .unwrap();
        set_offload(journal.path(), &valid).unwrap();
        for invalid in [
            json!({"enabled": true, "budget_bytes": 1}),
            json!({"enabled": true, "budget_bytes": 1, "floor_bytes": 2, "extra": 3}),
            json!({"enabled": 1, "budget_bytes": 1, "floor_bytes": 2}),
            json!({"enabled": true, "budget_bytes": true, "floor_bytes": 2}),
            json!({"enabled": true, "budget_bytes": 0, "floor_bytes": 2}),
            json!({"enabled": true, "budget_bytes": -1, "floor_bytes": 2}),
        ] {
            let invalid = serde_json::from_value(invalid).unwrap();
            assert_rejection_preserves_config(&journal, || set_offload(journal.path(), &invalid));
        }
    }

    #[test]
    fn clear_backup_config_preserves_top_level_keys_and_is_idempotent() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"outside": {"kept": true}, "backup": {"enabled": true, "future": "discarded"}}),
        );
        clear_backup_config(journal.path()).unwrap();
        let raw: Value =
            serde_json::from_slice(&fs::read(config_path(journal.path())).unwrap()).unwrap();
        assert_eq!(raw["outside"], json!({"kept": true}));
        assert_eq!(raw["backup"], Value::Object(backup_defaults()));
        let before = fs::read(config_path(journal.path())).unwrap();
        clear_backup_config(journal.path()).unwrap();
        assert_eq!(fs::read(config_path(journal.path())).unwrap(), before);
    }
    #[test]
    fn record_backup_and_prune_accept_arbitrary_status_strings() {
        let journal = journal();
        write_config(journal.path(), json!({"backup":{}}));
        record_backup_result(
            journal.path(),
            "anything",
            json!(5),
            json!("snap"),
            Value::Null,
        )
        .unwrap();
        record_prune_result(journal.path(), "anything", json!(6), Value::Null).unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_backup"]["status"], "anything");
        assert_eq!(config["last_prune"]["status"], "anything");
    }

    #[test]
    fn record_offload_rejects_unknown_status_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        assert_rejection_preserves_config(&journal, || {
            record_offload_result(
                journal.path(),
                "unknown",
                json!(1),
                Value::Null,
                json!(1),
                json!(2),
                json!(false),
            )
        });
    }

    #[test]
    fn record_offload_keeps_caller_owned_counter_values_and_last_ok_rules() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup": {"last_offload": {"last_ok_time": 99}}}),
        );
        record_offload_result(
            journal.path(),
            "ok",
            json!(1),
            Value::Null,
            json!(false),
            json!(-1),
            json!("not-bool"),
        )
        .unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_offload"]["last_ok_time"], 1);
        assert_eq!(config["last_offload"]["files_marked"], false);
        assert_eq!(config["last_offload"]["bytes_marked"], -1);
        assert_eq!(
            config["last_offload"]["ran_out_of_markable_media"],
            "not-bool"
        );
        record_offload_result(
            journal.path(),
            "ok",
            Value::Null,
            Value::Null,
            json!(0),
            json!(0),
            json!(false),
        )
        .unwrap();
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_offload"]["last_ok_time"],
            Value::Null
        );
        record_offload_result(
            journal.path(),
            "ok",
            json!(7),
            Value::Null,
            json!(0),
            json!(0),
            json!(false),
        )
        .unwrap();
        record_offload_result(
            journal.path(),
            "stalled",
            json!(8),
            Value::Null,
            json!(0),
            json!(0),
            json!(false),
        )
        .unwrap();
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_offload"]["last_ok_time"],
            7
        );
    }

    #[test]
    fn record_verification_rejects_unknown_status_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        assert_rejection_preserves_config(&journal, || {
            record_verification_result(
                journal.path(),
                "unknown",
                json!(1),
                Value::Null,
                json!("1/1"),
            )
        });
    }

    #[test]
    fn record_verification_replaces_or_preserves_last_ok_and_checked_subset_as_required() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"backup": {"last_verification": {"last_ok_time": 99, "checked_subset": "old"}}}),
        );
        record_verification_result(journal.path(), "ok", json!(1), Value::Null, json!("new"))
            .unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_verification"]["last_ok_time"], 1);
        assert_eq!(config["last_verification"]["checked_subset"], "new");
        record_verification_result(journal.path(), "ok", Value::Null, Value::Null, json!("all"))
            .unwrap();
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_verification"]["last_ok_time"],
            Value::Null
        );
        record_verification_result(journal.path(), "ok", json!(7), Value::Null, json!("kept"))
            .unwrap();
        record_verification_result(
            journal.path(),
            "error",
            json!(8),
            json!("reason"),
            json!("cleared"),
        )
        .unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(config["last_verification"]["last_ok_time"], 7);
        assert_eq!(config["last_verification"]["checked_subset"], Value::Null);
    }

    #[test]
    fn record_restore_rejects_status_and_scope_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        assert_rejection_preserves_config(&journal, || {
            record_restore_result(
                journal.path(),
                "unknown",
                Value::Null,
                Value::Null,
                "day",
                Value::Null,
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
            )
        });
        assert_rejection_preserves_config(&journal, || {
            record_restore_result(
                journal.path(),
                "ok",
                Value::Null,
                Value::Null,
                "month",
                Value::Null,
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
                json!(0),
            )
        });
    }

    #[test]
    fn record_restore_rejects_each_counter_kind_without_mutating() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        for index in 0..6 {
            for invalid in [json!(true), json!(1.5), json!("3"), json!(-1)] {
                let mut counters = [json!(0), json!(0), json!(0), json!(0), json!(0), json!(0)];
                counters[index] = invalid;
                assert_rejection_preserves_config(&journal, || {
                    restore_with_counters(journal.path(), counters)
                });
            }
        }
    }

    #[test]
    fn record_restore_allows_null_journal_file_counters_but_not_segment_counters() {
        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        record_restore_result(
            journal.path(),
            "degraded",
            json!(7),
            json!("restore_summary_missing"),
            "journal",
            Value::Null,
            json!(0),
            json!(0),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        )
        .unwrap();
        let restored = &get_backup_config(journal.path()).unwrap()["last_restore"];
        assert_eq!(restored["scope"], "journal");
        assert_eq!(restored["files_expected"], Value::Null);
        assert_eq!(restored["bytes_restored"], Value::Null);

        assert_rejection_preserves_config(&journal, || {
            record_restore_result(
                journal.path(),
                "error",
                json!(8),
                json!("restore_failed"),
                "journal",
                Value::Null,
                Value::Null,
                json!(0),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            )
        });
    }

    #[cfg(unix)]
    #[test]
    fn matching_mutation_is_a_metadata_preserving_no_op() {
        use std::os::unix::fs::MetadataExt;

        let journal = journal();
        write_config(
            journal.path(),
            json!({"outside": "kept", "backup": {"enabled": true, "future": "kept"}}),
        );
        let path = config_path(journal.path());
        let before = fs::read(&path).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        set_enabled(journal.path(), true).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(after.ino(), metadata.ino());
        assert_eq!(after.mode(), metadata.mode());
        assert_eq!(after.modified().unwrap(), metadata.modified().unwrap());
    }

    #[test]
    fn mutation_preserves_unrelated_top_level_and_backup_keys() {
        let journal = journal();
        write_config(
            journal.path(),
            json!({"outside": {"kept": true}, "backup": {"future": "kept", "daily_key": "daily"}}),
        );
        set_enabled(journal.path(), true).unwrap();
        let raw: Value =
            serde_json::from_slice(&fs::read(config_path(journal.path())).unwrap()).unwrap();
        assert_eq!(raw["outside"], json!({"kept": true}));
        assert_eq!(raw["backup"]["future"], "kept");
        assert_eq!(raw["backup"]["daily_key"], "daily");
    }

    #[test]
    fn disjoint_mutations_survive_deterministic_contention() {
        let journal = tempfile::tempdir().unwrap();
        write_config(journal.path(), json!({"backup": {}}));
        let root = journal.path().to_owned();
        let start = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(2));
        let first_hook = Arc::new(AtomicBool::new(true));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let enabled_root = root.clone();
        let enabled_start = Arc::clone(&start);
        let enabled_release = Arc::clone(&release);
        let enabled_first = Arc::clone(&first_hook);
        let enabled_entered = entered_tx.clone();
        let enabled_started = started_tx.clone();
        let enabled = thread::spawn(move || {
            let _guard = install_mutation_hook(Rc::new(move || {
                enabled_entered.send(()).unwrap();
                if enabled_first.swap(false, Ordering::SeqCst) {
                    enabled_release.wait();
                }
            }));
            enabled_started.send(()).unwrap();
            enabled_start.wait();
            set_enabled(&enabled_root, true).unwrap();
        });
        let mode_root = root.clone();
        let mode_start = Arc::clone(&start);
        let mode_release = Arc::clone(&release);
        let mode_first = Arc::clone(&first_hook);
        let mode_entered = entered_tx;
        let mode_started = started_tx;
        let mode = thread::spawn(move || {
            let _guard = install_mutation_hook(Rc::new(move || {
                mode_entered.send(()).unwrap();
                if mode_first.swap(false, Ordering::SeqCst) {
                    mode_release.wait();
                }
            }));
            mode_started.send(()).unwrap();
            mode_start.wait();
            set_mode(&mode_root, "operated").unwrap();
        });
        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        start.wait();
        entered_rx.recv().unwrap();
        release.wait();
        enabled.join().unwrap();
        mode.join().unwrap();
        entered_rx.recv().unwrap();
        let config = get_backup_config(&root).unwrap();
        assert_eq!(config["enabled"], true);
        assert_eq!(config["mode"], "operated");
    }

    #[test]
    #[cfg(unix)]
    fn key_generation_returns_existing_pair_without_publishing() {
        use std::os::unix::fs::MetadataExt;

        let journal = journal();
        let first = generate_and_store_keys(journal.path()).unwrap();
        let config = config_path(journal.path());
        let bytes = fs::read(&config).unwrap();
        let metadata = fs::metadata(&config).unwrap();
        let inode = metadata.ino();
        let mode = metadata.mode();
        let modified = metadata.modified().unwrap();
        let second = generate_and_store_keys(journal.path()).unwrap();
        assert_eq!(second, first);
        assert_eq!(fs::read(&config).unwrap(), bytes);
        let after = fs::metadata(&config).unwrap();
        assert_eq!(after.ino(), inode);
        assert_eq!(after.mode(), mode);
        assert_eq!(after.modified().unwrap(), modified);
    }

    #[cfg(unix)]
    #[test]
    fn key_generation_fills_only_missing_key_with_one_publication() {
        use std::os::unix::fs::MetadataExt;

        let one_missing = tempfile::tempdir().unwrap();
        write_config(
            one_missing.path(),
            json!({"backup":{"daily_key":"existing"}}),
        );
        let config = config_path(one_missing.path());
        let before = fs::metadata(&config).unwrap();
        let transactions = Arc::new(AtomicUsize::new(0));
        let transaction_counter = Arc::clone(&transactions);
        let _guard = install_mutation_hook(Rc::new(move || {
            transaction_counter.fetch_add(1, Ordering::SeqCst);
        }));
        let keys = generate_and_store_keys(one_missing.path()).unwrap();
        assert_eq!(keys.daily_key, "existing");
        assert!(!keys.recovery_key.is_empty());
        assert_eq!(get_keys(one_missing.path()).unwrap(), Some(keys));
        assert_eq!(transactions.load(Ordering::SeqCst), 1);
        let after = fs::metadata(&config).unwrap();
        assert_ne!(after.ino(), before.ino());
    }

    #[test]
    fn concurrent_key_generation_converges_on_one_persisted_pair() {
        let concurrent = tempfile::tempdir().unwrap();
        let root = concurrent.path().to_owned();
        let start = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(2));
        let first_hook = Arc::new(AtomicBool::new(true));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let first_hook = Arc::clone(&first_hook);
            let entered_tx = entered_tx.clone();
            let started_tx = started_tx.clone();
            workers.push(thread::spawn(move || {
                let _guard = install_mutation_hook(Rc::new(move || {
                    entered_tx.send(()).unwrap();
                    if first_hook.swap(false, Ordering::SeqCst) {
                        release.wait();
                    }
                }));
                started_tx.send(()).unwrap();
                start.wait();
                generate_and_store_keys(&root).unwrap()
            }));
        }
        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        start.wait();
        entered_rx.recv().unwrap();
        release.wait();
        let left = workers.remove(0).join().unwrap();
        let right = workers.remove(0).join().unwrap();
        entered_rx.recv().unwrap();
        assert_eq!(left, right);
        assert_eq!(get_keys(&root).unwrap(), Some(left));
    }

    #[cfg(unix)]
    #[test]
    fn lock_acquisition_failure_does_not_publish() {
        use std::os::unix::fs::symlink;

        let journal = journal();
        write_config(journal.path(), json!({"backup": {}}));
        let config = journal.path().join("config/journal.json");
        let before = fs::read(&config).unwrap();
        let sentinel = journal.path().join("sentinel");
        fs::write(&sentinel, b"sentinel").unwrap();
        symlink(&sentinel, journal.path().join("config/journal.json.lock")).unwrap();
        assert!(set_enabled(journal.path(), true).is_err());
        assert_eq!(fs::read(&config).unwrap(), before);
    }
}
