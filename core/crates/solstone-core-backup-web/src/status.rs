use serde_json::{Map, Value, json};
use std::path::Path;

use crate::{
    config,
    measurement::{self, SharedMeasurementCache},
};

pub fn status(root: &Path) -> Result<Value, ()> {
    let backup = config::backup(root)?;
    let destination = backup.get("destination").and_then(Value::as_object);
    Ok(json!({
        "success": true, "enabled": backup.get("enabled").cloned().unwrap_or(Value::Null), "mode": backup.get("mode").cloned().unwrap_or(Value::Null),
        "destination": {"repository": destination.and_then(|d| d.get("repository")).cloned().unwrap_or(Value::Null), "backend": destination.and_then(|d| d.get("backend")).cloned().unwrap_or(Value::Null), "credentials_set": destination.and_then(|d| d.get("credentials")).is_some_and(|value| value.as_object().is_some_and(|value| !value.is_empty()))},
        "daily_key_set": backup.get("daily_key").is_some_and(|value| !value.is_null()), "recovery_key_set": backup.get("recovery_key").is_some_and(|value| !value.is_null()), "recovery_key_confirmed": backup.get("confirmed_recovery_key").and_then(Value::as_bool).unwrap_or(false),
        "retention": backup.get("retention").cloned().unwrap_or(Value::Null), "offload": backup.get("offload").cloned().unwrap_or(Value::Null), "schedule": backup.get("schedule").cloned().unwrap_or(Value::Null),
        "last_backup": backup.get("last_backup").cloned().unwrap_or(Value::Null), "last_prune": backup.get("last_prune").cloned().unwrap_or(Value::Null), "last_offload": backup.get("last_offload").cloned().unwrap_or(Value::Null), "last_verification": backup.get("last_verification").cloned().unwrap_or(Value::Null), "last_restore": backup.get("last_restore").cloned().unwrap_or(Value::Null), "hosted": {"bound": false}, "operation": Value::Null
    }))
}
pub fn offload(root: &Path, cache: &SharedMeasurementCache) -> Result<Value, ()> {
    let backup = config::backup(root)?;
    let measured = measurement::snapshot(cache);
    let device = Map::from_iter([
        ("free_bytes".to_owned(), measured["free_bytes"].clone()),
        ("total_bytes".to_owned(), measured["total_bytes"].clone()),
    ]);
    Ok(
        json!({"success":true,"offload":backup["offload"],"last_offload":backup["last_offload"],"last_verification":backup["last_verification"],"last_restore":backup["last_restore"],"device":device,"suggested_defaults":measured["suggested_defaults"],"raw_media":{"total_bytes":0,"total_files":0},"backup_only":{"total_bytes":0,"total_files":0,"total_segments":0,"total_days":0,"degraded":false,"skipped_records":0,"unreadable_ledgers":[]},"pending_release":{"total_bytes":0,"total_files":0,"total_segments":0,"total_days":0},"days":[],"operation":Value::Null}),
    )
}
