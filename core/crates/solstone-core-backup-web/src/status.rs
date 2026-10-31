// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use std::path::Path;

use solstone_core_backup::{HostedBinding, backup_setup_fingerprint, load_hosted_binding};
use solstone_core_offload::status::build_checked_offload_status;

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

fn hosted_view(binding: Option<&HostedBinding>, operation: Option<&Operation>) -> Value {
    if operation
        .is_some_and(|operation| operation.kind == "enable_hosted" && operation.phase != "done")
    {
        return json!({"bound": false});
    }

    // Lift bound/bucket/prefix only. status_view has no success/operation and
    // errors when destination is missing.
    match binding {
        Some(binding) => json!({
            "bound": true,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
        }),
        None => json!({"bound": false}),
    }
}

// A worker writes its result to disk before it publishes a terminal phase, so a
// snapshot must observe the operation before it reads the persisted state. Read
// in the other order and a result that lands between the two reads is missed
// while its terminal phase is still reported.
pub fn status(root: &Path, operations: &SharedOperationSlot) -> Result<Value, ()> {
    #[cfg(windows)]
    let operation = crate::operation::observed_current(operations)?;
    #[cfg(not(windows))]
    let operation = crate::operation::current(operations);
    status_with_operation(root, operation.as_ref())
}

pub(crate) fn status_with_operation(
    root: &Path,
    operation: Option<&Operation>,
) -> Result<Value, ()> {
    let backup = config::backup(root)?;
    let binding = load_hosted_binding(root);
    let current_setup = backup
        .get("last_backup")
        .and_then(|result| result.get("setup_fingerprint"))
        .and_then(Value::as_str)
        .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|stored| {
            backup_setup_fingerprint(&backup, binding.as_ref()).map(|current| stored == current)
        });
    let destination = backup.get("destination").and_then(Value::as_object);
    let pending_val = backup.get("pending_key_revocation");
    let revocation_unresolved = pending_val.is_some();
    let revocation_pending = pending_val
        .and_then(solstone_core_backup::PendingKeyRevocation::parse)
        .is_some();
    Ok(with_cleanup_admission(json!({
        "success": true, "enabled": backup.get("enabled").cloned().unwrap_or(Value::Null), "paused_at": solstone_core_backup::backup_paused_at(&backup).map_or(Value::Null, Value::from), "mode": backup.get("mode").cloned().unwrap_or(Value::Null),
        "destination": {"repository": destination.and_then(|d| d.get("repository")).cloned().unwrap_or(Value::Null), "backend": destination.and_then(|d| d.get("backend")).cloned().unwrap_or(Value::Null), "credentials_set": destination.and_then(|d| d.get("credentials")).is_some_and(|value| value.as_object().is_some_and(|value| !value.is_empty()))},
        "daily_key_set": backup.get("daily_key").is_some_and(|value| !value.is_null()), "recovery_key_set": backup.get("recovery_key").is_some_and(|value| !value.is_null()), "recovery_key_confirmed": backup.get("confirmed_recovery_key").and_then(Value::as_bool).unwrap_or(false),
        "recovery_key_revocation_pending": revocation_pending, "recovery_key_revocation_unresolved": revocation_unresolved,
        "retention": backup.get("retention").cloned().unwrap_or(Value::Null), "offload": backup.get("offload").cloned().unwrap_or(Value::Null), "schedule": solstone_core_backup_runtime::effective_backup_schedule(root),
        "last_backup": backup.get("last_backup").cloned().unwrap_or(Value::Null), "last_backup_current_setup": current_setup, "last_prune": backup.get("last_prune").cloned().unwrap_or(Value::Null), "last_offload": backup.get("last_offload").cloned().unwrap_or(Value::Null), "last_verification": backup.get("last_verification").cloned().unwrap_or(Value::Null), "last_restore": backup.get("last_restore").cloned().unwrap_or(Value::Null), "hosted": hosted_view(binding.as_ref(), operation), "operation": operation_value(operation)
    })))
}

/// Home's small read-only observation. Historical or routine failures are quiet;
/// inability to associate a qualified result is unavailable, never recovery.
pub fn home_backup_observation(deps: &crate::BackupWebDeps) -> Value {
    let Ok(status) = status(&deps.journal_root, &deps.operations) else {
        return json!({"available": false});
    };
    let Some(enabled) = status["enabled"].as_bool() else {
        return json!({"available": false});
    };
    // A pause is the owner's choice, never an issue.
    if !enabled
        || status["paused_at"].is_i64()
        || status["operation"].as_object().is_some_and(|op| {
            op.get("phase")
                .and_then(Value::as_str)
                .is_none_or(|phase| !crate::operation::is_terminal(phase))
        })
    {
        return json!({"available": true, "issue": false});
    }
    if !status["last_backup"].is_object() {
        return json!({"available": false});
    }
    match &status["last_backup"]["status"] {
        Value::Null => {}
        Value::String(state) if matches!(state.as_str(), "error" | "ok") => {}
        _ => return json!({"available": false}),
    }
    if status["last_backup"]["status"] != "error"
        || status["last_backup"]["error_reason"] != "auth_failed"
    {
        return json!({"available": true, "issue": false});
    }
    // Records predating attempt provenance cannot justify a current warning.
    if status["last_backup"].get("setup_fingerprint").is_none() {
        return json!({"available": true, "issue": false});
    }
    match status["last_backup_current_setup"].as_bool() {
        Some(current) => json!({"available": true, "issue": current}),
        None => json!({"available": false}),
    }
}

