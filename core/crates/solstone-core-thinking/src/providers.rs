// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thinking-provider read projections.

use std::env;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_brain::derive_active_brain_lane;
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};

use crate::{brain, local};

const CLOUD: [(&str, &str); 3] = [
    ("google", "GOOGLE_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("anthropic", "ANTHROPIC_API_KEY"),
];

pub fn keys(config: &Map<String, Value>) -> Value {
    let env = config.get("env").and_then(Value::as_object);
    let api_keys = Map::from_iter(CLOUD.map(|(provider, key)| {
        (
            provider.to_owned(),
            Value::Bool(
                env.and_then(|values| values.get(key))
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
                    || env::var(key).is_ok_and(|value| !value.trim().is_empty()),
            ),
        )
    }));
    json!({"env":{"GOOGLE_API_KEY":api_keys["google"],"OPENAI_API_KEY":api_keys["openai"],"ANTHROPIC_API_KEY":api_keys["anthropic"]},"api_keys":api_keys,"key_validation":config.get("providers").and_then(Value::as_object).and_then(|providers| providers.get("key_validation")).cloned().unwrap_or_else(|| json!({}))})
}

pub fn payload(journal: &Path, config: &Map<String, Value>, local_model: &str) -> Value {
    let endpoint = resolve_local_endpoint(config);
    let spp_configured =
        matches!(&endpoint, LocalEndpointResolution::Byo(value) if value.is_confidential);
    let brain_view = brain::presentation(journal, config, spp_configured);
    let active = active(config);
    let selected = active["provider"] == "local";
    let local_status = local_status(journal, &brain_view, selected, &endpoint);
    let key_payload = keys(config);
    let mut status = Map::new();
    for (provider, env_key) in CLOUD {
        let configured = key_payload["api_keys"][provider].as_bool().unwrap_or(false);
        let ready = configured;
        status.insert(provider.to_owned(), json!({"provider":provider,"configured":configured,"generate_ready":ready,"cogitate_ready":ready,"issues":if configured { Vec::<String>::new() } else { vec![format!("{env_key} not set")] }}));
    }
    status.insert("local".to_owned(), local_status.clone());
    let endpoint_view = match endpoint {
        LocalEndpointResolution::Bundled => {
            json!({"enabled":false,"endpoint_url":"","served_model_id":"","credential_configured":false})
        }
        LocalEndpointResolution::Byo(value) => {
            json!({"enabled":true,"endpoint_url":value.base_url,"served_model_id":value.served_model_id,"credential_configured":value.credential.is_some()})
        }
    };
    json!({
        "providers":[{"name":"google","label":"Google (Gemini)","env_key":"GOOGLE_API_KEY"},{"name":"openai","label":"OpenAI (GPT)","env_key":"OPENAI_API_KEY"},{"name":"anthropic","label":"Anthropic (Claude)","env_key":"ANTHROPIC_API_KEY"},{"name":"local","label":"Local (on-device)","env_key":""}],
        "api_keys":key_payload["api_keys"], "key_validation":key_payload["key_validation"], "active":active,
        "byo_models":config.get("providers").and_then(Value::as_object).and_then(|value|value.get("byo_models")).cloned().unwrap_or_else(||json!({})),
        "model_tiers":{"google":[{"tier":"mid","label":"Gemini 3.5 Flash","model":"gemini-3.5-flash"},{"tier":"lite","label":"Gemini 3.1 Flash Lite","model":"gemini-3.1-flash-lite"}],"anthropic":[{"tier":"top","label":"Claude Opus","model":"claude-opus-4-8"},{"tier":"mid","label":"Claude Sonnet","model":"claude-sonnet-5"},{"tier":"lite","label":"Claude Haiku","model":"claude-haiku-4-5"}],"openai":[{"tier":"top","label":"GPT","model":"gpt-5.5"},{"tier":"mid","label":"GPT mini","model":"gpt-5.4-mini"},{"tier":"lite","label":"GPT nano","model":"gpt-5.4-nano"}]},
        "active_lane":{"lane":ui_lane(config),"confidential_enabled":spp_configured,"confidential_provenance_configured":spp_configured,"confidential_audio":confidential_audio(config),"confidential_operation":confidential_operation(),"confidential_attestation":brain_view["confidential_attestation"]},
        "brain":brain_view["brain"],"provider_status":status,"local":local::bootstrap_status(journal, local_model),"local_runtime":local::runtime(journal),"local_override":endpoint_view,"local_backend":if cfg!(target_os="macos") {"mlx"} else {"local"},"configuration_guidance":google_exact_model_advisory(config)
    })
}

pub fn local_status_only(journal: &Path, config: &Map<String, Value>) -> Value {
    payload(journal, config, local::default_model())["provider_status"]["local"].clone()
}

pub trait ManagedKeyValidator {
    fn validate(&self, provider: &str, key: &str) -> Result<Value, String>;
}

struct UnavailableValidator;

impl ManagedKeyValidator for UnavailableValidator {
    fn validate(&self, _provider: &str, _key: &str) -> Result<Value, String> {
        Err("key validation is unavailable".to_owned())
    }
}

pub fn validate_keys(config: &Map<String, Value>) -> Value {
    // `OneShotClient::execute` accepts only a `GenerateRequest`, with no
    // provider or key override, so native per-key validation has no seam yet.
    validate_keys_with(config, &UnavailableValidator)
}

pub fn validate_keys_with(
    config: &Map<String, Value>,
    validator: &dyn ManagedKeyValidator,
) -> Value {
    let env = config.get("env").and_then(Value::as_object);
    let mut validation = Map::new();
    for (provider, key) in CLOUD {
        if let Some(value) = env
            .and_then(|values| values.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let result = validator.validate(provider, value).unwrap_or_else(
                |error| json!({"valid":false,"reason_code":"validation_unavailable","error":error}),
            );
            validation.insert(provider.to_owned(), result);
        }
    }
    json!({"key_validation":validation})
}

fn active(config: &Map<String, Value>) -> Value {
    let active = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active"))
        .and_then(Value::as_object);
    let provider = active
        .and_then(|active| active.get("provider"))
        .and_then(Value::as_str)
        .filter(|provider| !provider.is_empty())
        .unwrap_or("none");
    let model = active
        .and_then(|active| active.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| default_model_for(provider));
    json!({"provider":provider,"model":model})
}

fn default_model_for(provider: &str) -> &'static str {
    match provider {
        "google" => "gemini-3.5-flash",
        "openai" => "gpt-5.4-mini",
        "anthropic" => "claude-sonnet-4-6",
        "local" => local::default_model(),
        _ => "",
    }
}

