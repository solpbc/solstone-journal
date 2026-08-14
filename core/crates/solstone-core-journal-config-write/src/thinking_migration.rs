// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Locked one-time transformations of historical thinking configuration.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::{
    ConfigMutationError, JournalConfigMutation, JournalConfigTransaction, LockOptions,
    mutate_journal_config,
};

const DEFAULTS: [(&str, &str); 4] = [
    ("google", "gemini-3.5-flash"),
    ("openai", "gpt-5.4-mini"),
    ("anthropic", "claude-sonnet-4-6"),
    ("local", "local/qwen3.5-4b"),
];
const RETIRED_FIELDS: [&str; 7] = [
    "generate",
    "cogitate",
    "tier",
    "backup",
    "models",
    "google_backend",
    "vertex_credentials",
];
const LEGACY_INSTALL_FIELDS: [&str; 28] = [
    "install_state",
    "last_transition_at",
    "last_progress_at",
    "progress_bytes_received",
    "progress_bytes_total",
    "install_error",
    "binary_artifact",
    "binary_sha256",
    "binary_path",
    "model_id",
    "model_path",
    "model_sha256",
    "mmproj_path",
    "mmproj_sha256",
    "mlx_model_id",
    "mlx_revision",
    "mlx_snapshot_dir",
    "mlx_variant_dir",
    "binary_artifact_cpu",
    "binary_sha256_cpu",
    "binary_path_cpu",
    "binary_artifact_vulkan",
    "binary_sha256_vulkan",
    "binary_path_vulkan",
    "model_repo",
    "model_filename",
    "model_revision",
    "model_path",
];

fn default_model(provider: &str) -> Option<&'static str> {
    DEFAULTS
        .iter()
        .find_map(|(name, model)| (*name == provider).then_some(*model))
}

fn profile(value: Option<&Value>) -> Option<Value> {
    let value = value?.as_object()?;
    let provider = value.get("provider")?.as_str()?;
    let default = default_model(provider)?;
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(default);
    Some(json!({"provider":provider,"model":model}))
}

fn chosen_active(config: &Map<String, Value>, providers: &Map<String, Value>) -> Value {
    for field in ["active", "cogitate", "generate"] {
        if let Some(profile) = profile(providers.get(field)) {
            return profile;
        }
    }
    let env = config.get("env").and_then(Value::as_object);
    for (key, provider) in [
        ("GOOGLE_API_KEY", "google"),
        ("ANTHROPIC_API_KEY", "anthropic"),
        ("OPENAI_API_KEY", "openai"),
    ] {
        if env.and_then(|env| env.get(key)).is_some_and(truthy) {
            return json!({"provider":provider,"model":default_model(provider).unwrap()});
        }
    }
    json!({"provider":"local","model":default_model("local").unwrap()})
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
        && !matches!(value, Value::String(text) if text.is_empty())
        && !matches!(value, Value::Array(items) if items.is_empty())
        && !matches!(value, Value::Object(items) if items.is_empty())
}

fn migrate_unified(config: &mut Map<String, Value>) -> bool {
    let snapshot = config.clone();
    if !config.get("providers").is_some_and(Value::is_object) {
        config.insert("providers".into(), json!({}));
    }
    let active = {
        let providers = config["providers"].as_object().expect("provider map");
        chosen_active(config, providers)
    };
    let legacy_contexts = config["providers"]
        .as_object()
        .and_then(|providers| providers.get("contexts"))
        .and_then(Value::as_object)
        .cloned();
    let mut override_updates = Map::new();
    if let Some(contexts) = legacy_contexts {
        for (context, value) in contexts {
            let Some(value) = value.as_object() else {
                continue;
            };
            let supported = ["disabled", "extract"]
                .into_iter()
                .filter_map(|key| value.get(key).cloned().map(|value| (key.to_owned(), value)))
                .collect::<Map<_, _>>();
            if !supported.is_empty() {
                override_updates.insert(context, Value::Object(supported));
            }
        }
    }
    if !override_updates.is_empty() {
        if !config.get("talent_overrides").is_some_and(Value::is_object) {
            config.insert("talent_overrides".into(), json!({}));
        }
        let overrides = config["talent_overrides"].as_object_mut().unwrap();
        overrides.extend(override_updates);
    }
    {
        let providers = config["providers"].as_object_mut().unwrap();
        providers.insert("active".into(), active);
        providers.remove("contexts");
        for field in RETIRED_FIELDS {
            providers.remove(field);
        }
    }
    let prior = config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .and_then(Value::as_object)
        .and_then(|confidential| {
            confidential
                .get("prior_cogitate_provider")
                .or_else(|| confidential.get("prior_generate_provider"))
        })
        .and_then(Value::as_str)
        .and_then(|provider| {
            default_model(provider).map(|model| json!({"provider":provider,"model":model}))
        });
    if let Some(confidential) = config
        .get_mut("services")
        .and_then(Value::as_object_mut)
        .and_then(|services| services.get_mut("confidential"))
        .and_then(Value::as_object_mut)
    {
        if !confidential.contains_key("prior_active") {
            confidential.insert("prior_active".into(), prior.unwrap_or(Value::Null));
        }
        confidential.remove("prior_generate_provider");
        confidential.remove("prior_cogitate_provider");
    }
    let validation = config["providers"]
        .as_object_mut()
        .and_then(|providers| providers.get_mut("key_validation"))
        .and_then(Value::as_object_mut)
        .map(|validation| {
            let moved = ["revai", "plaud"]
                .into_iter()
                .filter_map(|key| validation.remove(key).map(|value| (key.to_owned(), value)))
                .collect::<Map<_, _>>();
            validation.remove("google_vertex");
            (moved, validation.is_empty())
        });
    if let Some((moved, empty)) = validation {
        if !moved.is_empty() {
            if !config
                .get("service_key_validation")
                .is_some_and(Value::is_object)
            {
                config.insert("service_key_validation".into(), json!({}));
            }
            config["service_key_validation"]
                .as_object_mut()
                .unwrap()
                .extend(moved);
        }
        if empty {
            config["providers"]
                .as_object_mut()
                .unwrap()
                .remove("key_validation");
        }
    }
    *config != snapshot
}

