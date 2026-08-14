// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use crate::defaults::backup_defaults;

pub fn merge_backup_config(config: &Map<String, Value>) -> Map<String, Value> {
    let raw = config.get("backup").and_then(Value::as_object);
    merge_defaults(&backup_defaults(), raw)
}

pub(crate) fn merge_defaults(
    defaults: &Map<String, Value>,
    raw: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut merged = defaults.clone();
    let Some(raw) = raw else {
        return merged;
    };
    for (key, value) in raw {
        if let (Some(Value::Object(default)), Value::Object(value)) = (merged.get(key), value) {
            merged.insert(
                key.clone(),
                Value::Object(merge_defaults(default, Some(value))),
            );
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn partial_legacy_backup_deep_merges_populated_nested_values() {
        let config = serde_json::from_value(json!({
            "backup": {
                "enabled": true,
                "destination": {"repository": "s3:bucket/repo"},
                "retention": {"daily": 99},
                "last_offload": {"status": "ok", "files_marked": 42}
            }
        }))
        .unwrap();

        let merged = merge_backup_config(&config);
        assert_eq!(
            merged.keys().collect::<Vec<_>>(),
            backup_defaults().keys().collect::<Vec<_>>()
        );
        assert_eq!(merged["enabled"], true);
        assert_eq!(merged["destination"]["repository"], "s3:bucket/repo");
        assert_eq!(merged["destination"]["backend"], Value::Null);
        assert_eq!(merged["retention"]["daily"], 99);
        assert_eq!(merged["retention"]["hourly"], 24);
        assert_eq!(merged["last_offload"]["status"], "ok");
        assert_eq!(merged["last_offload"]["files_marked"], 42);
        assert_eq!(merged["last_offload"]["bytes_marked"], 0);
    }

    #[test]
    fn scalar_and_array_replace_default_objects() {
        let config = serde_json::from_value(json!({
            "backup": {"retention": 7, "destination": []}
        }))
        .unwrap();

        let merged = merge_backup_config(&config);
        assert_eq!(merged["retention"], 7);
        assert_eq!(merged["destination"], json!([]));
    }

    #[test]
    fn unknown_backup_keys_survive_merge() {
        let config = serde_json::from_value(json!({
            "backup": {"future_backup_field": {"populated": true}}
        }))
        .unwrap();

        let merged = merge_backup_config(&config);
        assert_eq!(merged["future_backup_field"], json!({"populated": true}));
    }

    #[test]
    fn fresh_and_legacy_empty_shapes_merge_independently() {
        let fresh = merge_backup_config(&Map::new());
        let legacy = merge_backup_config(&serde_json::from_value(json!({"backup": {}})).unwrap());
        assert_eq!(fresh, backup_defaults());
        assert_eq!(legacy, backup_defaults());
    }
}
