// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thinking-provider read projections.

use std::env;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_brain::derive_active_brain_lane;
use solstone_core_facets::append_action_log;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient,
};
use solstone_core_generate_wire::overrides::{
    API_KEY_OVERRIDE_ENV, MODEL_OVERRIDE_ENV, PROVIDER_OVERRIDE_ENV,
};
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};

use crate::{MutationError, brain, local, read_config};

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

pub fn payload(
    journal: &Path,
    config: &Map<String, Value>,
    local_model: &str,
    confidential_operation: Value,
) -> Value {
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
        "active_lane":{"lane":ui_lane(config),"confidential_enabled":spp_configured,"confidential_provenance_configured":spp_configured,"confidential_audio":confidential_audio(config),"confidential_operation":confidential_operation,"confidential_attestation":brain_view["confidential_attestation"]},
        "brain":brain_view["brain"],"provider_status":status,"local":local::bootstrap_status(journal, local_model),"local_runtime":local::runtime(journal),"local_override":endpoint_view,"local_backend":if cfg!(target_os="macos") {"mlx"} else {"local"},"configuration_guidance":google_exact_model_advisory(config)
    })
}

pub fn local_status_only(
    journal: &Path,
    config: &Map<String, Value>,
    confidential_operation: Value,
) -> Value {
    payload(
        journal,
        config,
        local::default_model(),
        confidential_operation,
    )["provider_status"]["local"]
        .clone()
}

pub trait ManagedKeyValidator {
    fn validate(&self, provider: &str, key: &str) -> Result<Value, String>;
}

pub struct UnavailableValidator;

impl ManagedKeyValidator for UnavailableValidator {
    fn validate(&self, _provider: &str, _key: &str) -> Result<Value, String> {
        Err("key validation is unavailable".to_owned())
    }
}

/// Native one-shot validation whose candidate credentials are child-only
/// environment overrides, never GenerateRequest data.
pub struct OneShotKeyValidator {
    client: OneShotClient,
}

impl OneShotKeyValidator {
    pub fn sibling() -> Result<Self, ClientError> {
        Ok(Self {
            client: OneShotClient::sibling()?,
        })
    }

    pub fn at_path(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            client: OneShotClient::at_path(path),
        }
    }

    pub fn validate_model(&self, provider: &str, model: &str, key: &str) -> Result<Value, String> {
        Ok(classify_model_probe(self.probe(provider, Some(model), key)))
    }

    fn probe(
        &self,
        provider: &str,
        model: Option<&str>,
        key: &str,
    ) -> Result<GenerateResponse, ClientError> {
        let mut client = self.client.clone().with_env(API_KEY_OVERRIDE_ENV, key);
        if let Some(model) = model {
            client = client
                .with_env(PROVIDER_OVERRIDE_ENV, provider)
                .with_env(MODEL_OVERRIDE_ENV, model);
        }
        client.execute(&validation_request())
    }
}

/// Key validation accepts `model_not_found` and `provider_quota_exceeded`:
/// either response proves the provider accepted the credential, even though
/// the canned request cannot establish that a particular model is available.
/// Model validation rejects those outcomes because it is the definitive,
/// model-specific probe. This mirrors `solstone/think/cogitate_client.py`.
fn classify_key_probe(result: Result<GenerateResponse, ClientError>) -> Value {
    match result {
        Ok(GenerateResponse::Generated(_)) => json!({"valid":true}),
        Ok(GenerateResponse::Refused(refusal)) => {
            let reason_code = refusal
                .reason_code
                .as_ref()
                .map(|value| value.as_wire().to_owned())
                .unwrap_or_else(|| "provider_response_invalid".to_owned());
            if matches!(
                reason_code.as_str(),
                "model_not_found" | "provider_quota_exceeded"
            ) {
                json!({"valid":true,"probe_reason_code":reason_code})
            } else {
                json!({"valid":false,"reason_code":reason_code,"error":refusal.detail})
            }
        }
        Err(error) => client_failure(error),
    }
}