pub fn offload(
    root: &Path,
    cache: &SharedMeasurementCache,
    operations: &SharedOperationSlot,
) -> Result<Value, ()> {
    // Operation first, then persisted state: see `status`.
    #[cfg(windows)]
    let operation = crate::operation::observed_current(operations)?;
    #[cfg(not(windows))]
    let operation = crate::operation::current(operations);
    let mut value = build_checked_offload_status(root).map_err(|_| ())?.value;
    value["enable_supported"] = json!(!cfg!(any(target_os = "ios", target_os = "android")));
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
    value
        .as_object_mut()
        .ok_or(())?
        .insert("operation".to_owned(), operation_value(operation.as_ref()));
    Ok(with_cleanup_admission(value))
}

fn with_cleanup_admission(value: Value) -> Value {
    #[cfg(windows)]
    {
        let mut value = value;
        value["cleanup_admission"] = json!(crate::cleanup::readiness());
        value
    }
    #[cfg(not(windows))]
    value
}

#[cfg(test)]
mod tests {
    #[test]
    fn home_issue_suppresses_active_operation_but_returns_after_each_terminal_phase() {
        use solstone_core_backup::{
            Destination, generate_and_store_keys, record_backup_result, set_destination,
            set_enabled,
        };
        let root = tempfile::tempdir().unwrap();
        set_destination(
            root.path(),
            &Destination {
                repository: "s3:fixture".into(),
                backend: "s3".into(),
                credentials: Map::new(),
            },
        )
        .unwrap();
        generate_and_store_keys(root.path()).unwrap();
        set_enabled(root.path(), true).unwrap();
        let config = solstone_core_backup::get_backup_config(root.path()).unwrap();
        let fingerprint = backup_setup_fingerprint(&config, None).unwrap();
        record_backup_result(
            root.path(),
            "error",
            json!(100),
            Value::Null,
            json!("auth_failed"),
            None,
            Some(&fingerprint),
        )
        .unwrap();
        let deps = crate::BackupWebDeps::new(root.path().into());
        assert_eq!(home_backup_observation(&deps)["issue"], true);
        let begin = crate::operation::begin(&deps.operations, "backup", None, None, None).unwrap();
        assert_eq!(home_backup_observation(&deps)["issue"], false);
        for phase in [
            "cancelling",
            "done",
            "error",
            "degraded",
            "needs_subscription",
            "refused",
        ] {
            deps.operations
                .lock()
                .unwrap()
                .current
                .as_mut()
                .unwrap()
                .view
                .phase = phase.into();
            assert_eq!(
                home_backup_observation(&deps)["issue"],
                crate::operation::is_terminal(phase),
                "phase {phase}"
            );
        }
        let _ = begin;
    }

