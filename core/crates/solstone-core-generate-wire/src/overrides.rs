// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local overrides used only by native generate validation probes.

use serde_json::{Map, Value};

pub const API_KEY_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_API_KEY_OVERRIDE";
pub const BASE_URL_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_BASE_URL_OVERRIDE";
pub const MODEL_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_MODEL_OVERRIDE";
pub const PROVIDER_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_PROVIDER_OVERRIDE";

pub fn configured_api_key(config: &Map<String, Value>, config_key: &str) -> Option<String> {
    // Never consult conventional provider environment variables here: they may be
    // ambient host credentials. Only this dedicated child-only override may beat config.
    non_blank_process_env(API_KEY_OVERRIDE_ENV)
        .or_else(|| config_string(config, &["env", config_key]))
}

pub fn configured_model(config: &Map<String, Value>, default: &str) -> String {
    non_blank_process_env(MODEL_OVERRIDE_ENV)
        .or_else(|| config_string(config, &["providers", "active", "model"]))
        .unwrap_or_else(|| default.to_owned())
}

pub fn configured_base_url(_config: &Map<String, Value>, default: &str) -> String {
    non_blank_process_env(BASE_URL_OVERRIDE_ENV).unwrap_or_else(|| default.to_owned())
}

pub fn configured_provider(config: &Map<String, Value>) -> String {
    non_blank_process_env(PROVIDER_OVERRIDE_ENV)
        .or_else(|| config_string(config, &["providers", "active", "provider"]))
        .unwrap_or_else(|| "none".to_owned())
}

fn non_blank_process_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn config_string(config: &Map<String, Value>, path: &[&str]) -> Option<String> {
    let (first, rest) = path.split_first()?;
    let mut value = config.get(*first)?;
    for key in rest {
        value = value.as_object()?.get(*key)?;
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use serde_json::{Map, Value, json};

    use super::*;

    fn config(
        provider: Option<&str>,
        model: Option<&str>,
        key: Option<&str>,
    ) -> Map<String, Value> {
        let mut active = Map::new();
        if let Some(provider) = provider {
            active.insert("provider".into(), json!(provider));
        }
        if let Some(model) = model {
            active.insert("model".into(), json!(model));
        }
        let mut env = Map::new();
        if let Some(key) = key {
            env.insert("OPENAI_API_KEY".into(), json!(key));
        }
        Map::from_iter([
            (
                "providers".into(),
                Value::Object(Map::from_iter([("active".into(), Value::Object(active))])),
            ),
            ("env".into(), Value::Object(env)),
        ])
    }

    fn child(name: &str, environment: &[(&str, &str)]) -> bool {
        if std::env::var_os(name).is_some() {
            return false;
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(format!("overrides::tests::{name}"));
        command.env(name, "1");
        command.env_remove(API_KEY_OVERRIDE_ENV);
        command.env_remove(PROVIDER_OVERRIDE_ENV);
        command.env_remove(MODEL_OVERRIDE_ENV);
        for (key, value) in environment {
            command.env(key, value);
        }
        assert!(command.status().unwrap().success());
        true
    }

    #[test]
    fn api_override_without_config_wins() {
        if child(
            "api_override_without_config_wins",
            &[(API_KEY_OVERRIDE_ENV, "override")],
        ) {
            return;
        }
        assert_eq!(
            configured_api_key(&config(None, None, None), "OPENAI_API_KEY").as_deref(),
            Some("override")
        );
    }

    #[test]
    fn api_override_beats_config() {
        if child(
            "api_override_beats_config",
            &[(API_KEY_OVERRIDE_ENV, "override")],
        ) {
            return;
        }
        assert_eq!(
            configured_api_key(&config(None, None, Some("stored")), "OPENAI_API_KEY").as_deref(),
            Some("override")
        );
    }

    #[test]
    fn api_config_ignores_conventional_process_env() {
        if child(
            "api_config_ignores_conventional_process_env",
            &[("OPENAI_API_KEY", "ambient")],
        ) {
            return;
        }
        assert_eq!(
            configured_api_key(&config(None, None, Some("stored")), "OPENAI_API_KEY").as_deref(),
            Some("stored")
        );
    }

    #[test]
    fn provider_override_without_config_wins() {
        if child(
            "provider_override_without_config_wins",
            &[(PROVIDER_OVERRIDE_ENV, "google")],
        ) {
            return;
        }
        assert_eq!(configured_provider(&config(None, None, None)), "google");
    }

    #[test]
    fn provider_override_beats_config() {
        if child(
            "provider_override_beats_config",
            &[(PROVIDER_OVERRIDE_ENV, "google")],
        ) {
            return;
        }
        assert_eq!(
            configured_provider(&config(Some("openai"), None, None)),
            "google"
        );
    }

    #[test]
    fn provider_config_is_used_without_override() {
        if child("provider_config_is_used_without_override", &[]) {
            return;
        }
        assert_eq!(
            configured_provider(&config(Some("openai"), None, None)),
            "openai"
        );
    }

    #[test]
    fn model_override_without_config_wins() {
        if child(
            "model_override_without_config_wins",
            &[(MODEL_OVERRIDE_ENV, "candidate")],
        ) {
            return;
        }
        assert_eq!(
            configured_model(&config(None, None, None), "default"),
            "candidate"
        );
    }

    #[test]
    fn model_override_beats_config() {
        if child(
            "model_override_beats_config",
            &[(MODEL_OVERRIDE_ENV, "candidate")],
        ) {
            return;
        }
        assert_eq!(
            configured_model(&config(None, Some("stored"), None), "default"),
            "candidate"
        );
    }

    #[test]
    fn model_config_is_used_without_override() {
        if child("model_config_is_used_without_override", &[]) {
            return;
        }
        assert_eq!(
            configured_model(&config(None, Some("stored"), None), "default"),
            "stored"
        );
    }
}
