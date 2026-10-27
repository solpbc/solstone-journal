// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};
use std::path::Path;

pub fn defaults() -> Value {
    json!({"enabled":false,"mode":"byo","destination":{"repository":null,"backend":null,"credentials":{}},"daily_key":null,"recovery_key":null,"confirmed_recovery_key":false,"retention":{"hourly":24,"daily":7,"weekly":4,"monthly":12},"offload":{"enabled":false,"budget_bytes":null,"floor_bytes":null},"last_backup":{"time":null,"snapshot_id":null,"status":null,"error_reason":null},"last_prune":{"time":null,"status":null,"error_reason":null},"last_offload":{"time":null,"status":null,"reason":null,"last_ok_time":null,"files_marked":0,"bytes_marked":0,"ran_out_of_markable_media":false},"last_verification":{"time":null,"status":null,"reason":null,"last_ok_time":null,"checked_subset":null},"last_restore":{"time":null,"status":null,"reason":null,"scope":null,"day":null,"segments_selected":0,"segments_restored":0,"files_expected":0,"files_restored":0,"bytes_expected":0,"bytes_restored":0}})
}
fn merge(default: &Value, source: &Value) -> Value {
    match (default, source) {
        (_, Value::Null) => default.clone(),
        (Value::Object(left), Value::Object(right)) => {
            let mut output = left.clone();
            for (key, value) in right {
                let old = output.get(key).unwrap_or(&Value::Null);
                output.insert(key.clone(), merge(old, value));
            }
            Value::Object(output)
        }
        (_, value) => value.clone(),
    }
}
fn merge_backup(source: &Value) -> Value {
    let mut value = merge(&defaults(), source);
    if let Some(restored) = source.get("last_restore").and_then(Value::as_object) {
        for counter in [
            "files_expected",
            "files_restored",
            "bytes_expected",
            "bytes_restored",
        ] {
            if restored.get(counter) == Some(&Value::Null) {
                value["last_restore"][counter] = Value::Null;
            }
        }
    }
    value
}
pub fn backup(root: &Path) -> Result<Map<String, Value>, ()> {
    let config = read_journal_config(root)
        .map_err(|_| ())?
        .config
        .unwrap_or_default();
    backup_from_map(&config)
}

pub(crate) fn backup_from_map(config: &Map<String, Value>) -> Result<Map<String, Value>, ()> {
    // Defaults fill absent configuration, not malformed observations.
    if let Some(source) = config.get("backup") {
        let source = source.as_object().ok_or(())?;
        if source.get("enabled").is_some_and(|v| !v.is_boolean())
            || source
                .get("mode")
                .is_some_and(|v| !matches!(v.as_str(), Some("byo" | "operated")))
            || source.get("last_backup").is_some_and(|v| !v.is_object())
            || source
                .get("destination")
                .and_then(|v| v.get("credentials"))
                .is_some_and(|v| !v.is_object())
        {
            return Err(());
        }
    }
    merge_backup(config.get("backup").unwrap_or(&Value::Null))
        .as_object()
        .cloned()
        .ok_or(())
}
pub fn mutate<T>(
    root: &Path,
    mutator: impl FnOnce(&mut Map<String, Value>) -> (bool, T),
) -> Result<T, ()> {
    mutate_journal_config(root, LockOptions::default(), |config| {
        let backup = config
            .entry("backup".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !backup.is_object() {
            *backup = Value::Object(Map::new());
        }
        let backup = backup
            .as_object_mut()
            .expect("backup was normalized to an object");
        let (changed, value) = mutator(backup);
        JournalConfigMutation { changed, value }
    })
    .map(|result| result.value)
    .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_counters_preserve_explicit_unknown_and_known_values() {
        for count in [Value::Null, json!(0), json!(4)] {
            let merged = merge_backup(&json!({
                "enabled": null,
                "last_restore": {
                    "segments_selected": null,
                    "files_expected": count,
                    "files_restored": count,
                    "bytes_expected": count,
                    "bytes_restored": count
                }
            }));
            for counter in [
                "files_expected",
                "files_restored",
                "bytes_expected",
                "bytes_restored",
            ] {
                assert_eq!(merged["last_restore"][counter], count, "{counter}");
            }
            assert_eq!(merged["enabled"], false);
            assert_eq!(merged["last_restore"]["segments_selected"], 0);
        }
        assert_eq!(merge_backup(&json!({})), defaults());
        let missing = merge_backup(&json!({"last_restore": {"status": "ok"}}));
        for counter in [
            "files_expected",
            "files_restored",
            "bytes_expected",
            "bytes_restored",
        ] {
            assert_eq!(missing["last_restore"][counter], 0, "{counter}");
        }
        assert_eq!(missing["last_restore"]["status"], "ok");
    }
}