    use super::{backup_setup_fingerprint, home_backup_observation, hosted_view};
    use crate::{operation::Operation, test_support::hosted_binding};
    use serde_json::{Map, Value, json};
    use solstone_core_backup::save_hosted_binding;

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn persisted_status_reaches_the_shared_health_backup_renderer() {
        use serde_json::json;
        use std::process::Command;
        let root = crate::test_support::root("healthy");
        let slot = crate::operation::new_slot();
        let success = json!({"status":"ok","time":1770000000,"snapshot_id":"copy-a","error_reason":"incomplete"});
        let mut fixtures = Vec::new();
        for (enabled, attempt, tone) in [
            (
                true,
                json!({"status":"error","time":1770000200,"error_reason":"timeout","last_success":success}),
                "attention",
            ),
            (
                true,
                json!({"status":"ok","time":1770000300,"last_success":null}),
                "neutral",
            ),
            (true, json!({"status":null,"time":null}), "unavailable"),
            (false, json!({"status":null,"time":null}), "neutral"),
        ] {
            crate::config::mutate(root.path(), |backup| {
                backup.insert("enabled".into(), json!(enabled));
                backup.insert("last_backup".into(), attempt.clone());
                (true, ())
            })
            .expect("persist fixture");
            let payload = super::status(root.path(), &slot).expect("actual status producer");
            fixtures.push(json!({"payload":payload,"tone":tone,
                "success_time": if attempt["last_success"]["status"] == "ok" { Some(1770000000_u64) } else { None }}));
        }
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let health = crate_dir.parent().unwrap().join("solstone-core-health-web");
        let output = Command::new("node")
            .arg(health.join("tests/health_web_dom.js"))
            .arg(&health)
            .env(
                "BACKUP_STATUS_FIXTURES",
                serde_json::to_string(&fixtures).unwrap(),
            )
            .output()
            .expect("Health DOM consumer starts");
        assert!(
            output.status.success(),
            "Health backup consumer failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn offload_keeps_actual_media_and_cached_geometry_when_catalog_is_uncertain() {
        use serde_json::json;
        use solstone_core_offload::append_offload_event;
        use solstone_core_retention::archive_catalog::OffloadFile;
        use std::fs;

        let root = crate::test_support::root("healthy");
        let segment = root.path().join("chronicle/20260101/010000_001");
        fs::create_dir_all(&segment).unwrap();
        for (name, bytes) in [
            ("ordinary.webm", 11),
            ("pending.flac", 7),
            ("kept.flac", 5),
            ("metadata.txt", 100),
        ] {
            fs::write(segment.join(name), vec![0; bytes]).unwrap();
        }
        append_offload_event(
            root.path(),
            "20260101",
            "_default",
            "010000_001",
            "snapshot",
            &[OffloadFile {
                name: "pending.flac".into(),
                bytes: 7,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        let cache = crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
            free_bytes: Some(31),
            total_bytes: Some(100),
        });
        let slot = crate::operation::new_slot();
        let before = super::offload(root.path(), &cache, &slot).unwrap();
        assert_eq!(before["raw_media"]["total_bytes"], 23);
        assert_eq!(before["raw_media"]["total_files"], 3);
        assert_eq!(before["raw_media"]["complete"], true);
        assert_eq!(before["backup_only"]["unbound_files"], 1);
        assert_eq!(before["backup_only"]["complete"], false);
        assert!(before["backup_only"]["total_bytes"].is_null());
        assert!(before["pending_release"]["total_bytes"].is_null());
        assert_eq!(before["proposed_release"]["available"], false);
        for key in [
            "structural_bytes",
            "eligible_bytes",
            "projected_local_bytes",
        ] {
            assert!(before["proposed_release"][key].is_null());
        }
        assert_eq!(before["device"], json!({"free_bytes":31,"total_bytes":100}));
        let ledger = solstone_core_offload::ledger_path_for_day(root.path(), "20260101").unwrap();
        fs::write(&ledger, b"{broken}\n").unwrap();
        let after = super::offload(root.path(), &cache, &slot).unwrap();
        assert_eq!(after["raw_media"]["total_bytes"], 23);
        assert_eq!(after["raw_media"]["complete"], true);
        assert_eq!(after["backup_only"]["degraded"], true);
        assert!(after["backup_only"]["total_bytes"].is_null());
        assert_eq!(after["device"], before["device"]);
        assert_eq!(after["suggested_defaults"], before["suggested_defaults"]);
        assert_eq!(fs::read(ledger).unwrap(), b"{broken}\n");
    }

    #[cfg(not(windows))]
    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn status_never_reports_a_terminal_operation_without_its_recorded_result() {
        use serde_json::Value;
        use std::{thread, time::Duration};

        let root = crate::test_support::root("healthy");
        let slot = crate::operation::new_slot();
        crate::operation::begin(&slot, "rotate", None, None, None).expect("begin");

        // The worker holds the slot while its result lands and its terminal
        // phase is published, so a reader started now has to wait for it.
        let mut worker = slot.lock().expect("operation slot lock");
        let reader = {
            let root = root.path().to_path_buf();
            let slot = slot.clone();
            thread::spawn(move || super::status(&root, &slot).expect("status"))
        };
        // Let the reader run up to the slot. A reader that samples persisted
        // state before the operation samples it here, before the result exists.
        thread::sleep(Duration::from_millis(100));
        crate::config::mutate(root.path(), |backup| {
            backup.insert("confirmed_recovery_key".to_owned(), Value::Bool(false));
            (true, ())
        })
        .expect("record result");
        worker.current.as_mut().expect("slot").view.phase = "done".to_owned();
        drop(worker);

        let body = reader.join().expect("reader");
        assert_eq!(body["operation"]["phase"], "done");
        assert_eq!(body["recovery_key_confirmed"], false);
    }

    fn operation(kind: &str, phase: &str) -> Operation {
        Operation {
            kind: kind.to_owned(),
            phase: phase.to_owned(),
            reason_code: None,
            detail: None,
            guidance: None,
            recording_failure: None,
            report: None,
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
                let view = hosted_view(
                    solstone_core_backup::load_hosted_binding(root.path()).as_ref(),
                    operation.as_ref(),
                );
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
