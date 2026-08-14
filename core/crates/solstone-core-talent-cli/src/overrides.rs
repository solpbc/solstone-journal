// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::discovery::TalentConfig;

pub(crate) fn merge(configs: &mut [TalentConfig], journal_root: &Path) -> Result<(), String> {
    let path = journal_root.join("config/journal.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let config: Value = serde_json::from_str(&text)
        .map_err(|_| format!("failed to read talent overrides from {}", path.display()))?;
    let contexts = config.get("talent_overrides").and_then(Value::as_object);
    for config in configs {
        let context = context_key(&config.key);
        let Some(override_value) = contexts
            .and_then(|contexts| contexts.get(&context))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for field in ["disabled", "extract"] {
            if let Some(value) = override_value.get(field) {
                config.metadata.insert(field.to_owned(), value.clone());
            }
        }
    }
    Ok(())
}

pub(crate) fn context_key(key: &str) -> String {
    match key.split_once(':') {
        Some((app, name)) => format!("talent.{app}.{name}"),
        None => format!("talent.system.{key}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_context_replaces_system_namespace() {
        assert_eq!(context_key("entities:assist"), "talent.entities.assist");
    }
}