fn confidential_audio(config: &Map<String, Value>) -> bool {
    let Some(transcribe) = config.get("transcribe").and_then(Value::as_object) else {
        return true;
    };
    match transcribe.get("confidential_audio") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => false,
    }
}

fn confidential_operation() -> Value {
    // The reference reads in-memory operation state; no native read API exists
    // yet, so this read-only projection cannot observe it.
    Value::Null
}

fn google_exact_model_advisory(config: &Map<String, Value>) -> Value {
    const PROVIDER: &str = "google";
    const PRO_ALIAS: &str = "gemini-pro-latest";
    let is_google_alias = |value: Option<&Value>| {
        value.and_then(Value::as_object).is_some_and(|profile| {
            profile.get("provider").and_then(Value::as_str) == Some(PROVIDER)
                && profile.get("model").and_then(Value::as_str) == Some(PRO_ALIAS)
        })
    };
    let mut targets = Vec::new();
    let providers = config.get("providers").and_then(Value::as_object);
    if is_google_alias(providers.and_then(|providers| providers.get("active"))) {
        targets.push("active");
    }
    if providers
        .and_then(|providers| providers.get("byo_models"))
        .and_then(Value::as_object)
        .and_then(|models| models.get(PROVIDER))
        .and_then(Value::as_str)
        == Some(PRO_ALIAS)
    {
        targets.push("remembered");
    }
    if is_google_alias(
        config
            .get("services")
            .and_then(Value::as_object)
            .and_then(|services| services.get("confidential"))
            .and_then(Value::as_object)
            .and_then(|confidential| confidential.get("prior_active")),
    ) {
        targets.push("confidential_prior");
    }
    if targets.is_empty() {
        Value::Null
    } else {
        json!({"id":"choose_exact_gemini_model","heading":"choose an exact Gemini model","google_model_resolution_targets":targets,"action":{"label":"choose model","href":"/app/thinking/#byo-setup"}})
    }
}
fn ui_lane(config: &Map<String, Value>) -> &'static str {
    match derive_active_brain_lane(config).lane.as_deref() {
        Some("bundled") => "local",
        Some("spp") => "confidential",
        Some("byo-cloud" | "byo-endpoint") => "byo",
        _ => "none",
    }
}
fn local_status(
    journal: &Path,
    brain: &Value,
    selected: bool,
    endpoint: &LocalEndpointResolution,
) -> Value {
    if brain["spp_active"] == Value::Bool(true) {
        return json!({"selected":true,"configured":true,"generate_ready":brain["spp_readiness"]["generate_ready"],"cogitate_ready":brain["spp_readiness"]["cogitate_ready"],"issues":brain["spp_readiness"]["issues"]});
    }
    let mut issues = Vec::new();
    let configured = matches!(endpoint, LocalEndpointResolution::Byo(_));
    match endpoint {
        LocalEndpointResolution::Bundled if selected => {
            let availability = local::availability(journal, local::default_model());
            if !availability["binary_present"].as_bool().unwrap_or(false) {
                issues.push("binary_missing");
            }
            if !availability["model_present"].as_bool().unwrap_or(false) {
                issues.push("model_missing");
            }
            if !issues.is_empty() {
                issues.push("run `journal install-provider local`");
            }
        }
        LocalEndpointResolution::Bundled => {}
        LocalEndpointResolution::Byo(value)
            if brain["spp_active"] != Value::Bool(true) && !reachable(&value.base_url) =>
        {
            issues.push("local_endpoint_unreachable");
        }
        LocalEndpointResolution::Byo(_) => {}
    }
    let ready = selected && brain["brain"]["state"] == "ready" && issues.is_empty();
    json!({"selected":selected,"configured":configured,"generate_ready":ready,"cogitate_ready":ready,"issues":issues})
}
fn reachable(url: &str) -> bool {
    let authority = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    let address = authority
        .parse::<SocketAddr>()
        .ok()
        .or_else(|| format!("{authority}:80").parse().ok());
    address.is_some_and(|address| {
        TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{ManagedKeyValidator, validate_keys_with};

    struct PanicValidator;

    impl ManagedKeyValidator for PanicValidator {
        fn validate(&self, _provider: &str, _key: &str) -> Result<Value, String> {
            panic!("managed-provider validation must not run without a configured key")
        }
    }

    struct FailureValidator;

    impl ManagedKeyValidator for FailureValidator {
        fn validate(&self, _provider: &str, _key: &str) -> Result<Value, String> {
            Err("provider rejected the key".to_owned())
        }
    }

    #[test]
    fn validate_keys_has_the_exact_contract_and_turns_failures_into_reason_codes() {
        assert_eq!(
            validate_keys_with(&Map::new(), &PanicValidator),
            json!({"key_validation": {}})
        );

        let config = Map::from_iter([(
            "env".to_owned(),
            json!({"OPENAI_API_KEY": "not-a-real-key"}),
        )]);
        let result = validate_keys_with(&config, &FailureValidator);
        assert_eq!(
            result
                .as_object()
                .expect("contract object")
                .keys()
                .collect::<Vec<_>>(),
            ["key_validation"]
        );
        assert_eq!(
            result["key_validation"]["openai"]["reason_code"],
            "validation_unavailable"
        );
    }

    #[test]
    #[should_panic(expected = "managed-provider validation must not run")]
    fn configured_key_reaches_the_injected_validator() {
        let config = Map::from_iter([(
            "env".to_owned(),
            json!({"OPENAI_API_KEY": "not-a-real-key"}),
        )]);
        let _ = validate_keys_with(&config, &PanicValidator);
    }
}
