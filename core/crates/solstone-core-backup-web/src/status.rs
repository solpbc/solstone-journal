use serde_json::{Map, Value, json};
use std::path::Path;

use solstone_core_backup::load_hosted_binding;
use solstone_core_offload::build_offload_status;

use crate::{
    config,
    measurement::{self, SharedMeasurementCache},
    operation::SharedOperationSlot,
};

fn operation_value(operations: &SharedOperationSlot) -> Value {
    crate::operation::current(operations)
        .map(|operation| serde_json::to_value(operation).expect("operation is serializable"))
        .unwrap_or(Value::Null)
}

fn hosted_view(root: &Path) -> Value {
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

pub fn status(root: &Path, operations: &SharedOperationSlot) -> Result<Value, ()> {
    let backup = config::backup(root)?;
    let destination = backup.get("destination").and_then(Value::as_object);
    Ok(json!({
        "success": true, "enabled": backup.get("enabled").cloned().unwrap_or(Value::Null), "mode": backup.get("mode").cloned().unwrap_or(Value::Null),
        "destination": {"repository": destination.and_then(|d| d.get("repository")).cloned().unwrap_or(Value::Null), "backend": destination.and_then(|d| d.get("backend")).cloned().unwrap_or(Value::Null), "credentials_set": destination.and_then(|d| d.get("credentials")).is_some_and(|value| value.as_object().is_some_and(|value| !value.is_empty()))},
        "daily_key_set": backup.get("daily_key").is_some_and(|value| !value.is_null()), "recovery_key_set": backup.get("recovery_key").is_some_and(|value| !value.is_null()), "recovery_key_confirmed": backup.get("confirmed_recovery_key").and_then(Value::as_bool).unwrap_or(false),
        "retention": backup.get("retention").cloned().unwrap_or(Value::Null), "offload": backup.get("offload").cloned().unwrap_or(Value::Null), "schedule": backup.get("schedule").cloned().unwrap_or(Value::Null),
        "last_backup": backup.get("last_backup").cloned().unwrap_or(Value::Null), "last_prune": backup.get("last_prune").cloned().unwrap_or(Value::Null), "last_offload": backup.get("last_offload").cloned().unwrap_or(Value::Null), "last_verification": backup.get("last_verification").cloned().unwrap_or(Value::Null), "last_restore": backup.get("last_restore").cloned().unwrap_or(Value::Null), "hosted": hosted_view(root), "operation": operation_value(operations)
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
    value
        .as_object_mut()
        .ok_or(())?
        .insert("operation".to_owned(), operation_value(operations));
    Ok(value)
}
