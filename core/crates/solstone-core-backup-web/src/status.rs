// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use std::path::Path;

use solstone_core_backup::load_hosted_binding;
use solstone_core_offload::build_offload_status;

use crate::{
    config,
    measurement::{self, SharedMeasurementCache},
    operation::{Operation, SharedOperationSlot},
};

fn operation_value(operation: Option<&Operation>) -> Value {
    operation
        .map(|operation| serde_json::to_value(operation).expect("operation is serializable"))
        .unwrap_or(Value::Null)
}

fn hosted_view(root: &Path, operation: Option<&Operation>) -> Value {
    if operation
        .is_some_and(|operation| operation.kind == "enable_hosted" && operation.phase != "done")
    {
        return json!({"bound": false});
    }

    // Lift bound/bucket/prefix only. status_view has no success/operation and
    // errors when destination is missing.
    match load_hosted_binding(root) {
        Some(binding) => json!({
            "bound": true,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
        }),
        None => json!({"bound": false}),
    }
}

// The scheduler owns effective cadence. Backup configuration owns retention,
// credentials and operation history, but cannot report whether a job will run.
fn backup_schedule(root: &Path) -> Value {
    match solstone_core_system::schedule::read_enabled_schedule_entry(
        &root.join("config/schedules.json"),
        "maintenance:backup:run",
    ) {
        Ok((Some(entry), _)) => json!({"enabled": true, "every": entry.every}),
        Ok((None, diagnostics)) if diagnostics.is_empty() => {
            json!({"enabled": false, "every": null})
        }
        _ => Value::Null,
    }
}

pub fn status(root: &Path, operations: &SharedOperationSlot) -> Result<Value, ()> {
    let backup = config::backup(root)?;
    let destination = backup.get("destination").and_then(Value::as_object);
    let operation = crate::operation::current(operations);
    Ok(json!({
        "success": true, "enabled": backup.get("enabled").cloned().unwrap_or(Value::Null), "mode": backup.get("mode").cloned().unwrap_or(Value::Null),
        "destination": {"repository": destination.and_then(|d| d.get("repository")).cloned().unwrap_or(Value::Null), "backend": destination.and_then(|d| d.get("backend")).cloned().unwrap_or(Value::Null), "credentials_set": destination.and_then(|d| d.get("credentials")).is_some_and(|value| value.as_object().is_some_and(|value| !value.is_empty()))},
        "daily_key_set": backup.get("daily_key").is_some_and(|value| !value.is_null()), "recovery_key_set": backup.get("recovery_key").is_some_and(|value| !value.is_null()), "recovery_key_confirmed": backup.get("confirmed_recovery_key").and_then(Value::as_bool).unwrap_or(false),
        "retention": backup.get("retention").cloned().unwrap_or(Value::Null), "offload": backup.get("offload").cloned().unwrap_or(Value::Null), "schedule": backup_schedule(root),
        "last_backup": backup.get("last_backup").cloned().unwrap_or(Value::Null), "last_prune": backup.get("last_prune").cloned().unwrap_or(Value::Null), "last_offload": backup.get("last_offload").cloned().unwrap_or(Value::Null), "last_verification": backup.get("last_verification").cloned().unwrap_or(Value::Null), "last_restore": backup.get("last_restore").cloned().unwrap_or(Value::Null), "hosted": hosted_view(root, operation.as_ref()), "operation": operation_value(operation.as_ref())
    }))
}

pub fn offload(
    root: &Path,
    cache: &SharedMeasurementCache,
    operations: &SharedOperationSlot,
) -> Result<Value, ()> {
    let mut value = build_offload_status(root).map_err(|_| ())?.value;
    let measured = measurement::snapshot(cache);
    let device = Map::from_iter([
        ("free_bytes".to_owned(), measured["free_bytes"].clone()),
        ("total_bytes".to_owned(), measured["total_bytes"].clone()),
    ]);
    value
        .as_object_mut()
        .ok_or(())?
        .insert("device".to_owned(), Value::Object(device));
    value.as_object_mut().ok_or(())?.insert(
        "suggested_defaults".to_owned(),
        measured["suggested_defaults"].clone(),
    );
    value
        .as_object_mut()
        .ok_or(())?
        .insert("success".to_owned(), Value::Bool(true));
    let operation = crate::operation::current(operations);
    value
        .as_object_mut()
        .ok_or(())?
        .insert("operation".to_owned(), operation_value(operation.as_ref()));
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{backup_schedule, hosted_view};
    use crate::{operation::Operation, test_support::hosted_binding};
    use solstone_core_backup::save_hosted_binding;

    #[test]
    fn effective_schedule_comes_from_scheduler_and_unknown_is_not_disabled() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            backup_schedule(root.path()),
            serde_json::json!({"enabled":false,"every":null})
        );
        assert!(
            !root.path().join("config").exists(),
            "status must not initialize schedules"
        );
        std::fs::create_dir(root.path().join("config")).unwrap();
        let path = root.path().join("config/schedules.json");
        for (raw, expected) in [
            (
                r#"{"maintenance:backup:run":{"cmd":["journal","maintenance","run","backup:run"],"every":"hourly","enabled":true}}"#,
                serde_json::json!({"enabled":true,"every":"hourly"}),
            ),
            (
                r#"{"maintenance:backup:run":{"cmd":["journal","maintenance","run","backup:run"],"every":"hourly","enabled":false}}"#,
                serde_json::json!({"enabled":false,"every":null}),
            ),
            ("broken", serde_json::Value::Null),
        ] {
            std::fs::write(&path, raw).unwrap();
            assert_eq!(backup_schedule(root.path()), expected);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
    }

    fn operation(kind: &str, phase: &str) -> Operation {
        Operation {
            kind: kind.to_owned(),
            phase: phase.to_owned(),
            reason_code: None,
            recording_failure: None,
            portal_url: None,
        }
    }

    #[test]
    fn hosted_view_reflects_enable_hosted_outcome_not_preliminary_binding() {
        let cases = [
            (
                "enable_hosted setting_up",
                Some(("enable_hosted", "setting_up")),
                false,
            ),
            ("enable_hosted done", Some(("enable_hosted", "done")), true),
            (
                "enable_hosted error",
                Some(("enable_hosted", "error")),
                false,
            ),
            (
                "restore_hosted restoring",
                Some(("restore_hosted", "restoring")),
                true,
            ),
            ("no operation", None, true),
        ];

        for binding_present in [false, true] {
            for (name, state, expected_when_present) in cases {
                let root = tempfile::tempdir().expect("temporary root");
                if binding_present {
                    save_hosted_binding(root.path(), &hosted_binding())
                        .expect("save hosted binding");
                }
                let operation = state.map(|(kind, phase)| operation(kind, phase));
                let view = hosted_view(root.path(), operation.as_ref());
                let expected = binding_present && expected_when_present;

                assert_eq!(
                    view["bound"], expected,
                    "{name}, binding_present={binding_present}"
                );
                if expected {
                    assert_eq!(view["bucket"], "bucket", "{name}");
                    assert_eq!(view["prefix"], "owner/prefix", "{name}");
                } else {
                    assert!(view.get("bucket").is_none(), "{name}");
                    assert!(view.get("prefix").is_none(), "{name}");
                }
            }
        }
    }
}