fn classify_model_probe(result: Result<GenerateResponse, ClientError>) -> Value {
    match result {
        Ok(GenerateResponse::Generated(_)) => json!({"valid":true}),
        Ok(GenerateResponse::Refused(refusal)) => {
            let reason_code = refusal
                .reason_code
                .as_ref()
                .map(|value| value.as_wire().to_owned())
                .unwrap_or_else(|| "provider_response_invalid".to_owned());
            json!({"valid":false,"reason_code":reason_code,"error":refusal.detail})
        }
        Err(error) => client_failure(error),
    }
}

fn validation_request() -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "settings.cloud.validate_key".to_owned(),
        contents: vec![ContentPart::Text {
            text: "Reply with the single word OK.".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 512,
        thinking_budget: Some(0),
        timeout_s: Some(30.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: Some(0),
    }
}

fn client_failure(error: ClientError) -> Value {
    match error {
        ClientError::Resolve(error) => {
            json!({"valid":false,"reason_code":"validation_unavailable","error":error})
        }
        ClientError::Io(error) => {
            json!({"valid":false,"reason_code":"validation_unavailable","error":error})
        }
        ClientError::Protocol(error) => {
            json!({"valid":false,"reason_code":error.reason,"error":error.detail})
        }
        ClientError::Decode(error) => {
            json!({"valid":false,"reason_code":"validation_unavailable","error":error})
        }
    }
}

impl ManagedKeyValidator for OneShotKeyValidator {
    fn validate(&self, provider: &str, key: &str) -> Result<Value, String> {
        Ok(classify_key_probe(self.probe(provider, None, key)))
    }
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

pub fn save_key(
    journal: &Path,
    env_var: &str,
    provider: &str,
    value: &str,
    validation: Option<Value>,
) -> Result<Value, MutationError> {
    let value = value.trim().to_owned();
    let transaction = mutate_journal_config(journal, Default::default(), |config| {
        let old_value = config
            .get("env")
            .and_then(Value::as_object)
            .and_then(|env| env.get(env_var))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let prior_validation = config
            .get("providers")
            .and_then(Value::as_object)
            .and_then(|providers| providers.get("key_validation"))
            .and_then(Value::as_object)
            .and_then(|validations| validations.get(provider))
            .cloned();
        if value.is_empty() {
            object_at(config, "env").remove(env_var);
            object_at(object_at(config, "providers"), "key_validation").remove(provider);
            if let Some(byo_models) = config
                .get_mut("providers")
                .and_then(Value::as_object_mut)
                .and_then(|providers| providers.get_mut("byo_models"))
                .and_then(Value::as_object_mut)
            {
                byo_models.remove(provider);
            }
        } else {
            object_at(config, "env").insert(env_var.to_owned(), Value::String(value.clone()));
            object_at(object_at(config, "providers"), "key_validation").insert(
                provider.to_owned(),
                validation
                    .clone()
                    .expect("nonblank keys have a validation result"),
            );
        }
        let next_validation = config
            .get("providers")
            .and_then(Value::as_object)
            .and_then(|providers| providers.get("key_validation"))
            .and_then(Value::as_object)
            .and_then(|validations| validations.get(provider))
            .cloned();
        let changed = old_value.as_deref() != (!value.is_empty()).then_some(value.as_str())
            || prior_validation != next_validation;
        JournalConfigMutation {
            changed,
            value: (old_value, keys(config)),
        }
    })
    .map_err(MutationError::config)?;
    if transaction.value.0.as_deref() != (!value.is_empty()).then_some(value.as_str()) {
        append_action_log(
            journal,
            None,
            "app",
            "thinking",
            "env_update",
            json!({"changed_fields": {env_var: {"old":"***", "new":"***"}}}),
        )
        .map_err(|error| MutationError::ActionLog(error.to_string()))?;
    }
    Ok(
        json!({"success":true,"env_var":env_var,"set":!value.is_empty(),"validation":validation,"api_keys":transaction.value.1["api_keys"],"env":transaction.value.1["env"],"key_validation":transaction.value.1["key_validation"]}),
    )
}

pub fn persist_key_validations(
    journal: &Path,
    validator: &dyn ManagedKeyValidator,
) -> Result<Value, MutationError> {
    let snapshot = read_config(journal).map_err(MutationError::Read)?;
    let env = snapshot.get("env").and_then(Value::as_object);
    let snapshots = CLOUD.map(|(provider, env_var)| {
        (
            provider,
            env.and_then(|values| values.get(env_var))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned(),
        )
    });
    let computed = Map::from_iter(snapshots.iter().filter(|(_, value)| !value.is_empty()).map(
        |(provider, value)| {
            let result = validator.validate(provider, value).unwrap_or_else(
                |error| json!({"valid":false,"reason_code":"validation_unavailable","error":error}),
            );
            let mut result = result.as_object().cloned().unwrap_or_default();
            result.insert(
                "timestamp".to_owned(),
                Value::String(Utc::now().to_rfc3339()),
            );
            (provider.to_string(), Value::Object(result))
        },
    ));
    let transaction = mutate_journal_config(journal, Default::default(), |config| {
        let current_env = config.get("env").and_then(Value::as_object).cloned();
        let existing = object_at(object_at(config, "providers"), "key_validation");
        let mut changed = false;
        for ((provider, env_var), (_, snapshot_value)) in CLOUD.into_iter().zip(snapshots) {
            let current = current_env
                .as_ref()
                .and_then(|env| env.get(env_var))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if current != snapshot_value {
                continue;
            }
            match computed.get(provider) {
                Some(result) if existing.get(provider) != Some(result) => {
                    existing.insert(provider.to_owned(), result.clone());
                    changed = true;
                }
                None if existing.remove(provider).is_some() => changed = true,
                _ => {}
            }
        }
        let persisted = Map::from_iter(CLOUD.into_iter().filter_map(|(provider, _)| {
            existing
                .get(provider)
                .cloned()
                .map(|value| (provider.to_owned(), value))
        }));
        JournalConfigMutation {
            changed,
            value: Value::Object(persisted),
        }
    })
    .map_err(MutationError::config)?;
    Ok(json!({"success":true,"key_validation":transaction.value}))
}

#[derive(Debug, Clone)]
pub struct ProviderUpdate {
    pub lane: String,
    pub provider: String,
    pub model: Option<String>,
    pub resolution_targets: Vec<String>,
}

#[derive(Debug)]
pub enum ProviderUpdateError {
    Mutation(MutationError),
    Confidential(String),
}

pub fn update_providers(
    journal: &Path,
    update: ProviderUpdate,
    confidential_operation: Value,
) -> Result<Value, ProviderUpdateError> {
    let transaction = mutate_journal_config(journal, Default::default(), |config| {
        let confidential_active = is_confidential_active(config);
        let restore_only = confidential_active
            && update
                .resolution_targets
                .iter()
                .any(|target| target == "confidential_prior");
        if confidential_active && update.lane != "confidential" && !restore_only {
            return JournalConfigMutation {
                changed: false,
                value: Err(
                    "Turn off confidential thinking first, then switch your thinking provider."
                        .to_owned(),
                ),
            };
        }
        let effective_targets = update
            .resolution_targets
            .iter()
            .filter(|target| {
                google_alias_slots(config)
                    .iter()
                    .any(|slot| slot == *target)
            })
            .collect::<Vec<_>>();
        let providers = object_at(config, "providers");
        let before = providers.clone();
        let mut changes = Map::new();
        if let Some(model) = &update.model {
            let byo_models = object_at(providers, "byo_models");
            let old = before
                .get("byo_models")
                .and_then(Value::as_object)
                .and_then(|models| models.get(&update.provider))
                .cloned()
                .unwrap_or(Value::Null);
            if old != Value::String(model.clone()) {
                changes.insert(
                    format!("byo_models.{}", update.provider),
                    json!({"old":old,"new":model}),
                );
            }
            byo_models.insert(update.provider.clone(), Value::String(model.clone()));
        }
        if !restore_only {
            let active = object_at(providers, "active");
            let old_active = before.get("active").and_then(Value::as_object);
            let model = update.model.clone().unwrap_or_else(|| {
                old_active
                    .and_then(|active| {
                        (active.get("provider").and_then(Value::as_str)
                            == Some(update.provider.as_str()))
                        .then(|| {
                            active
                                .get("model")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned()
                        })
                    })
                    .filter(|model| !model.is_empty())
                    .unwrap_or_else(|| default_model_for(&update.provider).to_owned())
            });
            for (field, next) in [
                ("provider", Value::String(update.provider.clone())),
                ("model", Value::String(model)),
            ] {
                let old = old_active
                    .and_then(|active| active.get(field))
                    .cloned()
                    .unwrap_or(Value::Null);
                if old != next {
                    changes.insert(format!("active.{field}"), json!({"old":old,"new":next}));
                }
                active.insert(field.to_owned(), next);
            }
        }
        if let Some(model) = &update.model
            && effective_targets
                .iter()
                .any(|target| **target == "confidential_prior")
            && let Some(prior) = config
                .get_mut("services")
                .and_then(Value::as_object_mut)
                .and_then(|services| services.get_mut("confidential"))
                .and_then(Value::as_object_mut)
                .and_then(|confidential| confidential.get_mut("prior_active"))
                .and_then(Value::as_object_mut)
        {
            let old = prior.get("model").cloned().unwrap_or(Value::Null);
            if old != Value::String(model.clone()) {
                changes.insert(
                    "services.confidential.prior_active.model".to_owned(),
                    json!({"old":old,"new":model}),
                );
            }
            prior.insert("model".to_owned(), Value::String(model.clone()));
        }
        JournalConfigMutation {
            changed: !changes.is_empty(),
            value: Ok(changes),
        }
    })
    .map_err(|error| ProviderUpdateError::Mutation(MutationError::config(error)))?;
    let changes = transaction
        .value
        .map_err(ProviderUpdateError::Confidential)?;
    if !changes.is_empty() {
        append_action_log(
            journal,
            None,
            "app",
            "thinking",
            "providers_update",
            json!({"changed_fields":changes}),
        )
        .map_err(|error| {
            ProviderUpdateError::Mutation(MutationError::ActionLog(error.to_string()))
        })?;
    }
    let config = read_config(journal)
        .map_err(|error| ProviderUpdateError::Mutation(MutationError::Read(error)))?;
    Ok(payload(
        journal,
        &config,
        local::default_model(),
        confidential_operation,
    ))
}

fn object_at<'a>(parent: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !parent.get(key).is_some_and(Value::is_object) {
        parent.insert(key.to_owned(), Value::Object(Map::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object inserted")
}

fn is_confidential_active(config: &Map<String, Value>) -> bool {
    derive_active_brain_lane(config).lane.as_deref() == Some("spp")
}

fn google_alias_slots(config: &Map<String, Value>) -> Vec<&'static str> {
    let mut slots = Vec::new();
    let providers = config.get("providers").and_then(Value::as_object);
    if providers
        .and_then(|providers| providers.get("active"))
        .and_then(Value::as_object)
        .is_some_and(|active| {
            active.get("provider").and_then(Value::as_str) == Some("google")
                && active.get("model").and_then(Value::as_str) == Some("gemini-pro-latest")
        })
    {
        slots.push("active");
    }
    if providers
        .and_then(|providers| providers.get("byo_models"))
        .and_then(Value::as_object)
        .and_then(|models| models.get("google"))
        .and_then(Value::as_str)
        == Some("gemini-pro-latest")
    {
        slots.push("remembered");
    }
    if config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .and_then(Value::as_object)
        .and_then(|confidential| confidential.get("prior_active"))
        .and_then(Value::as_object)
        .is_some_and(|active| {
            active.get("provider").and_then(Value::as_str) == Some("google")
                && active.get("model").and_then(Value::as_str) == Some("gemini-pro-latest")
        })
    {
        slots.push("confidential_prior");
    }
    slots
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
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use serde_json::{Map, Value, json};

    use solstone_core_generate::{
        ClientError, ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse,
        ProtocolError, ReasonCode, ReasonCodeValue, RefusalReason, RefusedResponse,
    };
    #[cfg(unix)]
    use solstone_core_generate::{encode_one_shot_request, encode_one_shot_response};

    use super::{
        ManagedKeyValidator, OneShotKeyValidator, ProviderUpdate, classify_key_probe,
        classify_model_probe, save_key, update_providers, validate_keys_with, validation_request,
    };
    use crate::read_config;

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

    #[test]
    fn rejected_key_is_stored_with_its_validation_result() {
        let journal = temporary_journal("rejected-key", json!({}));
        let validation = json!({"valid":false,"reason_code":"invalid_key","error":"rejected"});
        save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            " rejected ",
            Some(validation.clone()),
        )
        .expect("rejected key is still persisted");
        let config = read_config(&journal).expect("config reads");
        assert_eq!(config["env"]["OPENAI_API_KEY"], "rejected");
        assert_eq!(config["providers"]["key_validation"]["openai"], validation);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn validation_request_matches_the_canned_generate_contract() {
        assert_eq!(validation_request(), expected_validation_request());
    }

    #[test]
    fn confidential_prior_target_updates_only_the_remembered_prior_model() {
        let journal = temporary_journal(
            "restore-only",
            json!({
                "providers":{"active":{"provider":"local","model":"private"},"local":{"endpoint_url":"https://private.example/v1","served_model_id":"private","credential":"secret"},"byo_models":{"google":"gemini-pro-latest"}},
                "services":{"confidential":{"endpoint_url":"https://private.example","served_model_id":"private","credential_fingerprint_sha256":"2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b" ,"prior_active":{"provider":"google","model":"gemini-pro-latest"}}}
            }),
        );
        // The exact SHA-256 for "secret" is required for the active lane to
        // resolve as confidential.
        let result = update_providers(
            &journal,
            ProviderUpdate {
                lane: "byo".to_owned(),
                provider: "google".to_owned(),
                model: Some("gemini-3.5-flash".to_owned()),
                resolution_targets: vec!["confidential_prior".to_owned()],
            },
            Value::Null,
        );
        assert!(
            result.is_ok(),
            "confidential prior update is allowed: {result:?}"
        );
        let config = read_config(&journal).expect("config reads");
        assert_eq!(config["providers"]["active"]["provider"], "local");
        assert_eq!(
            config["services"]["confidential"]["prior_active"]["model"],
            "gemini-3.5-flash"
        );
        let _ = fs::remove_dir_all(journal);
    }

    fn generated() -> GenerateResponse {
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: "OK".to_owned(),
            model: "test-model".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    fn refused(reason_code: &str) -> GenerateResponse {
        GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason: RefusalReason::ProviderResponseInvalid,
            reason_code: Some(ReasonCodeValue::Known(
                ReasonCode::new(reason_code).expect("known reason code"),
            )),
            retryable: false,
            blocking: true,
            reset_at_ms: None,
            provider: Some("test".to_owned()),
            detail: format!("refused: {reason_code}"),
        })
    }

    #[test]
    fn classify_key_generated_response_is_valid() {
        let result = classify_key_probe(Ok(generated()));
        assert_eq!(result, json!({"valid":true}));
    }

    #[test]
    fn classify_key_model_not_found_refusal_is_valid_with_probe_reason_code() {
        let result = classify_key_probe(Ok(refused("model_not_found")));
        assert_eq!(
            result,
            json!({"valid":true,"probe_reason_code":"model_not_found"})
        );
    }

    #[test]
    fn classify_key_quota_refusal_is_valid_with_probe_reason_code() {
        let result = classify_key_probe(Ok(refused("provider_quota_exceeded")));
        assert_eq!(
            result,
            json!({"valid":true,"probe_reason_code":"provider_quota_exceeded"})
        );
    }

    #[test]
    fn classify_key_other_refusal_is_invalid() {
        let result = classify_key_probe(Ok(refused("provider_key_invalid")));
        assert_eq!(result["valid"], false);
        assert_eq!(result["reason_code"], "provider_key_invalid");
        assert_eq!(result["error"], "refused: provider_key_invalid");
    }

    #[test]
    fn classify_key_transport_failure_is_invalid() {
        let result = classify_key_probe(Err(ClientError::Protocol(ProtocolError {
            id: None,
            reason: "stub_failure".to_owned(),
            detail: "one-shot stub hard failure".to_owned(),
        })));
        assert_eq!(result["valid"], false);
        assert_eq!(result["reason_code"], "stub_failure");
        assert_eq!(result["error"], "one-shot stub hard failure");
    }

    #[test]
    fn classify_model_generated_response_is_valid() {
        let result = classify_model_probe(Ok(generated()));
        assert_eq!(result, json!({"valid":true}));
    }

    #[test]
    fn classify_model_model_not_found_refusal_is_invalid() {
        let result = classify_model_probe(Ok(refused("model_not_found")));
        assert_eq!(
            result,
            json!({"valid":false,"reason_code":"model_not_found","error":"refused: model_not_found"})
        );
    }

    #[test]
    fn classify_model_quota_refusal_is_invalid() {
        let result = classify_model_probe(Ok(refused("provider_quota_exceeded")));
        assert_eq!(
            result,
            json!({"valid":false,"reason_code":"provider_quota_exceeded","error":"refused: provider_quota_exceeded"})
        );
    }

    #[test]
    fn classify_model_other_refusal_is_invalid() {
        let result = classify_model_probe(Ok(refused("provider_key_invalid")));
        assert_eq!(result["valid"], false);
        assert_eq!(result["reason_code"], "provider_key_invalid");
        assert_eq!(result["error"], "refused: provider_key_invalid");
    }

    #[test]
    fn classify_model_transport_failure_is_invalid() {
        let result = classify_model_probe(Err(ClientError::Protocol(ProtocolError {
            id: None,
            reason: "stub_failure".to_owned(),
            detail: "one-shot stub hard failure".to_owned(),
        })));
        assert_eq!(result["valid"], false);
        assert_eq!(result["reason_code"], "stub_failure");
        assert_eq!(result["error"], "one-shot stub hard failure");
    }

    // The prompt, token budget, thinking budget, retry count, and timeout are
    // pinned by `solstone/think/providers/shared.py:468-476`. The schema,
    // responsiveness, attempt index, and exclusive admission values mirror
    // defaults in `solstone/think/generate_client.py:313-319`.
    fn expected_validation_request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "settings.cloud.validate_key".to_owned(),
            contents: vec![ContentPart::Text {
                text: "Reply with the single word OK.".to_owned(),
            }],
            system_instruction: None,
            temperature: 0.0,
            max_output_tokens: 512,
            thinking_budget: Some(0),
            timeout_s: Some(30.0),
            json_output: false,
            json_schema: None,
            enforce_responsiveness: true,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: Some(0),
        }
    }

    #[cfg(unix)]
    struct ValidationStub {
        root: std::path::PathBuf,
        executable: std::path::PathBuf,
        raw_request: std::path::PathBuf,
        environment: std::path::PathBuf,
    }

    #[cfg(unix)]
    impl ValidationStub {
        fn new(name: &str, reason_code: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("solstone-thinking-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("stub directory creates");
            let executable = root.join("stub.sh");
            let raw_request = root.join("request.json");
            let environment = root.join("environment.txt");
            let response =
                encode_one_shot_response(&refused(reason_code)).expect("stub response encodes");
            let script = format!(
                "#!/bin/sh\ncat > {}\nenv > {}\ncat <<'SOLSTONE_RESPONSE'\n{}\nSOLSTONE_RESPONSE\n",
                shell_quote(&raw_request),
                shell_quote(&environment),
                response,
            );
            fs::write(&executable, script).expect("stub script writes");
            let mut permissions = fs::metadata(&executable)
                .expect("stub metadata reads")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).expect("stub script becomes executable");
            Self {
                root,
                executable,
                raw_request,
                environment,
            }
        }

        fn records(&self) -> (Vec<u8>, String) {
            (
                fs::read(&self.raw_request).expect("raw request records"),
                fs::read_to_string(&self.environment).expect("environment records"),
            )
        }
    }

    #[cfg(unix)]
    fn shell_quote(path: &std::path::Path) -> String {
        format!(
            "'{}'",
            path.display().to_string().replace('\'', "'\\\"'\\\"'")
        )
    }

    #[cfg(unix)]
    fn assert_environment_line(environment: &str, name: &str, value: &str) {
        assert!(
            environment
                .lines()
                .any(|line| line == format!("{name}={value}")),
            "environment includes {name}"
        );
    }

    #[cfg(unix)]
    fn assert_split_probe(reason_code: &str) -> (Value, Value) {
        const KEY: &str = "sk-candidate-not-in-request";
        let stub = ValidationStub::new(reason_code, reason_code);
        let validator = OneShotKeyValidator::at_path(&stub.executable);
        let key_result = validator.validate("openai", KEY).expect("key probe runs");
        let (key_request, key_environment) = stub.records();
        let model_result = validator
            .validate_model("openai", "candidate-model", KEY)
            .expect("model probe runs");
        let (model_request, model_environment) = stub.records();
        let expected = encode_one_shot_request(&expected_validation_request())
            .expect("expected request encodes");
        for request in [&key_request, &model_request] {
            assert!(
                !request
                    .windows(KEY.len())
                    .any(|window| window == KEY.as_bytes()),
                "candidate key is absent from raw request"
            );
            assert_eq!(request.as_slice(), expected.as_bytes());
        }
        assert_environment_line(&key_environment, "SOLSTONE_GENERATE_API_KEY_OVERRIDE", KEY);
        assert_environment_line(
            &model_environment,
            "SOLSTONE_GENERATE_API_KEY_OVERRIDE",
            KEY,
        );
        assert_environment_line(
            &model_environment,
            "SOLSTONE_GENERATE_PROVIDER_OVERRIDE",
            "openai",
        );
        assert_environment_line(
            &model_environment,
            "SOLSTONE_GENERATE_MODEL_OVERRIDE",
            "candidate-model",
        );
        let _ = fs::remove_dir_all(stub.root);
        (key_result, model_result)
    }

    #[cfg(unix)]
    #[test]
    fn key_probe_accepts_model_not_found_but_model_probe_rejects_it() {
        let (key_result, model_result) = assert_split_probe("model_not_found");
        assert_eq!(key_result["valid"], true);
        assert_eq!(key_result["probe_reason_code"], "model_not_found");
        assert_eq!(model_result["valid"], false);
        assert_eq!(model_result["reason_code"], "model_not_found");
    }

    #[cfg(unix)]
    #[test]
    fn key_probe_accepts_quota_but_model_probe_rejects_it() {
        let (key_result, model_result) = assert_split_probe("provider_quota_exceeded");
        assert_eq!(key_result["valid"], true);
        assert_eq!(key_result["probe_reason_code"], "provider_quota_exceeded");
        assert_eq!(model_result["valid"], false);
        assert_eq!(model_result["reason_code"], "provider_quota_exceeded");
    }

    fn temporary_journal(name: &str, config: Value) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("solstone-thinking-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("config")).expect("config directory creates");
        fs::write(
            path.join("config/journal.json"),
            serde_json::to_vec(&config).expect("config serializes"),
        )
        .expect("config writes");
        path
    }
}