pub fn unify_provider_config(journal: &Path) -> Result<bool, String> {
    let transaction = mutate_journal_config(journal, LockOptions::default(), |config| {
        let changed = migrate_unified(config);
        JournalConfigMutation {
            changed,
            value: changed,
        }
    })
    .map_err(|error| error.to_string())?;
    let credentials = journal.join(".config/vertex-credentials.json");
    let removed = match fs::symlink_metadata(&credentials) {
        Ok(_) => {
            fs::remove_file(&credentials).map_err(|error| error.to_string())?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    Ok(transaction.changed || removed)
}

fn pin_profile(value: Option<&mut Value>, path: &str, lines: &mut Vec<String>) {
    let Some(profile) = value.and_then(Value::as_object_mut) else {
        return;
    };
    if profile.get("provider").and_then(Value::as_str) != Some("google") {
        return;
    }
    pin_model(profile.get_mut("model"), path, lines);
}

fn pin_model(value: Option<&mut Value>, path: &str, lines: &mut Vec<String>) {
    let Some(Value::String(model)) = value else {
        return;
    };
    let replacement = match model.as_str() {
        "gemini-flash-latest" => Some("gemini-3.5-flash"),
        "gemini-flash-lite-latest" => Some("gemini-3.1-flash-lite"),
        _ => None,
    };
    if let Some(replacement) = replacement {
        lines.push(format!("{path}: {model} -> {replacement}"));
        *model = replacement.to_owned();
    }
}

fn pro_line(
    value: Option<&Value>,
    requires_google_profile: bool,
    path: &str,
    lines: &mut Vec<String>,
) {
    let model = if requires_google_profile {
        value.and_then(Value::as_object).and_then(|profile| {
            (profile.get("provider").and_then(Value::as_str) == Some("google"))
                .then(|| profile.get("model").and_then(Value::as_str))
                .flatten()
        })
    } else {
        value.and_then(Value::as_str)
    };
    if model == Some("gemini-pro-latest") {
        lines.push(format!(
            "{path}: gemini-pro-latest -> choose exact Gemini model"
        ));
    }
}

pub fn pin_google_model_aliases(
    journal: &Path,
) -> Result<JournalConfigTransaction<Vec<String>>, ConfigMutationError> {
    mutate_journal_config(journal, LockOptions::default(), |config| {
        let mut lines = Vec::new();
        if let Some(providers) = config.get_mut("providers").and_then(Value::as_object_mut) {
            pin_profile(
                providers.get_mut("active"),
                "providers.active.model",
                &mut lines,
            );
            if let Some(models) = providers
                .get_mut("byo_models")
                .and_then(Value::as_object_mut)
            {
                pin_model(
                    models.get_mut("google"),
                    "providers.byo_models.google",
                    &mut lines,
                );
            }
        }
        if let Some(confidential) = config
            .get_mut("services")
            .and_then(Value::as_object_mut)
            .and_then(|services| services.get_mut("confidential"))
            .and_then(Value::as_object_mut)
        {
            pin_profile(
                confidential.get_mut("prior_active"),
                "services.confidential.prior_active.model",
                &mut lines,
            );
        }
        let changed = !lines.is_empty();
        let providers = config.get("providers").and_then(Value::as_object);
        pro_line(
            providers.and_then(|p| p.get("active")),
            true,
            "providers.active.model",
            &mut lines,
        );
        pro_line(
            providers
                .and_then(|p| p.get("byo_models"))
                .and_then(Value::as_object)
                .and_then(|m| m.get("google")),
            false,
            "providers.byo_models.google",
            &mut lines,
        );
        pro_line(
            config
                .get("services")
                .and_then(Value::as_object)
                .and_then(|s| s.get("confidential"))
                .and_then(Value::as_object)
                .and_then(|c| c.get("prior_active")),
            true,
            "services.confidential.prior_active.model",
            &mut lines,
        );
        JournalConfigMutation {
            changed,
            value: lines,
        }
    })
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct LegacyProviderCleanup {
    pub removed: usize,
    pub moved: usize,
}

pub fn cleanup_legacy_provider_install_config(
    journal: &Path,
    clean_local: bool,
    clean_parakeet: bool,
) -> Result<JournalConfigTransaction<LegacyProviderCleanup>, ConfigMutationError> {
    mutate_journal_config(journal, LockOptions::default(), |config| {
        let mut result = LegacyProviderCleanup::default();
        let local_vulkan = config
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut("bundled"))
            .and_then(Value::as_object_mut)
            .and_then(|bundled| bundled.get_mut("local"))
            .and_then(Value::as_object_mut)
            .and_then(|local| local.remove("vulkan_device_index"));
        if let Some(value) = local_vulkan {
            let providers = config
                .entry("providers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            let local = providers.entry("local").or_insert_with(|| json!({}));
            if let Some(local) = local.as_object_mut()
                && local.get("vulkan_device_index") != Some(&value)
            {
                local.insert("vulkan_device_index".into(), value);
                result.moved += 1;
            }
            result.removed += 1;
        }
        if let Some(bundled) = config
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut("bundled"))
            .and_then(Value::as_object_mut)
        {
            for (provider, clean) in [("local", clean_local), ("parakeet", clean_parakeet)] {
                if !clean {
                    continue;
                }
                if let Some(record) = bundled.get_mut(provider).and_then(Value::as_object_mut) {
                    for field in LEGACY_INSTALL_FIELDS {
                        if record.remove(field).is_some() {
                            result.removed += 1;
                        }
                    }
                }
            }
        }
        JournalConfigMutation {
            changed: result.removed > 0 || result.moved > 0,
            value: result,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn write_config(journal: &Path, value: Value) -> std::path::PathBuf {
        let path = journal.join("config/journal.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        path
    }

    #[test]
    fn provider_unification_preserves_unrelated_data_and_is_idempotent() {
        let journal = TempDir::new();
        let path = write_config(
            journal.path(),
            json!({
                "unknown": {"keep": true},
                "env": {"ANTHROPIC_API_KEY": "present"},
                "providers": {
                    "generate": {"provider":"google", "model":"  gemini-custom  "},
                    "contexts": {"timeline": {"disabled":true,"extract":"x","ignored":1}},
                    "key_validation": {"revai":{"ok":true},"google_vertex":{"old":true}}
                },
                "services":{"confidential":{"prior_generate_provider":"openai"}}
            }),
        );
        let credentials = journal.path().join(".config/vertex-credentials.json");
        fs::create_dir_all(credentials.parent().unwrap()).unwrap();
        fs::write(&credentials, b"secret").unwrap();

        assert!(unify_provider_config(journal.path()).unwrap());
        let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["unknown"], json!({"keep":true}));
        assert_eq!(
            stored["providers"]["active"],
            json!({"provider":"google","model":"gemini-custom"})
        );
        assert_eq!(
            stored["talent_overrides"]["timeline"],
            json!({"disabled":true,"extract":"x"})
        );
        assert_eq!(
            stored["services"]["confidential"]["prior_active"],
            json!({"provider":"openai","model":"gpt-5.4-mini"})
        );
        assert_eq!(
            stored["service_key_validation"]["revai"],
            json!({"ok":true})
        );
        assert!(!credentials.exists());
        let before = fs::read(&path).unwrap();
        assert!(!unify_provider_config(journal.path()).unwrap());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn google_alias_pin_is_exact_and_pro_alias_is_advisory_only() {
        let journal = TempDir::new();
        let path = write_config(
            journal.path(),
            json!({
                "providers":{
                    "active":{"provider":"google","model":"gemini-flash-latest"},
                    "byo_models":{"google":"gemini-pro-latest"}
                },
                "services":{"confidential":{"prior_active":{"provider":"anthropic","model":"gemini-flash-latest"}}}
            }),
        );
        let result = pin_google_model_aliases(journal.path()).unwrap();
        assert_eq!(
            result.value,
            [
                "providers.active.model: gemini-flash-latest -> gemini-3.5-flash",
                "providers.byo_models.google: gemini-pro-latest -> choose exact Gemini model",
            ]
        );
        let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["providers"]["active"]["model"], "gemini-3.5-flash");
        assert_eq!(
            stored["providers"]["byo_models"]["google"],
            "gemini-pro-latest"
        );
        assert_eq!(
            stored["services"]["confidential"]["prior_active"]["model"],
            "gemini-flash-latest"
        );
    }
}
