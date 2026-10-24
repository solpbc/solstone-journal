// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thinking-provider read projections.

use std::env;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_brain::{
    CredentialValidationWitness, FingerprintKeyMaterial, StoredCredentialBinding,
    attach_credential_binding, classify_stored_binding, derive_active_brain_lane,
    generate_fingerprint_key_at_root_checked, managed_credential_binding_hmac,
    read_fingerprint_key_material_bound, strip_credential_binding,
};
use solstone_core_facets::{append_action_log, append_journal_action_log_at_root_checked};
pub use solstone_core_generate::Cancellation;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient,
};
use solstone_core_generate_wire::overrides::{
    API_KEY_OVERRIDE_ENV, MODEL_OVERRIDE_ENV, PROVIDER_OVERRIDE_ENV,
};
use solstone_core_journal_config_write::{
    BoundConfigOutcome, JournalConfigMutation, mutate_journal_config,
    mutate_journal_config_at_root_interlocked,
};
use solstone_core_journal_io::{JournalRoot, ObjectIdentity};
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
    let mut key_validation = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("key_validation"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    strip_credential_binding(&mut key_validation);
    json!({"env":{"GOOGLE_API_KEY":api_keys["google"],"OPENAI_API_KEY":api_keys["openai"],"ANTHROPIC_API_KEY":api_keys["anthropic"]},"api_keys":api_keys,"key_validation":key_validation})
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
        status.insert(provider.to_owned(), json!({"provider":provider,"configured":configured,"generate_ready":ready,"issues":if configured { Vec::<String>::new() } else { vec![format!("{env_key} not set")] }}));
    }
    let chatgpt_status = solstone_core_chatgpt_auth::get_status(journal);
    let chatgpt_configured = chatgpt_status
        .as_ref()
        .map(|s| s.signed_in)
        .unwrap_or(false);
    let chatgpt_model_chosen = owner_model_for(config, "chatgpt").is_some();
    let chatgpt_ready = chatgpt_configured && chatgpt_model_chosen;
    let mut chatgpt_issues = Vec::new();
    if !chatgpt_configured {
        chatgpt_issues.push("sign in to ChatGPT required".to_string());
    } else if !chatgpt_model_chosen {
        chatgpt_issues.push("no model chosen".to_string());
    }
    status.insert(
        "chatgpt".to_owned(),
        json!({
            "provider": "chatgpt",
            "configured": chatgpt_configured,
            "generate_ready": chatgpt_ready,
            "issues": chatgpt_issues,
        }),
    );
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
        "providers":[{"name":"google","label":"Google (Gemini)","env_key":"GOOGLE_API_KEY"},{"name":"openai","label":"OpenAI (GPT)","env_key":"OPENAI_API_KEY"},{"name":"anthropic","label":"Anthropic (Claude)","env_key":"ANTHROPIC_API_KEY"},{"name":"chatgpt","label":"ChatGPT","env_key":""},{"name":"local","label":"Local (on-device)","env_key":""}],
        "api_keys":key_payload["api_keys"], "key_validation":key_payload["key_validation"], "active":active,
        "byo_models":config.get("providers").and_then(Value::as_object).and_then(|value|value.get("byo_models")).cloned().unwrap_or_else(||json!({})),
        // The owner's one thinking choice for their own model, normalized: 0 is off.
        "byo_thinking_budget":solstone_core_generate_wire::byo_thinking(config).budget(),
        "active_lane":{"lane":ui_lane(config),"confidential_enabled":spp_configured,"confidential_provenance_configured":spp_configured,"confidential_audio":confidential_audio(config),"confidential_operation":confidential_operation,"confidential_attestation":brain_view["confidential_attestation"]},
        "brain":brain_view["brain"],"provider_status":status,"local":local::bootstrap_status(journal, local_model),"local_runtime":local::runtime(journal),"local_override":endpoint_view,"local_backend":if cfg!(target_os="macos") {"metal"} else {"local"},"configuration_guidance":google_exact_model_advisory(config)
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

/// Production Generate validation with candidates confined to child-only
/// overrides or call-local configuration, never GenerateRequest data.
pub struct OneShotKeyValidator {
    client: ProbeClient,
}

enum ProbeClient {
    Process(OneShotClient),
    Embedded {
        journal: std::path::PathBuf,
        runtime: Box<solstone_core_generate_wire::EndpointRuntime>,
    },
}

impl OneShotKeyValidator {
    pub fn sibling() -> Result<Self, ClientError> {
        Ok(Self {
            client: ProbeClient::Process(OneShotClient::sibling()?),
        })
    }

    pub fn at_path(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            client: ProbeClient::Process(OneShotClient::at_path(path)),
        }
    }

    /// Validate candidate credentials through the production provider dispatch
    /// in an app process. Candidate values stay in this call's configuration;
    /// they never enter process environment or the persisted active selection.
    pub fn embedded(journal: std::path::PathBuf) -> Self {
        Self {
            client: ProbeClient::Embedded {
                journal,
                runtime: Box::new(solstone_core_generate_wire::EndpointRuntime::default()),
            },
        }
    }

    pub fn validate_model(&self, provider: &str, model: &str, key: &str) -> Result<Value, String> {
        Ok(classify_model_probe(self.probe(provider, model, key)))
    }

    fn probe(
        &self,
        provider: &str,
        model: &str,
        key: &str,
    ) -> Result<GenerateResponse, ClientError> {
        match &self.client {
            ProbeClient::Process(client) => client
                .clone()
                .with_env(API_KEY_OVERRIDE_ENV, key)
                .with_env(PROVIDER_OVERRIDE_ENV, provider)
                .with_env(MODEL_OVERRIDE_ENV, model)
                .execute(&validation_request()),
            ProbeClient::Embedded { journal, runtime } => {
                let env_key = match provider {
                    "google" => "GOOGLE_API_KEY",
                    "openai" => "OPENAI_API_KEY",
                    "anthropic" => "ANTHROPIC_API_KEY",
                    _ => {
                        return Err(ClientError::Decode(
                            "unsupported credential probe provider".into(),
                        ));
                    }
                };
                let mut config =
                    read_config(journal).map_err(|e| ClientError::Decode(e.to_string()))?;
                object_at(&mut config, "env").insert(env_key.into(), Value::String(key.into()));
                let active = object_at(object_at(&mut config, "providers"), "active");
                active.insert("provider".into(), Value::String(provider.into()));
                active.insert("model".into(), Value::String(model.into()));
                solstone_core_generate_wire::respond_with_config(
                    &validation_request(),
                    runtime,
                    journal,
                    &config,
                    |_| None,
                    &solstone_core_generate::Cancellation::never(),
                )
                .map_err(ClientError::Decode)
            }
        }
    }
}

/// Key validation accepts `model_not_found` and `provider_quota_exceeded`:
/// either response proves the provider accepted the credential, even though
/// the canned request cannot establish that a particular model is available.
/// Model validation rejects those outcomes because it is the definitive,
/// model-specific probe. This retains the retired Python probe contract.
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
        Ok(GenerateResponse::Cancelled) => {
            json!({"valid":false,"reason_code":"cancelled","error":"probe cancelled"})
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
        Ok(GenerateResponse::Cancelled) => {
            json!({"valid":false,"reason_code":"cancelled","error":"probe cancelled"})
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
        timeout_s: Some(30.0),
        json_output: false,
        json_schema: None,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: Some(0),
    }
}

fn client_failure(error: ClientError) -> Value {
    match error {
        ClientError::Resolve(error) | ClientError::Decode(error) => {
            json!({"valid":false,"reason_code":"validation_unavailable","error":error})
        }
        error @ (ClientError::Io { .. }
        | ClientError::ProcessIo(_)
        | ClientError::InvalidResponse(_)
        | ClientError::UnexpectedChild(_)) => {
            json!({"valid":false,"reason_code":"validation_unavailable","error":error.to_string()})
        }
        error @ ClientError::Protocol(_) => {
            let reason = match &error {
                ClientError::Protocol(failure) => failure.error.reason.clone(),
                _ => unreachable!("matched protocol error"),
            };
            json!({"valid":false,"reason_code":reason,"error":error.to_string()})
        }
    }
}

/// A model id no provider serves. Every cloud provider authenticates the key
/// before it resolves the model, so a key check against this id answers
/// `model_not_found` for a good key and an authentication failure for a bad one,
/// without depending on any real model that a provider may later retire.
const KEY_PROBE_MODEL: &str = "solstone-key-check";

impl ManagedKeyValidator for OneShotKeyValidator {
    fn validate(&self, provider: &str, key: &str) -> Result<Value, String> {
        Ok(classify_key_probe(self.probe(
            provider,
            KEY_PROBE_MODEL,
            key,
        )))
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

#[allow(clippy::too_many_arguments)]
pub fn save_key(
    journal: &Path,
    admitted: &JournalRoot,
    cancellation: &Cancellation<'_>,
    env_var: &str,
    provider: &str,
    value: &str,
    validation: Option<Value>,
    pre_probe_key: &str,
    pre_probe_entry: Option<&Value>,
    validation_witness: Option<CredentialValidationWitness>,
) -> Result<Value, MutationError> {
    let value = value.trim().to_owned();
    let prior_key = normalize_credential(pre_probe_key).to_owned();
    let identity = admitted.identity();
    let rejected = AtomicBool::new(false);
    let root = match validation_witness.as_ref() {
        Some(witness) => witness.admitted_root(),
        None if value.is_empty() => {
            Arc::new(JournalRoot::open(journal).map_err(|_| MutationError::StaleProbe)?)
        }
        None => return Err(MutationError::StaleProbe),
    };
    if root.identity() != identity {
        return Err(MutationError::StaleProbe);
    }
    if !value.is_empty() && validation.as_ref().is_some_and(definitive_key_rejection) {
        let witness = validation_witness.as_ref().unwrap();
        let receipt = retain_validation_rejection(
            journal,
            admitted,
            cancellation,
            env_var,
            provider,
            &value,
            &prior_key,
            pre_probe_entry,
            validation.as_ref().unwrap(),
            witness,
        )?
        .ok_or(MutationError::StaleProbe)?;
        if receipt.carrier() != solstone_core_brain::CredentialRejectionCarrier::Config {
            return Err(MutationError::ConfigWrite(
                "the provider rejected this key, and your journal couldn't confirm the result was saved in settings.".to_owned(),
            ));
        }
        let config = solstone_core_journal_config::read_journal_config_bound(&root)
            .map_err(|_| MutationError::StaleProbe)?
            .config
            .unwrap_or_default();
        receipt
            .checkpoint(witness)
            .map_err(|_| MutationError::StaleProbe)?;
        admit_still(admitted, identity, cancellation).map_err(|_| MutationError::StaleProbe)?;
        if stored_env(&config, env_var) != value {
            return Err(MutationError::StaleProbe);
        }
        if prior_key != value {
            let saved_entry = stored_validation(&config, provider).cloned();
            let owner_checkpoint = || {
                admit_still(admitted, identity, cancellation)?;
                if JournalRoot::open(journal)
                    .map_err(|error| error.to_string())?
                    .identity()
                    != identity
                {
                    return Err("admitted journal path changed".to_owned());
                }
                receipt
                    .checkpoint(witness)
                    .map_err(|error| error.to_string())?;
                let current = solstone_core_journal_config::read_journal_config_bound(&root)
                    .map_err(|error| error.to_string())?
                    .config
                    .unwrap_or_default();
                if stored_env(&current, env_var) != value
                    || stored_validation(&current, provider) != saved_entry.as_ref()
                {
                    return Err("credential validation changed after publication".to_owned());
                }
                Ok(())
            };
            append_journal_action_log_at_root_checked(
                &root,
                "app",
                "thinking",
                "env_update",
                json!({"changed_fields": {env_var: {"old":"***", "new":"***"}}}),
                &owner_checkpoint,
            )
            .map_err(|error| MutationError::ActionLog(error.to_string()))?;
        }
        let projected = keys(&config);
        return Ok(json!({"success":true,"env_var":env_var,"set":true,
            "validation":presented_validation(validation.as_ref().unwrap()),
            "api_keys":projected["api_keys"],"env":projected["env"],"key_validation":projected["key_validation"]}));
    }
    let binding_key = if value.is_empty() {
        None
    } else {
        Some(prepare_validation_binding_key(
            journal,
            &root,
            validation_witness.as_ref().unwrap(),
            cancellation,
        )?)
    };
    let checkpoint = || match (|| {
        if rejected.load(Ordering::SeqCst) {
            return Err("credential validation was superseded".to_owned());
        }
        admit_still(admitted, identity, cancellation)?;
        if let (Some(witness), Some(key)) = (validation_witness.as_ref(), binding_key.as_ref()) {
            witness
                .checkpoint_with_binding_key(key)
                .map_err(|error| error.to_string())?;
        }
        if JournalRoot::open(journal)
            .map_err(|error| error.to_string())?
            .identity()
            != identity
        {
            return Err("admitted journal path changed".to_owned());
        }
        Ok(())
    })() {
        Ok(()) => Ok(()),
        Err(error) => {
            rejected.store(true, Ordering::SeqCst);
            Err(error)
        }
    };
    let transaction = mutate_journal_config_at_root_interlocked(
        &root,
        Default::default(),
        &checkpoint,
        &|| {
            if let (Some(witness), Some(key)) = (validation_witness.as_ref(), binding_key.as_ref())
            {
                witness
                    .begin_positive_config_publication(key)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        },
        |config| {
            let old_value = config
                .get("env")
                .and_then(Value::as_object)
                .and_then(|env| env.get(env_var))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let stored = normalize_credential(old_value.as_deref().unwrap_or("")).to_owned();
            let prior_validation = stored_validation(config, provider).cloned();
            if !value.is_empty() {
                let installing = stored == prior_key && stored != value;
                if stored != value && stored != prior_key {
                    return JournalConfigMutation {
                        changed: false,
                        value: SaveLock::Stale,
                    };
                }
                let Some(hmac_key) = binding_key else {
                    return JournalConfigMutation {
                        changed: false,
                        value: SaveLock::Stale,
                    };
                };
                let probe = validation
                    .clone()
                    .expect("nonblank keys have a validation result");
                let Ok(mut bound) = bound_validation(
                    installing,
                    &hmac_key,
                    provider,
                    &value,
                    prior_validation.as_ref(),
                    pre_probe_entry,
                    &probe,
                ) else {
                    return JournalConfigMutation {
                        changed: false,
                        value: SaveLock::Stale,
                    };
                };
                if bound.get("valid").and_then(Value::as_bool) == Some(true)
                    && validation_witness
                        .as_ref()
                        .unwrap()
                        .prepare_successful_validation(&mut bound)
                        .is_err()
                {
                    return JournalConfigMutation {
                        changed: false,
                        value: SaveLock::Stale,
                    };
                }
                object_at(config, "env").insert(env_var.to_owned(), Value::String(value.clone()));
                object_at(object_at(config, "providers"), "key_validation")
                    .insert(provider.to_owned(), bound);
            } else {
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
            }
            let next_validation = stored_validation(config, provider).cloned();
            let changed = old_value.as_deref() != (!value.is_empty()).then_some(value.as_str())
                || prior_validation != next_validation;
            JournalConfigMutation {
                changed,
                value: SaveLock::Done {
                    previous: old_value,
                    projected: keys(config),
                },
            }
        },
    )
    .map_err(|error| {
        if rejected.load(Ordering::SeqCst) {
            MutationError::StaleProbe
        } else {
            MutationError::bound_config(error)
        }
    })?;
    if matches!(&transaction.outcome, BoundConfigOutcome::Published(facts)
        if facts.durability_uncertainty.is_some()
            || facts.observation_uncertainty.is_some()
            || facts.confirmation_refusal.is_some())
    {
        return Err(MutationError::ConfigWrite(
            "configuration publication could not be confirmed".to_owned(),
        ));
    }
    let SaveLock::Done {
        previous,
        projected,
    } = transaction.value
    else {
        return Err(MutationError::StaleProbe);
    };
    let expected_validation = validation
        .as_ref()
        .zip(binding_key.as_ref())
        .map(|(result, key)| {
            let mut bound = bind_validation(result, key, provider, &value);
            if bound.get("valid").and_then(Value::as_bool) == Some(true) {
                validation_witness
                    .as_ref()
                    .unwrap()
                    .prepare_successful_validation(&mut bound)
                    .map_err(|_| MutationError::StaleProbe)?;
            }
            Ok::<Value, MutationError>(bound)
        })
        .transpose()?;
    let owner_checkpoint = || {
        admit_still(admitted, identity, cancellation)?;
        if JournalRoot::open(journal)
            .map_err(|error| error.to_string())?
            .identity()
            != identity
        {
            return Err("admitted journal path changed".to_owned());
        }
        let current = solstone_core_journal_config::read_journal_config_bound(&root)
            .map_err(|error| error.to_string())?
            .config
            .unwrap_or_default();
        if stored_env(&current, env_var) != value {
            return Err("credential selection changed after validation".to_owned());
        }
        if stored_validation(&current, provider) != expected_validation.as_ref() {
            return Err("credential validation changed after publication".to_owned());
        }
        if let Some(key) = binding_key
            && read_fingerprint_key_material_bound(root.clone(), journal)
                .map_err(|error| error.to_string())?
                != FingerprintKeyMaterial::Ready(key)
        {
            return Err("protected fingerprint key changed after validation".to_owned());
        }
        Ok(())
    };
    if validation
        .as_ref()
        .and_then(|result| result.get("valid"))
        .and_then(Value::as_bool)
        == Some(true)
        && let (Some(witness), Some(key)) = (validation_witness, binding_key)
    {
        witness
            .retire_after_validation(&key, &owner_checkpoint)
            .map_err(|error| {
                log::warn!("credential rejection could not be retired: {error}");
                match error {
                    solstone_core_brain::WriterError::Conflict(_)
                    | solstone_core_brain::WriterError::Fingerprint(_) => MutationError::StaleProbe,
                    other => MutationError::ConfigWrite(other.to_string()),
                }
            })?;
    }
    if previous.as_deref() != (!value.is_empty()).then_some(value.as_str()) {
        if let Err(error) = append_journal_action_log_at_root_checked(
            &root,
            "app",
            "thinking",
            "env_update",
            json!({"changed_fields": {env_var: {"old":"***", "new":"***"}}}),
            &owner_checkpoint,
        ) {
            if rejected.load(Ordering::SeqCst) {
                log::warn!("credential binding action was not recorded: {error}");
                return Err(MutationError::StaleProbe);
            }
            return Err(MutationError::ActionLog(error.to_string()));
        }
    } else if let Err(error) = admit_still(admitted, identity, cancellation) {
        log::warn!("credential binding action was not recorded: {error}");
        return Err(MutationError::StaleProbe);
    }
    let validation = validation.as_ref().map(presented_validation);
    Ok(
        json!({"success":true,"env_var":env_var,"set":!value.is_empty(),"validation":validation,"api_keys":projected["api_keys"],"env":projected["env"],"key_validation":projected["key_validation"]}),
    )
}

enum SaveLock {
    Stale,
    Done {
        previous: Option<String>,
        projected: Value,
    },
}

fn definitive_key_rejection(result: &Value) -> bool {
    result.get("valid").and_then(Value::as_bool) == Some(false)
        && result.get("reason_code").and_then(Value::as_str) == Some("provider_key_invalid")
}

#[allow(clippy::too_many_arguments)]
fn retain_validation_rejection(
    journal: &Path,
    admitted: &JournalRoot,
    cancellation: &Cancellation<'_>,
    env_var: &str,
    provider: &str,
    credential: &str,
    prior_key: &str,
    pre_probe_entry: Option<&Value>,
    validation: &Value,
    witness: &CredentialValidationWitness,
) -> Result<Option<solstone_core_brain::CredentialRejectionReceipt>, MutationError> {
    let identity = admitted.identity();
    let checkpoint = || {
        admit_still(admitted, identity, cancellation)?;
        if JournalRoot::open(journal)
            .map_err(|error| error.to_string())?
            .identity()
            != identity
        {
            return Err("admitted journal path changed".to_owned());
        }
        Ok(())
    };
    let prior_matches = |config: &Map<String, Value>| {
        stored_env(config, env_var) == prior_key
            && stored_validation(config, provider) == pre_probe_entry
    };
    match solstone_core_brain::retain_credential_rejection_with_config_bound(
        journal,
        solstone_core_brain::CredentialHoldTarget::Managed {
            provider,
            credential,
        },
        witness,
        &checkpoint,
        &prior_matches,
        &|config| stored_env(config, env_var) == credential,
        &|config, entry| {
            if !prior_matches(config) {
                return None;
            }
            let mut next = config.clone();
            object_at(&mut next, "env").insert(env_var.to_owned(), json!(credential));
            object_at(object_at(&mut next, "providers"), "key_validation")
                .insert(provider.to_owned(), entry.clone());
            Some(next)
        },
        Some(validation),
    ) {
        Ok(receipt) => Ok(receipt),
        Err(solstone_core_brain::CredentialRejectionFailure::Refused(_)) => {
            Err(MutationError::StaleProbe)
        }
        Err(solstone_core_brain::CredentialRejectionFailure::Unpublished) => {
            Err(MutationError::ConfigWrite(
                "the provider rejected this key, and your journal couldn't confirm the result was saved."
                    .to_owned(),
            ))
        }
    }
}

pub fn persist_key_validations(
    journal: &Path,
    validator: &dyn ManagedKeyValidator,
) -> Result<Value, MutationError> {
    let admitted = Arc::new(JournalRoot::open(journal).map_err(|error| {
        log::warn!("credential binding was not persisted: {error}");
        MutationError::StaleProbe
    })?);
    let identity = admitted.identity();
    let cancellation = Cancellation::never();
    let snapshot = solstone_core_journal_config::read_journal_config_bound(&admitted)
        .map_err(MutationError::Read)?
        .config
        .unwrap_or_default();
    let env = snapshot.get("env").and_then(Value::as_object);
    let snapshots = CLOUD.map(|(provider, env_var)| {
        (
            provider,
            env_var,
            normalize_credential(
                env.and_then(|values| values.get(env_var))
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            )
            .to_owned(),
            stored_validation(&snapshot, provider).cloned(),
        )
    });
    let mut witnesses = std::collections::BTreeMap::new();
    for (provider, _, value, _) in &snapshots {
        if !value.is_empty() {
            let witness = solstone_core_brain::capture_credential_validation_bound(
                admitted.clone(),
                journal,
                solstone_core_brain::CredentialHoldTarget::Managed {
                    provider,
                    credential: value,
                },
            )
            .map_err(|error| {
                log::warn!("credential validation lost its local authority: {error}");
                MutationError::StaleProbe
            })?;
            witnesses.insert(*provider, witness);
        }
    }
    let mut computed = Map::new();
    let mut rejection_receipts = std::collections::BTreeMap::new();
    let mut retention_failure = None;
    for (provider, env_var, credential, entry) in snapshots
        .iter()
        .filter(|(_, _, value, _)| !value.is_empty())
    {
        // Preserve each observed negative before another blocking provider
        // probe. A storage failure must not discard other already observed
        // current negatives or claim the batch completed successfully.
        admit_still(&admitted, identity, &cancellation).map_err(|_| MutationError::StaleProbe)?;
        if JournalRoot::open(journal)
            .map_err(|_| MutationError::StaleProbe)?
            .identity()
            != identity
        {
            return Err(MutationError::StaleProbe);
        }
        let result = validator.validate(provider, credential).unwrap_or_else(
            |error| json!({"valid":false,"reason_code":"validation_unavailable","error":error}),
        );
        let mut result = result.as_object().cloned().unwrap_or_default();
        result.insert(
            "timestamp".to_owned(),
            Value::String(Utc::now().to_rfc3339()),
        );
        let result = Value::Object(result);
        computed.insert(provider.to_string(), result.clone());
        if !definitive_key_rejection(&result) {
            continue;
        }
        match retain_validation_rejection(
            journal,
            &admitted,
            &cancellation,
            env_var,
            provider,
            credential,
            credential,
            entry.as_ref(),
            &result,
            witnesses.get(provider).unwrap(),
        ) {
            Ok(Some(receipt)) => {
                if receipt.carrier() != solstone_core_brain::CredentialRejectionCarrier::Config {
                    retention_failure.get_or_insert_with(|| MutationError::ConfigWrite(
                        "the provider rejected this key, and your journal couldn't confirm the result was saved in settings.".to_owned(),
                    ));
                }
                rejection_receipts.insert(*provider, receipt);
            }
            Ok(None) => {
                // P still protects the actual rejected target. This batch
                // has no authority to publish or retire its changed provider.
                witnesses.remove(provider);
            }
            Err(error) => {
                retention_failure.get_or_insert(error);
            }
        }
    }
    if let Some(error) = retention_failure {
        return Err(error);
    }
    // Preserve bulk validation's existing skip contract. A provider changed
    // during its probe grants no publication or retirement authority; it must
    // not prevent unrelated unchanged providers from settling their results.
    let current = solstone_core_journal_config::read_journal_config_bound(&admitted)
        .map_err(|_| MutationError::StaleProbe)?
        .config
        .unwrap_or_default();
    witnesses.retain(|provider, witness| {
        if let Some(receipt) = rejection_receipts.get(provider) {
            return receipt.checkpoint(witness).is_ok();
        }
        snapshots.iter().any(|(name, env_var, credential, entry)| {
            name == provider
                && stored_env(&current, env_var) == *credential
                && stored_validation(&current, provider) == entry.as_ref()
        })
    });
    rejection_receipts.retain(|provider, _| witnesses.contains_key(provider));
    for witness in witnesses.values() {
        witness.begin_publication_budget();
    }
    let rejected = AtomicBool::new(false);
    let hmac_key = if !rejection_receipts.is_empty() {
        match read_fingerprint_key_material_bound(admitted.clone(), journal)
            .map_err(|_| MutationError::StaleProbe)?
        {
            FingerprintKeyMaterial::Ready(key) => Some(key),
            _ => return Err(MutationError::StaleProbe),
        }
    } else if let Some(witness) = witnesses.values().next() {
        Some(prepare_validation_binding_key(
            journal,
            &admitted,
            witness,
            &cancellation,
        )?)
    } else {
        None
    };
    let checkpoint = || match (|| {
        if rejected.load(Ordering::SeqCst) {
            return Err("credential validation was superseded".to_owned());
        }
        admit_still(&admitted, identity, &cancellation)?;
        if JournalRoot::open(journal)
            .map_err(|error| error.to_string())?
            .identity()
            != identity
        {
            return Err("admitted journal path changed".to_owned());
        }
        if let Some(key) = &hmac_key {
            for (provider, witness) in &witnesses {
                if let Some(receipt) = rejection_receipts.get(provider) {
                    receipt
                        .checkpoint(witness)
                        .map_err(|error| error.to_string())?;
                } else {
                    witness
                        .checkpoint_with_binding_key(key)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        Ok(())
    })() {
        Ok(()) => Ok(()),
        Err(error) => {
            rejected.store(true, Ordering::SeqCst);
            Err(error)
        }
    };
    let transaction = mutate_journal_config_at_root_interlocked(
        &admitted,
        Default::default(),
        &checkpoint,
        &|| {
            if let Some(key) = hmac_key.as_ref() {
                for (provider, witness) in &witnesses {
                    if !rejection_receipts.contains_key(provider) {
                        witness
                            .begin_positive_config_publication(key)
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            Ok(())
        },
        |config| {
            let currents = snapshots
                .iter()
                .map(|(_, env_var, _, _)| stored_env(config, env_var))
                .collect::<Vec<_>>();
            let existing = object_at(object_at(config, "providers"), "key_validation");
            let mut changed = false;
            let mut settled = Vec::new();
            for ((provider, _, snapshot_value, snapshot_entry), current) in
                snapshots.iter().zip(currents.iter())
            {
                if current != snapshot_value {
                    continue;
                }
                if rejection_receipts.contains_key(provider) {
                    // This exact pre-probe generation already published its
                    // negative result through the common retention owner.
                    continue;
                }
                if snapshot_value.is_empty() {
                    if computed.get(*provider).is_none() && existing.remove(*provider).is_some() {
                        changed = true;
                    }
                    continue;
                }
                let Some(result) = computed.get(*provider) else {
                    continue;
                };
                if !witnesses.contains_key(provider) {
                    continue;
                }
                let Some(hmac_key) = hmac_key.as_ref() else {
                    continue;
                };
                let stored_entry = existing.get(*provider);
                if !persist_allows(
                    stored_entry,
                    snapshot_entry.as_ref(),
                    result,
                    hmac_key,
                    provider,
                    snapshot_value,
                ) {
                    continue;
                }
                let mut bound = bind_validation(result, hmac_key, provider, snapshot_value);
                preserve_success_observation(stored_entry, &mut bound);
                if bound.get("valid").and_then(Value::as_bool) == Some(true)
                    && witnesses
                        .get(provider)
                        .unwrap()
                        .prepare_successful_validation(&mut bound)
                        .is_err()
                {
                    rejected.store(true, Ordering::SeqCst);
                    return JournalConfigMutation {
                        changed: false,
                        value: (Value::Object(Map::new()), Vec::new()),
                    };
                }
                if result.get("valid").and_then(Value::as_bool) == Some(true) {
                    settled.push(*provider);
                }
                if existing.get(*provider) != Some(&bound) {
                    existing.insert((*provider).to_owned(), bound);
                    changed = true;
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
                value: (Value::Object(persisted), settled),
            }
        },
    )
    .map_err(|error| {
        if rejected.load(Ordering::SeqCst) {
            MutationError::StaleProbe
        } else {
            MutationError::bound_config(error)
        }
    })?;
    if matches!(&transaction.outcome, BoundConfigOutcome::Published(facts)
        if facts.durability_uncertainty.is_some()
            || facts.observation_uncertainty.is_some()
            || facts.confirmation_refusal.is_some())
    {
        return Err(MutationError::ConfigWrite(
            "configuration publication could not be confirmed".to_owned(),
        ));
    }
    let (mut returned, settled) = transaction.value;
    for provider in settled {
        let Some(witness) = witnesses.remove(provider) else {
            continue;
        };
        let (_, env_var, credential, _) = snapshots
            .iter()
            .find(|(name, _, _, _)| *name == provider)
            .unwrap();
        let key = hmac_key.as_ref().unwrap();
        let mut expected =
            bind_validation(computed.get(provider).unwrap(), key, provider, credential);
        witness
            .prepare_successful_validation(&mut expected)
            .map_err(|_| MutationError::StaleProbe)?;
        let owner_checkpoint = || {
            admit_still(&admitted, identity, &cancellation)?;
            if JournalRoot::open(journal)
                .map_err(|error| error.to_string())?
                .identity()
                != identity
            {
                return Err("admitted journal path changed".to_owned());
            }
            let current = solstone_core_journal_config::read_journal_config_bound(&admitted)
                .map_err(|error| error.to_string())?
                .config
                .unwrap_or_default();
            if stored_env(&current, env_var) != *credential
                || stored_validation(&current, provider) != Some(&expected)
            {
                return Err("credential validation changed after publication".to_owned());
            }
            Ok(())
        };
        witness
            .retire_after_validation(key, &owner_checkpoint)
            .map_err(|error| {
                log::warn!("credential rejection could not be retired: {error}");
                match error {
                    solstone_core_brain::WriterError::Conflict(_)
                    | solstone_core_brain::WriterError::Fingerprint(_) => MutationError::StaleProbe,
                    other => MutationError::ConfigWrite(other.to_string()),
                }
            })?;
    }
    strip_credential_binding(&mut returned);
    Ok(json!({"success":true,"key_validation":returned}))
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

#[derive(Debug)]
pub enum ProviderRequestError {
    InvalidInput(String),
    /// A BYO save named no model and none is remembered for the provider.
    /// Carries the command-line guidance.
    ModelMissing(String),
    InvalidState(String),
    ConfigUnreadable(String),
    Reason {
        reason_code: &'static str,
        detail: String,
    },
}

pub fn resolve_provider_update(
    journal: &Path,
    lane: &str,
    request: &Map<String, Value>,
) -> Result<ProviderUpdate, ProviderRequestError> {
    resolve_provider_update_with(journal, lane, request, &crate::chatgpt::LiveChatGptModels)
}

pub fn resolve_provider_update_with<M: crate::chatgpt::ChatGptModelSource>(
    journal: &Path,
    lane: &str,
    request: &Map<String, Value>,
    model_source: &M,
) -> Result<ProviderUpdate, ProviderRequestError> {
    if !matches!(lane, "byo" | "confidential" | "local") {
        return Err(ProviderRequestError::InvalidInput(format!(
            "Invalid lane: {}. Must be one of: byo, confidential, local",
            lane
        )));
    }
    let config = read_config(journal)
        .map_err(|error| ProviderRequestError::ConfigUnreadable(error.to_string()))?;
    let endpoint_configured = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("local"))
        .and_then(Value::as_object)
        .is_some_and(|local| {
            local
                .get("endpoint_url")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
                && local
                    .get("served_model_id")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        });
    // `services.confidential` existing means confidential is PROVISIONED,
    // independent of whether it is the currently active lane -- this is
    // the retired Python implementation's exact provisioned-state check, not
    // its active-lane check. The activation flow can still reject after this
    // resolver approves.
    let confidential_provisioned = config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .is_some_and(Value::is_object);
    let provider = match lane {
        "confidential" if confidential_provisioned => "local".to_owned(),
        "confidential" => {
            return Err(ProviderRequestError::InvalidState(
                "confidential processing isn't on yet. turn it on first.".to_owned(),
            ));
        }
        "local" if confidential_provisioned => {
            return Err(ProviderRequestError::InvalidState(
                "turn off confidential processing first, then switch to the bundled local model."
                    .to_owned(),
            ));
        }
        "local" if endpoint_configured => {
            return Err(ProviderRequestError::InvalidState(
                "clear your own endpoint first to run the bundled local model.".to_owned(),
            ));
        }
        "local" => "local".to_owned(),
        "byo" => match request.get("provider").and_then(Value::as_str) {
            None | Some("") => {
                return Err(ProviderRequestError::InvalidInput(
                    "No BYO provider selected. Must be one of: anthropic, chatgpt, google, local, or openai"
                        .to_owned(),
                ));
            }
            Some(value @ ("anthropic" | "chatgpt" | "google" | "local" | "openai"))
                if value != "local" || endpoint_configured =>
            {
                value.to_owned()
            }
            Some("local") => {
                return Err(ProviderRequestError::InvalidState(
                    "save your own endpoint first.".to_owned(),
                ));
            }
            Some(_) => {
                return Err(ProviderRequestError::InvalidInput(
                    "Invalid provider for BYO lane. Must be one of: anthropic, chatgpt, google, local, or openai"
                        .to_owned(),
                ));
            }
        },
        _ => unreachable!(),
    };
    if lane == "byo" && provider == "chatgpt" {
        match model_source.status(journal) {
            Ok(status) => {
                if !status.signed_in {
                    let detail =
                        "ChatGPT sign-in required: run 'solstone journal thinking chatgpt sign-in'"
                            .to_owned();
                    return Err(ProviderRequestError::Reason {
                        reason_code: "chatgpt_sign_in_required",
                        detail,
                    });
                }
            }
            Err(err) => {
                let (reason_code, detail) = map_credential_error_to_reason(err);
                return Err(ProviderRequestError::Reason {
                    reason_code,
                    detail,
                });
            }
        }
    }
    let model = match request.get("model") {
        None => None,
        Some(_value)
            if lane != "byo"
                || !matches!(
                    provider.as_str(),
                    "anthropic" | "chatgpt" | "google" | "openai"
                ) =>
        {
            return Err(ProviderRequestError::InvalidInput(
                "model is only valid with cloud BYO providers: anthropic, chatgpt, google, openai."
                    .to_owned(),
            ));
        }
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Some(_) => {
            return Err(ProviderRequestError::InvalidInput(
                "model must be a non-empty string.".to_owned(),
            ));
        }
    };
    let targets = match request.get("google_model_resolution_targets") {
        None => Vec::new(),
        Some(Value::Array(values)) => {
            let mut targets = Vec::new();
            let mut unknown = Vec::new();
            for value in values {
                let Some(value) = value.as_str() else {
                    unknown.push(json_display(value));
                    continue;
                };
                if !matches!(value, "active" | "remembered" | "confidential_prior") {
                    unknown.push(value.to_owned());
                } else if !targets.iter().any(|target: &String| target == value) {
                    targets.push(value.to_owned());
                }
            }
            if !unknown.is_empty() {
                unknown.sort();
                return Err(ProviderRequestError::InvalidInput(format!(
                    "Invalid Google model resolution targets: {}. Must be one of: active, confidential_prior, remembered",
                    unknown.join(", ")
                )));
            }
            targets
        }
        Some(_) => {
            return Err(ProviderRequestError::InvalidInput(
                "google_model_resolution_targets must be a list of: active, confidential_prior, remembered"
                    .to_owned(),
            ));
        }
    };
    if request.contains_key("google_model_resolution_targets")
        && (lane != "byo" || provider != "google" || model.is_none())
    {
        return Err(ProviderRequestError::InvalidInput(
            "google_model_resolution_targets is only valid with Google BYO model saves.".to_owned(),
        ));
    }
    let model = match model {
        None if lane == "byo" && provider != "local" => {
            Some(owner_model_for(&config, &provider).ok_or_else(|| {
                let name = match provider.as_str() {
                    "anthropic" => "Anthropic",
                    "chatgpt" => "ChatGPT",
                    "google" => "Google",
                    "openai" => "OpenAI",
                    other => other,
                };
                ProviderRequestError::ModelMissing(format!(
                    "No model chosen for {name}. Set one with --model, using a model id exactly as {name} lists it."
                ))
            })?)
        }
        model => model,
    };
    if let ("byo", "chatgpt", Some(chosen_model)) = (lane, provider.as_str(), &model) {
        let models = match model_source.list(journal) {
            Ok(models) => models,
            Err(err) => {
                let (reason_code, detail) = map_credential_error_to_reason(err);
                return Err(ProviderRequestError::Reason {
                    reason_code,
                    detail,
                });
            }
        };
        if !models.iter().any(|m| m.slug == *chosen_model) {
            return Err(ProviderRequestError::Reason {
                reason_code: "model_not_found",
                detail: format!("Model '{chosen_model}' was not found in ChatGPT models list"),
            });
        }
    }
    Ok(ProviderUpdate {
        lane: lane.to_owned(),
        provider,
        model,
        resolution_targets: targets,
    })
}

fn map_credential_error_to_reason(
    err: solstone_core_chatgpt_auth::CredentialError,
) -> (&'static str, String) {
    match err {
        solstone_core_chatgpt_auth::CredentialError::SignInRequired => {
            ("chatgpt_sign_in_required", err.to_string())
        }
        solstone_core_chatgpt_auth::CredentialError::NotEligible => {
            ("chatgpt_not_eligible", err.to_string())
        }
        solstone_core_chatgpt_auth::CredentialError::Network => {
            ("network_unreachable", err.to_string())
        }
        solstone_core_chatgpt_auth::CredentialError::Unavailable => {
            ("provider_unavailable", err.to_string())
        }
        solstone_core_chatgpt_auth::CredentialError::Busy => {
            ("pipeline_unavailable", "Busy".to_owned())
        }
        solstone_core_chatgpt_auth::CredentialError::Io(kind) => ("pipeline_unavailable", kind),
        solstone_core_chatgpt_auth::CredentialError::GrantNotSaved => {
            ("pipeline_unavailable", "GrantNotSaved".to_owned())
        }
        solstone_core_chatgpt_auth::CredentialError::Storage(_) => {
            ("pipeline_unavailable", err.to_string())
        }
        solstone_core_chatgpt_auth::CredentialError::Refused(ref maybe_code) => {
            if let Some(code) = maybe_code {
                let norm = code.replace("_v2_", "_").replace("_v2", "");
                let reason_code = match norm.as_str() {
                    "subscription_sharing_invalid_user" => "chatgpt_sign_in_required",
                    "subscription_sharing_user_not_eligible"
                    | "subscription_sharing_client_not_enabled"
                    | "subscription_sharing_unsupported_capability" => "chatgpt_not_eligible",
                    "usage_limit_reached" | "rate_limit_exceeded" => "chatgpt_usage_limit",
                    _ => "provider_response_invalid",
                };
                (reason_code, code.clone())
            } else {
                ("provider_response_invalid", err.to_string())
            }
        }
        solstone_core_chatgpt_auth::CredentialError::Malformed(field) => {
            ("provider_response_invalid", field.unwrap_or_default())
        }
    }
}

fn json_display(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "None".to_owned(),
        value => value.to_string(),
    }
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
                    "turn off confidential processing first, then switch your thinking provider."
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

fn normalize_credential(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.is_empty() { "" } else { trimmed }
}

fn stored_env(config: &Map<String, Value>, env_var: &str) -> String {
    normalize_credential(
        config
            .get("env")
            .and_then(Value::as_object)
            .and_then(|env| env.get(env_var))
            .and_then(Value::as_str)
            .unwrap_or(""),
    )
    .to_owned()
}

fn stored_validation<'a>(config: &'a Map<String, Value>, provider: &str) -> Option<&'a Value> {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("key_validation"))
        .and_then(Value::as_object)
        .and_then(|entries| entries.get(provider))
}

fn bound_validation(
    installing: bool,
    hmac_key: &[u8; 32],
    provider: &str,
    credential: &str,
    stored_entry: Option<&Value>,
    pre_probe_entry: Option<&Value>,
    validation: &Value,
) -> Result<Value, ()> {
    let mut bound = bind_validation(validation, hmac_key, provider, credential);
    if installing {
        return Ok(bound);
    }
    preserve_success_observation(stored_entry, &mut bound);
    if stored_entry != pre_probe_entry {
        return Err(());
    }
    let hmac = managed_credential_binding_hmac(hmac_key, provider, credential);
    let class = classify_stored_binding(stored_entry, Some(hmac_key), Some(&hmac));
    if !matches!(
        class,
        StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt
    ) {
        return Ok(bound);
    }
    if may_replace_bound_rejection(validation) {
        return Ok(bound);
    }
    Err(())
}

fn bind_validation(
    validation: &Value,
    hmac_key: &[u8; 32],
    provider: &str,
    credential: &str,
) -> Value {
    let mut object = Map::new();
    if let Some(source) = validation.as_object() {
        for key in ["valid", "reason_code", "probe_reason_code", "timestamp"] {
            if let Some(field) = source.get(key) {
                object.insert(key.to_owned(), field.clone());
            }
        }
    }
    let hmac = managed_credential_binding_hmac(hmac_key, provider, credential);
    attach_credential_binding(&mut object, &hmac);
    Value::Object(object)
}

fn preserve_success_observation(previous: Option<&Value>, result: &mut Value) {
    if result.get("valid").and_then(Value::as_bool) == Some(true)
        || definitive_key_rejection(result)
    {
        return;
    }
    let Some(previous) = previous else {
        return;
    };
    if previous.get("binding_version") == result.get("binding_version")
        && previous.get("credential_hmac") == result.get("credential_hmac")
        && let Some(id) = previous.get("credential_observation_id")
    {
        result
            .as_object_mut()
            .unwrap()
            .insert("credential_observation_id".to_owned(), id.clone());
    }
}

fn presented_validation(validation: &Value) -> Value {
    let mut kept = Map::new();
    if let Some(source) = validation.as_object() {
        for key in ["valid", "reason_code", "probe_reason_code", "timestamp"] {
            if let Some(field) = source.get(key) {
                kept.insert(key.to_owned(), field.clone());
            }
        }
    }
    Value::Object(kept)
}

fn admit_still(
    admitted: &JournalRoot,
    identity: ObjectIdentity,
    cancellation: &Cancellation<'_>,
) -> Result<(), String> {
    admitted
        .revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    if admitted.identity() != identity {
        return Err("admitted journal root changed".to_owned());
    }
    if cancellation.is_cancelled() {
        return Err("cancelled".to_owned());
    }
    Ok(())
}

fn prepare_validation_binding_key(
    journal: &Path,
    root: &Arc<JournalRoot>,
    witness: &CredentialValidationWitness,
    cancellation: &Cancellation<'_>,
) -> Result<[u8; 32], MutationError> {
    witness.begin_publication_budget();
    let checkpoint = || {
        if cancellation.is_cancelled() {
            return Err("cancelled".to_owned());
        }
        witness.checkpoint().map_err(|error| error.to_string())
    };
    if cancellation.is_cancelled() {
        return Err(MutationError::StaleProbe);
    }
    match read_fingerprint_key_material_bound(root.clone(), journal)
        .map_err(|_| MutationError::StaleProbe)?
    {
        FingerprintKeyMaterial::Ready(key) => {
            witness
                .checkpoint_with_binding_key(&key)
                .map_err(|_| MutationError::StaleProbe)?;
            Ok(key)
        }
        FingerprintKeyMaterial::Absent => {
            generate_fingerprint_key_at_root_checked(root.clone(), journal, &checkpoint).map_err(
                |error| {
                    log::warn!("credential binding key was not created: {error}");
                    MutationError::StaleProbe
                },
            )
        }
        FingerprintKeyMaterial::Unavailable => Err(MutationError::StaleProbe),
    }
}

fn may_replace_bound_rejection(result: &Value) -> bool {
    matches!(
        (
            result.get("valid").and_then(Value::as_bool),
            result.get("reason_code").and_then(Value::as_str),
        ),
        (Some(true), _) | (Some(false), Some("provider_key_invalid"))
    )
}

fn persist_allows(
    stored_entry: Option<&Value>,
    snapshot_entry: Option<&Value>,
    result: &Value,
    hmac_key: &[u8; 32],
    provider: &str,
    credential: &str,
) -> bool {
    if stored_entry != snapshot_entry {
        return false;
    }
    let hmac = managed_credential_binding_hmac(hmac_key, provider, credential);
    let class = classify_stored_binding(stored_entry, Some(hmac_key), Some(&hmac));
    if matches!(
        class,
        StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt
    ) {
        return may_replace_bound_rejection(result);
    }
    true
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
    let configured_model = active
        .and_then(|active| active.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| default_model_for(provider));
    // One bundled local model is active on every platform. Keep reading old
    // journal selections, but never present a retired Gemma/MLX selection as
    // the model the native supervisor actually serves.
    let model = if provider == "local"
        && matches!(
            resolve_local_endpoint(config),
            LocalEndpointResolution::Bundled
        ) {
        local::default_model()
    } else {
        configured_model
    };
    json!({"provider":provider,"model":model})
}

/// The model the owner already chose for a cloud provider: the active model when
/// that provider is active, the one confidential processing is holding for it,
/// or the one remembered for it. Cloud providers
/// have no built-in default: their catalogs change faster than releases, so the
/// owner supplies the model id alongside their own key.
fn owner_model_for(config: &Map<String, Value>, provider: &str) -> Option<String> {
    let providers = config.get("providers").and_then(Value::as_object);
    fn chosen_for<'a>(profile: Option<&'a Value>, provider: &str) -> Option<&'a Value> {
        profile
            .and_then(Value::as_object)
            .filter(|profile| profile.get("provider").and_then(Value::as_str) == Some(provider))
            .and_then(|profile| profile.get("model"))
    }
    let active = chosen_for(
        providers.and_then(|providers| providers.get("active")),
        provider,
    );
    let prior = chosen_for(
        config
            .get("services")
            .and_then(|services| services.get("confidential"))
            .and_then(|confidential| confidential.get("prior_active")),
        provider,
    );
    let remembered = providers
        .and_then(|providers| providers.get("byo_models"))
        .and_then(Value::as_object)
        .and_then(|models| models.get(provider));
    [active, prior, remembered]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|model| !model.is_empty())
        .map(str::to_owned)
}

/// Only the bundled local model has a built-in default; see `owner_model_for`.
fn default_model_for(provider: &str) -> &'static str {
    match provider {
        "local" => local::default_model(),
        _ => "",
    }
}

fn confidential_audio(config: &Map<String, Value>) -> bool {
    crate::brain::confidential_audio_enabled(config)
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
pub fn ui_lane(config: &Map<String, Value>) -> &'static str {
    match derive_active_brain_lane(config).lane.as_deref() {
        Some("bundled") => "local",
        Some("spp") => "confidential",
        Some("byo-cloud" | "byo-endpoint") => "byo",
        _ => "none",
    }
}
/// The local provider's install step, named where the owner can take it. A mac has
/// no `journal` on its PATH, and on windows the thinking page is where local setup
/// is documented; its install button runs the same step.
#[cfg(any(target_os = "macos", windows))]
const LOCAL_INSTALL_ISSUE: &str = "choose \"install local model\" on the thinking page";
#[cfg(not(any(target_os = "macos", windows)))]
const LOCAL_INSTALL_ISSUE: &str = "run `solstone journal install-provider local`";

fn local_status(
    journal: &Path,
    brain: &Value,
    selected: bool,
    endpoint: &LocalEndpointResolution,
) -> Value {
    if brain["spp_active"] == Value::Bool(true) {
        return json!({"selected":true,"configured":true,"generate_ready":brain["spp_readiness"]["generate_ready"],"issues":brain["spp_readiness"]["issues"]});
    }
    let mut issues = Vec::new();
    let configured = matches!(endpoint, LocalEndpointResolution::Byo(_));
    match endpoint {
        LocalEndpointResolution::Bundled if selected => {
            let availability = local::provider_status_availability(journal, local::default_model());
            if !availability["binary_present"].as_bool().unwrap_or(false) {
                issues.push("binary_missing");
            }
            if !availability["model_present"].as_bool().unwrap_or(false) {
                issues.push("model_missing");
            }
            if !issues.is_empty() {
                issues.push(LOCAL_INSTALL_ISSUE);
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
    json!({"selected":selected,"configured":configured,"generate_ready":ready,"issues":issues})
}
fn reachable(url: &str) -> bool {
    let Some(target) = socket_target(url) else {
        return false;
    };
    // Resolve names too: an owner's endpoint is usually a hostname (a hosted
    // service, or `mybox.local`), and only an IP literal parses as a `SocketAddr`.
    let Ok(addresses) = target.to_socket_addrs() else {
        return false;
    };
    addresses
        .take(2)
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok())
}

/// The `host:port` an endpoint URL connects to, with the scheme's default port
/// when the URL names none.
fn socket_target(url: &str) -> Option<String> {
    let (rest, default_port) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, 443)
    } else {
        (url.strip_prefix("http://")?, 80)
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host_port.is_empty() {
        return None;
    }
    let has_port = match host_port.strip_prefix('[') {
        Some(bracketed) => bracketed.contains("]:"),
        None => host_port.contains(':'),
    };
    Some(if has_port {
        host_port.to_owned()
    } else {
        format!("{host_port}:{default_port}")
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    #[cfg(all(test, feature = "full-tests"))]
    use std::os::unix::fs::PermissionsExt;

    use serde_json::{Map, Value, json};

    use solstone_core_generate::{
        CapturedStream, ChildStatus, ClientError, ContentPart, GenerateRequest, GenerateResponse,
        GeneratedResponse, ProtocolError, ProtocolFailure, ReasonCode, ReasonCodeValue,
        RefusalReason, RefusedResponse,
    };
    #[cfg(unix)]
    #[cfg(all(test, feature = "full-tests"))]
    use solstone_core_generate::{encode_one_shot_request, encode_one_shot_response};

    #[cfg(all(test, feature = "full-tests"))]
    use super::OneShotKeyValidator;
    use super::{
        ManagedKeyValidator, ProviderRequestError, ProviderUpdate, ProviderUpdateError,
        classify_key_probe, classify_model_probe, resolve_provider_update, update_providers,
        validate_keys_with, validation_request,
    };

    fn save_witness(
        journal: &std::path::Path,
        provider: &str,
        value: &str,
    ) -> Result<Option<solstone_core_brain::CredentialValidationWitness>, crate::MutationError>
    {
        if value.trim().is_empty() {
            return Ok(None);
        }
        let root = std::sync::Arc::new(
            solstone_core_journal_io::JournalRoot::open(journal)
                .map_err(|_| crate::MutationError::StaleProbe)?,
        );
        solstone_core_brain::capture_credential_validation_bound(
            root,
            journal,
            solstone_core_brain::CredentialHoldTarget::Managed {
                provider,
                credential: value.trim(),
            },
        )
        .map(Some)
        .map_err(|_| crate::MutationError::StaleProbe)
    }

    fn save_key(
        journal: &std::path::Path,
        env_var: &str,
        provider: &str,
        value: &str,
        validation: Option<Value>,
        pre_probe_key: &str,
        pre_probe_entry: Option<&Value>,
    ) -> Result<Value, crate::MutationError> {
        let admitted = solstone_core_journal_io::JournalRoot::open(journal).expect("journal root");
        let cancellation = solstone_core_generate::Cancellation::never();
        let validation_witness = save_witness(journal, provider, value)?;
        super::save_key(
            journal,
            &admitted,
            &cancellation,
            env_var,
            provider,
            value,
            validation,
            pre_probe_key,
            pre_probe_entry,
            validation_witness,
        )
    }
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
    fn active_projection_normalizes_retired_local_models_to_shared_4b() {
        for retired in ["local/gemma-3-12b-it-qat", "qwen3.5:9b"] {
            let config = Map::from_iter([(
                "providers".to_owned(),
                json!({"active":{"provider":"local","model":retired}}),
            )]);
            assert_eq!(
                super::active(&config),
                json!({"provider":"local","model":"local/qwen3.5-4b"})
            );
        }
    }

    #[test]
    fn active_projection_preserves_byo_local_model_identity() {
        let config = Map::from_iter([(
            "providers".to_owned(),
            json!({
                "active":{"provider":"local","model":"private"},
                "local":{"endpoint_url":"https://private.example/v1","served_model_id":"private"}
            }),
        )]);
        assert_eq!(
            super::active(&config),
            json!({"provider":"local","model":"private"})
        );
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
        let validation =
            json!({"valid":false,"reason_code":"invalid_key","error":"provider said no"});
        let saved = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            " rejected ",
            Some(validation.clone()),
            "",
            None,
        )
        .expect("rejected key is still persisted");
        let config = read_config(&journal).expect("config reads");
        assert_eq!(config["env"]["OPENAI_API_KEY"], "rejected");
        let stored = &config["providers"]["key_validation"]["openai"];
        assert_eq!(stored["valid"], validation["valid"]);
        assert_eq!(stored["reason_code"], validation["reason_code"]);
        assert!(stored.get("error").is_none());
        assert!(stored.get("detail").is_none());
        assert!(saved["validation"].get("error").is_none());
        assert!(!stored.to_string().contains("provider said no"));
        assert!(!saved.to_string().contains("provider said no"));
        assert_eq!(stored["binding_version"], 1);
        let hmac = stored["credential_hmac"].as_str().expect("binding hmac");
        assert_eq!(hmac.len(), 64);
        assert!(
            hmac.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert!(!stored.to_string().contains("rejected"));
        assert!(
            saved["key_validation"]["openai"]
                .get("credential_hmac")
                .is_none()
        );
        assert!(
            saved["key_validation"]["openai"]
                .get("binding_version")
                .is_none()
        );
        let key_bytes =
            fs::read(solstone_core_brain::brain_fingerprint_key_path(&journal)).expect("key bytes");
        assert_eq!(key_bytes.len(), 32);
        let _ = fs::remove_dir_all(journal);
    }

    fn binding_journal(name: &str, config: Value) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "solstone-thinking-binding-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("config")).expect("config directory creates");
        fs::write(
            path.join("config/journal.json"),
            serde_json::to_vec(&config).expect("config serializes"),
        )
        .expect("config writes");
        path
    }

    fn install_openai(journal: &std::path::Path, key: &str, validation: Value) -> Value {
        save_key(
            journal,
            "OPENAI_API_KEY",
            "openai",
            key,
            Some(validation),
            "",
            None,
        )
        .expect("key installs")
    }

    #[test]
    fn save_key_mismatch_writes_nothing() {
        let journal = binding_journal("mismatch", json!({}));
        let rejection = json!({"valid":false,"reason_code":"provider_key_invalid"});
        install_openai(&journal, "alpha-key", rejection);
        let before = fs::read(journal.join("config/journal.json")).expect("config bytes");
        let error = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "gamma-key",
            None,
        )
        .expect_err("stale probe writes nothing");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config bytes"),
            before
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_key_same_key_success_clears_matching_rejection() {
        let journal = binding_journal("clear-rejection", json!({}));
        let rejection = json!({"valid":false,"reason_code":"provider_key_invalid"});
        install_openai(&journal, "alpha-key", rejection.clone());
        let stored = read_config(&journal).expect("config reads");
        let entry = stored["providers"]["key_validation"]["openai"].clone();
        let saved = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":true})),
            "alpha-key",
            Some(&entry),
        )
        .expect("matching success clears the rejection");
        let config = read_config(&journal).expect("config reads");
        assert_eq!(config["env"]["OPENAI_API_KEY"], "alpha-key");
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["valid"],
            true
        );
        assert!(
            config["providers"]["key_validation"]["openai"]
                .get("reason_code")
                .is_none()
        );
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["binding_version"],
            1
        );
        assert!(
            saved["key_validation"]["openai"]
                .get("credential_hmac")
                .is_none()
        );
        assert_eq!(saved["validation"]["valid"], true);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_key_unavailable_does_not_clear_matching_rejection() {
        let journal = binding_journal("unavailable", json!({}));
        install_openai(
            &journal,
            "alpha-key",
            json!({"valid":false,"reason_code":"provider_key_invalid"}),
        );
        let stored = read_config(&journal).expect("config reads");
        let entry = stored["providers"]["key_validation"]["openai"].clone();
        let before = fs::read(journal.join("config/journal.json")).expect("config bytes");
        let error = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":false,"reason_code":"validation_unavailable"})),
            "alpha-key",
            Some(&entry),
        )
        .expect_err("unavailable probe leaves the rejection");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config bytes"),
            before
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_key_same_key_after_committed_success_leaves_the_new_evidence() {
        let journal = binding_journal("same-key-stale", json!({}));
        install_openai(
            &journal,
            "alpha-key",
            json!({"valid":false,"reason_code":"provider_key_invalid"}),
        );
        let stored = read_config(&journal).expect("config reads");
        let old_rejection = stored["providers"]["key_validation"]["openai"].clone();
        save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":true})),
            "alpha-key",
            Some(&old_rejection),
        )
        .expect("same-key success commits");
        let committed = fs::read(journal.join("config/journal.json")).expect("config bytes");
        let config = read_config(&journal).expect("config reads");
        let committed_hmac = config["providers"]["key_validation"]["openai"]["credential_hmac"]
            .as_str()
            .expect("committed hmac")
            .to_owned();
        let error = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":true})),
            "alpha-key",
            Some(&old_rejection),
        )
        .expect_err("later probe keeps the committed success");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config bytes"),
            committed
        );
        let config = read_config(&journal).expect("config reads");
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["credential_hmac"],
            committed_hmac
        );
        let projected = super::keys(&config);
        assert!(
            projected["key_validation"]["openai"]
                .get("credential_hmac")
                .is_none()
        );
        assert!(!projected.to_string().contains("credential_hmac"));
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_key_unavailable_leaves_corrupt_claim_until_matching_success() {
        let journal = binding_journal(
            "corrupt-claim",
            json!({
                "env": {"OPENAI_API_KEY": "alpha-key"},
                "providers": {
                    "key_validation": {
                        "openai": {
                            "valid": false,
                            "reason_code": "provider_key_invalid",
                            "binding_version": 1,
                            "credential_hmac": "zzzz"
                        }
                    }
                }
            }),
        );
        assert!(!solstone_core_brain::brain_fingerprint_key_path(&journal).is_file());
        let stored = read_config(&journal).expect("config reads");
        let corrupt = stored["providers"]["key_validation"]["openai"].clone();
        let before = fs::read(journal.join("config/journal.json")).expect("config bytes");
        let error = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":false,"reason_code":"validation_unavailable"})),
            "alpha-key",
            Some(&corrupt),
        )
        .expect_err("unavailable leaves the corrupt claim");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config bytes"),
            before
        );
        let error = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":true})),
            "alpha-key",
            Some(&corrupt),
        )
        .expect_err("an absent key is not minted over an existing binding");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config bytes"),
            before
        );
        assert!(!solstone_core_brain::brain_fingerprint_key_path(&journal).exists());
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn ready_key_success_replaces_corrupt_same_key_claim() {
        let journal = binding_journal(
            "ready-corrupt",
            json!({
                "env": {"OPENAI_API_KEY": "alpha-key"},
                "providers": {
                    "key_validation": {
                        "openai": {
                            "valid": false,
                            "reason_code": "provider_key_invalid",
                            "binding_version": 1,
                            "credential_hmac": "zzzz"
                        }
                    }
                }
            }),
        );
        let key = [4u8; 32];
        let path = solstone_core_brain::brain_fingerprint_key_path(&journal);
        fs::create_dir_all(path.parent().expect("health")).expect("health");
        fs::write(&path, key).expect("ready key");
        let stored = read_config(&journal).expect("config reads");
        let corrupt = stored["providers"]["key_validation"]["openai"].clone();
        let saved = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":true,"error":"provider said no"})),
            "alpha-key",
            Some(&corrupt),
        )
        .expect("a ready key can replace a corrupt claim");
        assert_eq!(saved["success"], true);
        assert!(saved["validation"].get("error").is_none());
        assert_eq!(fs::read(&path).expect("key bytes"), key);
        let config = read_config(&journal).expect("config reads");
        let entry = &config["providers"]["key_validation"]["openai"];
        assert_eq!(entry["valid"], true);
        assert!(entry.get("error").is_none());
        assert_eq!(
            entry["credential_hmac"],
            solstone_core_brain::managed_credential_binding_hmac(&key, "openai", "alpha-key")
        );
        assert_ne!(entry["credential_hmac"], "zzzz");
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn bulk_skips_a_retained_negative_corrected_while_another_provider_is_probed() {
        let journal = binding_journal(
            "bulk-corrected-negative",
            json!({
                "env":{"OPENAI_API_KEY":"alpha-key","ANTHROPIC_API_KEY":"beta-key"},
                "providers":{"active":{"provider":"openai","model":"gpt-test"}}
            }),
        );
        struct CorrectingValidator {
            journal: std::path::PathBuf,
        }
        impl ManagedKeyValidator for CorrectingValidator {
            fn validate(&self, provider: &str, _key: &str) -> Result<Value, String> {
                match provider {
                    "openai" => Ok(json!({"valid":false,"reason_code":"provider_key_invalid"})),
                    "anthropic" => {
                        let current = read_config(&self.journal).unwrap();
                        let entry = current["providers"]["key_validation"]["openai"].clone();
                        let correction = save_key(
                            &self.journal,
                            "OPENAI_API_KEY",
                            "openai",
                            "alpha-key",
                            Some(json!({"valid":true})),
                            "alpha-key",
                            Some(&entry),
                        )
                        .unwrap();
                        assert_eq!(correction["success"], true);
                        Ok(json!({"valid":true}))
                    }
                    _ => panic!("unexpected probe"),
                }
            }
        }
        let result = super::persist_key_validations(
            &journal,
            &CorrectingValidator {
                journal: journal.clone(),
            },
        )
        .unwrap();
        assert_eq!(result["success"], true);
        let config = read_config(&journal).unwrap();
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["valid"],
            true
        );
        assert_eq!(
            config["providers"]["key_validation"]["anthropic"]["valid"],
            true
        );
        assert!(
            config["providers"]["key_validation"]["openai"]["credential_observation_id"]
                .is_string()
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn persist_skips_entry_mutated_during_validate() {
        let journal = binding_journal(
            "persist-mutated",
            json!({
                "env": {"OPENAI_API_KEY": "alpha-key"},
                "providers": {
                    "key_validation": {
                        "openai": {
                            "valid": false,
                            "reason_code": "provider_key_invalid",
                            "marker": "original"
                        }
                    }
                }
            }),
        );
        fs::create_dir_all(journal.join("health")).expect("health");
        fs::write(
            solstone_core_brain::brain_fingerprint_key_path(&journal),
            [3u8; 32],
        )
        .expect("ready key");
        struct MutatingUnavailable {
            journal: std::path::PathBuf,
        }
        impl ManagedKeyValidator for MutatingUnavailable {
            fn validate(&self, _provider: &str, _key: &str) -> Result<Value, String> {
                let mut config = read_config(&self.journal).expect("config reads");
                config
                    .get_mut("providers")
                    .and_then(Value::as_object_mut)
                    .expect("providers")
                    .get_mut("key_validation")
                    .and_then(Value::as_object_mut)
                    .expect("key_validation")
                    .insert(
                        "openai".to_owned(),
                        json!({
                            "valid": false,
                            "reason_code": "provider_key_invalid",
                            "marker": "mutated-during-validate",
                            "binding_version": 1,
                            "credential_hmac": "zzzz"
                        }),
                    );
                fs::write(
                    self.journal.join("config/journal.json"),
                    serde_json::to_vec(&config).expect("config bytes"),
                )
                .expect("mutated config writes");
                Ok(json!({"valid":false,"reason_code":"validation_unavailable"}))
            }
        }
        let saved = super::persist_key_validations(
            &journal,
            &MutatingUnavailable {
                journal: journal.clone(),
            },
        )
        .expect("a mutated provider is skipped");
        assert_eq!(saved["success"], true);
        let config = read_config(&journal).expect("config reads");
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["marker"],
            "mutated-during-validate"
        );
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["reason_code"],
            "provider_key_invalid"
        );
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["credential_hmac"],
            "zzzz"
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_key_public_json_omits_binding_disk_keeps_hmac() {
        let journal = binding_journal("public-json", json!({}));
        let key = "sk-secret-value";
        let saved = install_openai(
            &journal,
            key,
            json!({"valid":false,"reason_code":"provider_key_invalid","error":"authentication failed"}),
        );
        let public = saved.to_string();
        assert!(!public.contains("credential_hmac"));
        assert!(!public.contains("binding_version"));
        assert!(!public.contains(key));
        assert!(!public.contains("authentication failed"));
        assert!(saved["validation"].get("error").is_none());
        let config = read_config(&journal).expect("config reads");
        let stored = config["providers"]["key_validation"]["openai"].clone();
        assert_eq!(stored["binding_version"], 1);
        let hmac = stored["credential_hmac"].as_str().expect("disk hmac");
        assert_eq!(hmac.len(), 64);
        assert!(!stored.to_string().contains(key));
        assert!(stored.get("error").is_none());
        assert_eq!(config["env"]["OPENAI_API_KEY"], key);
        let projected = super::keys(&config);
        assert!(
            projected["key_validation"]["openai"]
                .get("credential_hmac")
                .is_none()
        );
        assert_eq!(
            config["providers"]["key_validation"]["openai"]["credential_hmac"],
            hmac
        );
        let logs = fs::read_dir(journal.join("config/actions")).expect("action logs");
        let mut logged = String::new();
        for entry in logs {
            logged
                .push_str(&fs::read_to_string(entry.expect("log entry").path()).expect("log text"));
        }
        assert!(logged.contains("***"));
        assert!(!logged.contains(key));
        save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "",
            None,
            key,
            Some(&stored),
        )
        .expect("empty key deletes the entry");
        let cleared = read_config(&journal).expect("config reads");
        assert!(
            cleared
                .get("env")
                .and_then(|env| env.get("OPENAI_API_KEY"))
                .is_none()
        );
        assert!(
            cleared
                .get("providers")
                .and_then(|providers| providers.get("key_validation"))
                .and_then(|entries| entries.get("openai"))
                .is_none()
        );
        let _ = fs::remove_dir_all(journal);
    }

    fn plant_ready_key(journal: &std::path::Path) {
        let path = solstone_core_brain::brain_fingerprint_key_path(journal);
        fs::create_dir_all(path.parent().expect("health")).expect("health");
        fs::write(&path, [9u8; 32]).expect("ready key");
    }

    #[test]
    fn wrong_length_fingerprint_key_is_unchanged_by_unavailable_save() {
        let journal = binding_journal(
            "wrong-length",
            json!({"env":{"OPENAI_API_KEY":"alpha-key"}}),
        );
        let path = solstone_core_brain::brain_fingerprint_key_path(&journal);
        fs::create_dir_all(path.parent().expect("health")).expect("health");
        let original = b"not-32-bytes".to_vec();
        fs::write(&path, &original).expect("short key");
        let before = fs::read(journal.join("config/journal.json")).expect("config");
        let error = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(
                json!({"valid":false,"reason_code":"validation_unavailable","error":"provider said no"}),
            ),
            "alpha-key",
            None,
        )
        .expect_err("unavailable material");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(fs::read(&path).expect("key bytes"), original);
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config"),
            before
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn fresh_same_key_success_retires_brain_and_memory_holds_for_all_credential_valid_results() {
        for reason in [
            None,
            Some("model_not_found"),
            Some("provider_quota_exceeded"),
        ] {
            let journal = binding_journal(
                "fresh-hold-retirement",
                json!({
                    "env":{"OPENAI_API_KEY":"alpha-key"},
                    "providers":{"active":{"provider":"openai","model":"gpt-binding"}}
                }),
            );
            plant_ready_key(&journal);
            let root =
                std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
            let target = solstone_core_brain::CredentialHoldTarget::Managed {
                provider: "openai",
                credential: "alpha-key",
            };
            let session =
                solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), target)
                    .unwrap();
            let marker_id = session.record_rejection().unwrap();
            let key = solstone_core_brain::load_existing_fingerprint_key(&journal).unwrap();
            solstone_core_brain::record_credential_hold_bound(
                root.clone(),
                &journal,
                "openai",
                &solstone_core_brain::CredentialRejectionHold {
                    credential_hmac: solstone_core_brain::managed_credential_binding_hmac(
                        &key,
                        "openai",
                        "alpha-key",
                    ),
                    marker_id,
                },
                &key,
                &|| Ok(()),
            )
            .unwrap();
            let witness = solstone_core_brain::capture_credential_validation_bound(
                root.clone(),
                &journal,
                target,
            )
            .unwrap();
            let mut result = json!({"valid":true});
            if let Some(reason) = reason {
                result["probe_reason_code"] = json!(reason);
            }
            super::save_key(
                &journal,
                &root,
                &super::Cancellation::never(),
                "OPENAI_API_KEY",
                "openai",
                "alpha-key",
                Some(result),
                "alpha-key",
                None,
                Some(witness),
            )
            .unwrap();
            assert!(!session.is_held().unwrap());
            let brain: Value =
                serde_json::from_slice(&fs::read(journal.join("health/brain.json")).unwrap())
                    .unwrap();
            assert_eq!(brain["credential_holds"], json!({}));
            assert_eq!(
                solstone_core_brain::classify_credential_admission_bound(
                    root, &journal, target, None
                )
                .unwrap(),
                solstone_core_brain::StoredCredentialBinding::DoesNotReject
            );
            fs::remove_dir_all(journal).unwrap();
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn first_negative_save_refuses_a_config_appearing_after_its_absent_snapshot() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let journal = binding_journal("new-negative-absent-race", json!({}));
        fs::remove_file(journal.join("config/journal.json")).unwrap();
        let root =
            std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
        let target = solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "alpha-key",
        };
        let witness = solstone_core_brain::capture_credential_validation_bound(
            root.clone(),
            &journal,
            target,
        )
        .unwrap();
        let session =
            solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), target).unwrap();
        let fired = AtomicBool::new(false);
        let external = br#"{"other_owner":"preserve this exact file"}"#;
        let check = || {
            // Key initialization follows the original absent-config read.
            // Observe that actual publication rather than counting callbacks.
            if solstone_core_brain::load_existing_fingerprint_key(&journal).is_some()
                && !fired.swap(true, Ordering::SeqCst)
            {
                fs::write(journal.join("config/journal.json"), external).unwrap();
            }
            false
        };
        let result = super::save_key(
            &journal,
            &root,
            &super::Cancellation::from_fn(&check),
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":false,"reason_code":"provider_key_invalid"})),
            "",
            None,
            Some(witness),
        );
        assert!(
            fired.load(Ordering::SeqCst),
            "actual post-key-publication checkpoint reached"
        );
        assert!(matches!(result, Err(crate::MutationError::StaleProbe)));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).unwrap(),
            external
        );
        assert!(!journal.join("config/actions").exists());
        assert!(session.is_held().unwrap());
        fs::remove_dir_all(journal).unwrap();
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn newly_entered_rejected_key_materializes_genuinely_absent_config() {
        for absent_parent in [false, true] {
            let journal = binding_journal("new-negative-absent-config", json!({}));
            fs::remove_file(journal.join("config/journal.json")).unwrap();
            if absent_parent {
                fs::remove_dir(journal.join("config")).unwrap();
            }
            let result = save_key(
                &journal,
                "OPENAI_API_KEY",
                "openai",
                "alpha-key",
                Some(json!({"valid":false,"reason_code":"provider_key_invalid"})),
                "",
                None,
            )
            .unwrap();
            assert_eq!(result["success"], true);
            assert_eq!(result["validation"]["valid"], false);
            let config = read_config(&journal).unwrap();
            assert_eq!(super::stored_env(&config, "OPENAI_API_KEY"), "alpha-key");
            assert_eq!(
                config["providers"]["key_validation"]["openai"]["valid"],
                false
            );
            assert!(
                config["providers"]["key_validation"]["openai"]["credential_hmac"]
                    .as_str()
                    .is_some_and(|hmac| hmac.len() == 64)
            );
            assert_eq!(
                config["providers"]["key_validation"]["openai"]["reason_code"],
                "provider_key_invalid"
            );
            fs::remove_dir_all(journal).unwrap();
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn newly_entered_rejected_key_saves_its_negative_without_losing_the_prior_target() {
        let journal = binding_journal(
            "new-negative-key",
            json!({"env":{"OPENAI_API_KEY":"alpha-key"}}),
        );
        plant_ready_key(&journal);
        let root =
            std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
        let alpha = solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "alpha-key",
        };
        let beta = solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "beta-key",
        };
        let alpha_session =
            solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), alpha).unwrap();
        let beta_session =
            solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), beta).unwrap();
        let negative = json!({"valid":false,"reason_code":"provider_key_invalid"});
        save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(negative.clone()),
            "alpha-key",
            None,
        )
        .unwrap();
        let prior = read_config(&journal).unwrap();
        let prior_entry = super::stored_validation(&prior, "openai");
        let old_positive =
            solstone_core_brain::capture_credential_validation_bound(root.clone(), &journal, beta)
                .unwrap();
        let result = save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(negative),
            "alpha-key",
            prior_entry,
        )
        .unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["validation"]["valid"], false);
        let config = read_config(&journal).unwrap();
        assert_eq!(super::stored_env(&config, "OPENAI_API_KEY"), "beta-key");
        let key = solstone_core_brain::load_existing_fingerprint_key(&journal).unwrap();
        let entry = super::stored_validation(&config, "openai").unwrap();
        assert_eq!(entry["valid"], false);
        assert_eq!(entry["reason_code"], "provider_key_invalid");
        assert_eq!(
            entry["credential_hmac"],
            solstone_core_brain::managed_credential_binding_hmac(&key, "openai", "beta-key")
        );
        for target in [alpha, beta] {
            assert_eq!(
                solstone_core_brain::classify_credential_admission_bound(
                    root.clone(),
                    &journal,
                    target,
                    None
                )
                .unwrap(),
                solstone_core_brain::StoredCredentialBinding::Matches
            );
        }
        let before = fs::read(journal.join("config/journal.json")).unwrap();
        let stale = super::save_key(
            &journal,
            &root,
            &super::Cancellation::never(),
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "alpha-key",
            prior_entry,
            Some(old_positive),
        );
        assert!(matches!(stale, Err(crate::MutationError::StaleProbe)));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).unwrap(),
            before
        );
        let actions = fs::read_dir(journal.join("config/actions"))
            .unwrap()
            .map(|file| fs::read_to_string(file.unwrap().path()).unwrap())
            .collect::<String>();
        assert!(actions.contains("env_update"));
        assert!(!actions.contains("alpha-key") && !actions.contains("beta-key"));
        save_key(
            &journal,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "beta-key",
            Some(entry),
        )
        .unwrap();
        assert!(alpha_session.is_held().unwrap());
        assert!(!beta_session.is_held().unwrap());
        fs::remove_dir_all(journal).unwrap();
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn negative_validations_keep_the_process_fact_when_post_probe_storage_is_unreadable() {
        struct RejectingWithUnreadableStorage {
            path: std::path::PathBuf,
            retained: std::path::PathBuf,
        }
        impl super::ManagedKeyValidator for RejectingWithUnreadableStorage {
            fn validate(&self, _: &str, _: &str) -> Result<Value, String> {
                fs::rename(&self.path, &self.retained).unwrap();
                fs::create_dir(&self.path).unwrap();
                Ok(json!({"valid":false,"reason_code":"provider_key_invalid"}))
            }
        }
        for bulk in [false, true] {
            for (unavailable, credential) in [
                ("config/journal.json", "alpha-key"),
                ("health/brain-fingerprint.key", "alpha-key"),
                ("config/journal.json", "beta-key"),
                ("health/brain-fingerprint.key", "beta-key"),
            ] {
                if bulk && credential != "alpha-key" {
                    continue;
                }
                let journal = binding_journal(
                    "negative-unreadable-storage",
                    json!({
                        "env":{"OPENAI_API_KEY":"alpha-key"},
                        "providers":{"active":{"provider":"openai","model":"gpt-binding"}}
                    }),
                );
                plant_ready_key(&journal);
                let root = std::sync::Arc::new(
                    solstone_core_journal_io::JournalRoot::open(&journal).unwrap(),
                );
                let target = solstone_core_brain::CredentialHoldTarget::Managed {
                    provider: "openai",
                    credential,
                };
                let session =
                    solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), target)
                        .unwrap();
                let config_before = fs::read(journal.join("config/journal.json")).unwrap();
                let key_before = fs::read(journal.join("health/brain-fingerprint.key")).unwrap();
                let brain_before = fs::read(journal.join("health/brain.json")).ok();
                let validator = RejectingWithUnreadableStorage {
                    path: journal.join(unavailable),
                    retained: journal.join(unavailable).with_extension("held-original"),
                };
                let result = if bulk {
                    super::persist_key_validations(&journal, &validator)
                } else {
                    let witness = solstone_core_brain::capture_credential_validation_bound(
                        root.clone(),
                        &journal,
                        target,
                    )
                    .unwrap();
                    let validation = validator.validate("openai", credential).unwrap();
                    super::save_key(
                        &journal,
                        &root,
                        &super::Cancellation::never(),
                        "OPENAI_API_KEY",
                        "openai",
                        credential,
                        Some(validation),
                        "alpha-key",
                        None,
                        Some(witness),
                    )
                };
                let held_while_unreadable = session.is_held().unwrap();
                fs::remove_dir(&validator.path).unwrap();
                fs::rename(&validator.retained, &validator.path).unwrap();
                assert!(result.is_err(), "bulk={bulk}, unavailable={unavailable}");
                assert_eq!(
                    fs::read(journal.join("config/journal.json")).unwrap(),
                    config_before
                );
                assert_eq!(
                    fs::read(journal.join("health/brain-fingerprint.key")).unwrap(),
                    key_before
                );
                assert_eq!(
                    fs::read(journal.join("health/brain.json")).ok(),
                    brain_before
                );
                assert_eq!(
                    (
                        held_while_unreadable,
                        session.is_held().unwrap(),
                        solstone_core_brain::classify_credential_admission_bound(
                            root.clone(),
                            &journal,
                            target,
                            None,
                        )
                        .unwrap()
                    ),
                    (
                        true,
                        true,
                        solstone_core_brain::StoredCredentialBinding::Matches
                    ),
                    "bulk={bulk}, unavailable={unavailable}: restoring exact old bytes is not a correction",
                );
                let fresh = solstone_core_brain::capture_credential_validation_bound(
                    root.clone(),
                    &journal,
                    target,
                )
                .unwrap();
                super::save_key(
                    &journal,
                    &root,
                    &super::Cancellation::never(),
                    "OPENAI_API_KEY",
                    "openai",
                    credential,
                    Some(json!({"valid":true})),
                    "alpha-key",
                    None,
                    Some(fresh),
                )
                .unwrap();
                assert!(!session.is_held().unwrap());
                fs::remove_dir_all(journal).unwrap();
            }
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn mutating_negative_validations_hold_work_when_config_or_all_publications_fail() {
        use std::os::unix::fs::PermissionsExt;
        struct Rejecting;
        impl super::ManagedKeyValidator for Rejecting {
            fn validate(&self, _: &str, _: &str) -> Result<Value, String> {
                Ok(json!({"valid":false,"reason_code":"provider_key_invalid"}))
            }
        }
        for bulk in [false, true] {
            for all_writes_fail in [false, true] {
                let journal = binding_journal(
                    "negative-write-fault",
                    json!({
                        "env":{"OPENAI_API_KEY":"alpha-key"},
                        "providers":{"active":{"provider":"openai","model":"gpt-binding"}}
                    }),
                );
                plant_ready_key(&journal);
                let root = std::sync::Arc::new(
                    solstone_core_journal_io::JournalRoot::open(&journal).unwrap(),
                );
                let target = solstone_core_brain::CredentialHoldTarget::Managed {
                    provider: "openai",
                    credential: "alpha-key",
                };
                let witness = solstone_core_brain::capture_credential_validation_bound(
                    root.clone(),
                    &journal,
                    target,
                )
                .unwrap();
                let session =
                    solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), target)
                        .unwrap();
                let before = fs::read(journal.join("config/journal.json")).unwrap();
                let key_before = fs::read(journal.join("health/brain-fingerprint.key")).unwrap();
                let config_mode = fs::metadata(journal.join("config")).unwrap().permissions();
                let health_mode = fs::metadata(journal.join("health")).unwrap().permissions();
                fs::set_permissions(journal.join("config"), fs::Permissions::from_mode(0o555))
                    .unwrap();
                if all_writes_fail {
                    fs::set_permissions(journal.join("health"), fs::Permissions::from_mode(0o555))
                        .unwrap();
                }
                let result = if bulk {
                    super::persist_key_validations(&journal, &Rejecting)
                } else {
                    super::save_key(
                        &journal,
                        &root,
                        &super::Cancellation::never(),
                        "OPENAI_API_KEY",
                        "openai",
                        "alpha-key",
                        Some(json!({"valid":false,"reason_code":"provider_key_invalid"})),
                        "alpha-key",
                        None,
                        Some(witness),
                    )
                };
                fs::set_permissions(journal.join("config"), config_mode).unwrap();
                fs::set_permissions(journal.join("health"), health_mode).unwrap();
                assert!(
                    matches!(result, Err(crate::MutationError::ConfigWrite(_))),
                    "bulk={bulk}, all={all_writes_fail}: {result:?}"
                );
                assert!(session.is_held().unwrap());
                assert_eq!(
                    fs::read(journal.join("config/journal.json")).unwrap(),
                    before
                );
                assert_eq!(
                    fs::read(journal.join("health/brain-fingerprint.key")).unwrap(),
                    key_before
                );
                assert!(!journal.join("config/actions").exists());
                assert_eq!(
                    solstone_core_brain::classify_credential_admission_bound(
                        root.clone(),
                        &journal,
                        target,
                        None
                    )
                    .unwrap(),
                    solstone_core_brain::StoredCredentialBinding::Matches
                );
                assert_eq!(journal.join("health/brain.json").exists(), !all_writes_fail);
                let fresh = solstone_core_brain::capture_credential_validation_bound(
                    root.clone(),
                    &journal,
                    target,
                )
                .unwrap();
                super::save_key(
                    &journal,
                    &root,
                    &super::Cancellation::never(),
                    "OPENAI_API_KEY",
                    "openai",
                    "alpha-key",
                    Some(json!({"valid":true})),
                    "alpha-key",
                    None,
                    Some(fresh),
                )
                .unwrap();
                assert!(!session.is_held().unwrap());
                fs::remove_dir_all(journal).unwrap();
            }
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn bulk_retains_each_observed_rejection_despite_earlier_publication_failure() {
        use std::os::unix::fs::PermissionsExt;

        struct RejectingBatch {
            journal: std::path::PathBuf,
            root: std::sync::Arc<solstone_core_journal_io::JournalRoot>,
            observed: std::sync::Mutex<Vec<String>>,
        }
        impl super::ManagedKeyValidator for RejectingBatch {
            fn validate(&self, provider: &str, credential: &str) -> Result<Value, String> {
                let mut observed = self.observed.lock().unwrap();
                for previous in observed.iter() {
                    let target = solstone_core_brain::CredentialHoldTarget::Managed {
                        provider: previous,
                        credential: "alpha-key",
                    };
                    assert_eq!(
                        solstone_core_brain::classify_credential_admission_bound(
                            self.root.clone(),
                            &self.journal,
                            target,
                            None
                        )
                        .unwrap(),
                        solstone_core_brain::StoredCredentialBinding::Matches,
                        "previous rejection must be retained before the next probe"
                    );
                }
                assert_eq!(credential, "alpha-key");
                observed.push(provider.to_owned());
                Ok(json!({"valid":false,"reason_code":"provider_key_invalid"}))
            }
        }

        for all_writes_fail in [false, true] {
            let journal = binding_journal(
                "bulk-negative-write-fault",
                json!({
                    "env": {
                        "GOOGLE_API_KEY":"alpha-key", "OPENAI_API_KEY":"alpha-key",
                        "ANTHROPIC_API_KEY":"alpha-key"
                    }
                }),
            );
            plant_ready_key(&journal);
            let root =
                std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
            let validator = RejectingBatch {
                journal: journal.clone(),
                root: root.clone(),
                observed: std::sync::Mutex::new(Vec::new()),
            };
            let before = fs::read(journal.join("config/journal.json")).unwrap();
            let key_before = fs::read(journal.join("health/brain-fingerprint.key")).unwrap();
            let config_mode = fs::metadata(journal.join("config")).unwrap().permissions();
            let health_mode = fs::metadata(journal.join("health")).unwrap().permissions();
            fs::set_permissions(journal.join("config"), fs::Permissions::from_mode(0o555)).unwrap();
            if all_writes_fail {
                fs::set_permissions(journal.join("health"), fs::Permissions::from_mode(0o555))
                    .unwrap();
            }
            let result = super::persist_key_validations(&journal, &validator);
            fs::set_permissions(journal.join("config"), config_mode).unwrap();
            fs::set_permissions(journal.join("health"), health_mode).unwrap();
            assert!(
                matches!(result, Err(crate::MutationError::ConfigWrite(_))),
                "{result:?}"
            );
            assert_eq!(
                *validator.observed.lock().unwrap(),
                vec!["google", "openai", "anthropic"]
            );
            for (provider, _) in super::CLOUD {
                let target = solstone_core_brain::CredentialHoldTarget::Managed {
                    provider,
                    credential: "alpha-key",
                };
                assert_eq!(
                    solstone_core_brain::classify_credential_admission_bound(
                        root.clone(),
                        &journal,
                        target,
                        None
                    )
                    .unwrap(),
                    solstone_core_brain::StoredCredentialBinding::Matches
                );
            }
            assert_eq!(
                fs::read(journal.join("config/journal.json")).unwrap(),
                before
            );
            assert_eq!(
                fs::read(journal.join("health/brain-fingerprint.key")).unwrap(),
                key_before
            );
            fs::remove_dir_all(journal).unwrap();
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn key_save_audit_refuses_native_parent_leaf_and_root_substitution() {
        use std::os::unix::fs::symlink;
        for fault in ["parent", "leaf", "root"] {
            let journal = binding_journal("audit-bound-native", json!({}));
            let root =
                std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
            let outside = journal.with_extension("outside");
            let moved = journal.with_extension("moved");
            fs::create_dir(&outside).unwrap();
            let sentinel = outside.join("foreign.jsonl");
            fs::write(&sentinel, b"foreign unchanged\n").unwrap();
            let witness = solstone_core_brain::capture_credential_validation_bound(
                root.clone(),
                &journal,
                solstone_core_brain::CredentialHoldTarget::Managed {
                    provider: "openai",
                    credential: "alpha-key",
                },
            )
            .unwrap();
            let attack_root = journal.clone();
            let attack_outside = outside.clone();
            let attack_moved = moved.clone();
            let (result, fired) = solstone_core_journal_io::run_with_bound_append_open_barrier(
                move || {
                    let actions = attack_root.join("config/actions");
                    match fault {
                        "parent" => {
                            fs::rename(&actions, &attack_moved).unwrap();
                            symlink(&attack_outside, &actions).unwrap();
                        }
                        "leaf" => {
                            let leaf = fs::read_dir(&actions)
                                .unwrap()
                                .next()
                                .unwrap()
                                .unwrap()
                                .path();
                            fs::rename(&leaf, &attack_moved).unwrap();
                            symlink(attack_outside.join("foreign.jsonl"), &leaf).unwrap();
                        }
                        "root" => {
                            fs::rename(&attack_root, &attack_moved).unwrap();
                            fs::create_dir_all(attack_root.join("config")).unwrap();
                            fs::write(
                                attack_root.join("config/journal.json"),
                                b"{\"foreign\":true}",
                            )
                            .unwrap();
                        }
                        _ => unreachable!(),
                    }
                },
                || {
                    super::save_key(
                        &journal,
                        &root,
                        &super::Cancellation::never(),
                        "OPENAI_API_KEY",
                        "openai",
                        "alpha-key",
                        Some(json!({"valid":true})),
                        "",
                        None,
                        Some(witness),
                    )
                },
            );
            assert!(fired, "actual append open barrier");
            assert!(result.is_err(), "fault={fault}");
            assert_eq!(fs::read(&sentinel).unwrap(), b"foreign unchanged\n");
            match fault {
                "parent" => assert_eq!(
                    fs::read_dir(&moved)
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .metadata()
                        .unwrap()
                        .len(),
                    0
                ),
                "leaf" => assert_eq!(fs::metadata(&moved).unwrap().len(), 0),
                "root" => {
                    assert_eq!(
                        fs::read(journal.join("config/journal.json")).unwrap(),
                        b"{\"foreign\":true}"
                    );
                    assert!(!journal.join("config/actions").exists());
                    assert_eq!(
                        fs::read_dir(moved.join("config/actions"))
                            .unwrap()
                            .next()
                            .unwrap()
                            .unwrap()
                            .metadata()
                            .unwrap()
                            .len(),
                        0
                    );
                }
                _ => unreachable!(),
            }
            fs::remove_dir_all(&journal).unwrap();
            if moved.is_dir() {
                fs::remove_dir_all(&moved).unwrap();
            } else {
                fs::remove_file(&moved).unwrap();
            }
            fs::remove_dir_all(&outside).unwrap();
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn older_validation_cannot_clear_a_new_memory_only_rejection_but_a_fresh_one_can() {
        let journal = binding_journal(
            "memory-only-retirement",
            json!({"env":{"OPENAI_API_KEY":"alpha-key"}}),
        );
        plant_ready_key(&journal);
        let root =
            std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
        let target = solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "alpha-key",
        };
        let older = solstone_core_brain::capture_credential_validation_bound(
            root.clone(),
            &journal,
            target,
        )
        .unwrap();
        let session =
            solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), target).unwrap();
        session.record_rejection().unwrap();
        let before = fs::read(journal.join("config/journal.json")).unwrap();
        let result = super::save_key(
            &journal,
            &root,
            &super::Cancellation::never(),
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":true})),
            "alpha-key",
            None,
            Some(older),
        );
        assert!(matches!(result, Err(crate::MutationError::StaleProbe)));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).unwrap(),
            before
        );
        assert!(session.is_held().unwrap());
        let fresh = solstone_core_brain::capture_credential_validation_bound(
            root.clone(),
            &journal,
            target,
        )
        .unwrap();
        super::save_key(
            &journal,
            &root,
            &super::Cancellation::never(),
            "OPENAI_API_KEY",
            "openai",
            "alpha-key",
            Some(json!({"valid":true})),
            "alpha-key",
            None,
            Some(fresh),
        )
        .unwrap();
        assert!(!session.is_held().unwrap());
        fs::remove_dir_all(journal).unwrap();
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn unacknowledged_config_publication_never_retires_a_surviving_hold() {
        let journal = binding_journal(
            "uncertain-retirement",
            json!({"env":{"OPENAI_API_KEY":"alpha-key"}}),
        );
        plant_ready_key(&journal);
        let root =
            std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
        let target = solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "alpha-key",
        };
        let session =
            solstone_core_brain::CredentialHoldSession::open_bound(root.clone(), target).unwrap();
        session.record_rejection().unwrap();
        let witness = solstone_core_brain::capture_credential_validation_bound(
            root.clone(),
            &journal,
            target,
        )
        .unwrap();
        let (result, consumed) = solstone_core_journal_io::run_with_bound_publication_fault(
            solstone_core_journal_io::BoundPublicationPrimitive::ParentSync,
            1,
            5,
            || {
                super::save_key(
                    &journal,
                    &root,
                    &super::Cancellation::never(),
                    "OPENAI_API_KEY",
                    "openai",
                    "alpha-key",
                    Some(json!({"valid":true})),
                    "alpha-key",
                    None,
                    Some(witness),
                )
            },
        );
        assert!(consumed);
        assert!(matches!(result, Err(crate::MutationError::ConfigWrite(_))));
        assert!(session.is_held().unwrap());
        assert_eq!(
            read_config(&journal).unwrap()["providers"]["key_validation"]["openai"]["valid"],
            true
        );
        assert!(!journal.join("config/actions").exists());
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn save_key_cancellation_before_publication_refuses_the_write() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let journal = binding_journal(
            "cancel-publish",
            json!({"env":{"OPENAI_API_KEY":"alpha-key"}}),
        );
        plant_ready_key(&journal);
        let before_config = fs::read(journal.join("config/journal.json")).expect("config");
        let before_key =
            fs::read(solstone_core_brain::brain_fingerprint_key_path(&journal)).expect("key");
        let calls = AtomicUsize::new(0);
        let check = || calls.fetch_add(1, Ordering::SeqCst) >= 2;
        let cancellation = solstone_core_generate::Cancellation::from_fn(&check);
        let admitted = solstone_core_journal_io::JournalRoot::open(&journal).expect("root");
        let validation_witness =
            save_witness(&journal, "openai", "beta-key").expect("pre-probe witness");
        let error = super::save_key(
            &journal,
            &admitted,
            &cancellation,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "alpha-key",
            None,
            validation_witness,
        )
        .expect_err("cancelled before publication");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config"),
            before_config
        );
        assert_eq!(
            fs::read(solstone_core_brain::brain_fingerprint_key_path(&journal)).expect("key"),
            before_key
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_key_cancellation_at_post_lock_refuses_before_publication() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let journal = binding_journal("cancel-lock", json!({"env":{"OPENAI_API_KEY":"alpha-key"}}));
        plant_ready_key(&journal);
        let before_config = fs::read(journal.join("config/journal.json")).expect("config");
        let calls = AtomicUsize::new(0);
        let check = || calls.fetch_add(1, Ordering::SeqCst) >= 1;
        let cancellation = solstone_core_generate::Cancellation::from_fn(&check);
        let admitted = solstone_core_journal_io::JournalRoot::open(&journal).expect("root");
        let validation_witness =
            save_witness(&journal, "openai", "beta-key").expect("pre-probe witness");
        let error = super::save_key(
            &journal,
            &admitted,
            &cancellation,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "alpha-key",
            None,
            validation_witness,
        )
        .expect_err("cancelled after the lock");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config"),
            before_config
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_after_admitted_root_rename_does_not_recreate_the_old_path() {
        let journal = binding_journal("rename-save", json!({"env":{"OPENAI_API_KEY":"alpha-key"}}));
        let admitted = solstone_core_journal_io::JournalRoot::open(&journal).expect("root");
        let validation_witness =
            save_witness(&journal, "openai", "beta-key").expect("pre-probe witness");
        let before = fs::read(journal.join("config/journal.json")).expect("config");
        let moved = journal.with_file_name(format!(
            "{}-moved",
            journal.file_name().expect("name").to_string_lossy()
        ));
        fs::rename(&journal, &moved).expect("rename admitted root");
        let other = journal.with_file_name(format!(
            "{}-other",
            journal.file_name().expect("name").to_string_lossy()
        ));
        fs::create_dir_all(other.join("config")).expect("other config");
        let other_bytes = br#"{"env":{"OPENAI_API_KEY":"other-key"}}"#;
        fs::write(other.join("config/journal.json"), other_bytes).expect("other config");
        let cancellation = solstone_core_generate::Cancellation::never();
        let error = super::save_key(
            &journal,
            &admitted,
            &cancellation,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "alpha-key",
            None,
            validation_witness,
        )
        .expect_err("renamed root");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert!(!journal.exists());
        assert!(!journal.join("health/brain-fingerprint.key").exists());
        assert!(!journal.join("config/journal.json").exists());
        assert!(!journal.join("config/journal.json.lock").exists());
        assert!(!journal.join("config/actions").exists());
        assert_eq!(
            fs::read(moved.join("config/journal.json")).expect("moved config"),
            before
        );
        assert_eq!(
            fs::read(other.join("config/journal.json")).expect("other config"),
            other_bytes
        );
        let other_key = other.join("health/brain-fingerprint.key");
        fs::create_dir_all(other_key.parent().expect("health")).expect("other health");
        let planted = [5u8; 32];
        fs::write(&other_key, planted).expect("other key");
        let error = super::save_key(
            &journal,
            &admitted,
            &cancellation,
            "OPENAI_API_KEY",
            "openai",
            "",
            None,
            "other-key",
            None,
            None,
        )
        .expect_err("empty delete uses the same checkpoint");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert!(!journal.exists());
        assert_eq!(fs::read(&other_key).expect("other key bytes"), planted);
        assert_eq!(
            fs::read(other.join("config/journal.json")).expect("other config"),
            other_bytes
        );
        let _ = fs::remove_dir_all(&moved);
        let _ = fs::remove_dir_all(&other);
    }

    fn publication_candidate(dir: &std::path::Path) -> bool {
        fs::read_dir(dir).ok().is_some_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                solstone_core_journal_io::is_publication_candidate_name(&entry.file_name())
            })
        })
    }

    #[test]
    fn save_key_cancellation_at_pre_rename_leaves_the_destination() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let journal = binding_journal(
            "cancel-rename",
            json!({"env":{"OPENAI_API_KEY":"alpha-key"}}),
        );
        plant_ready_key(&journal);
        let before_config = fs::read(journal.join("config/journal.json")).expect("config");
        let before_key =
            fs::read(solstone_core_brain::brain_fingerprint_key_path(&journal)).expect("key");
        let config_dir = journal.join("config");
        let calls = AtomicUsize::new(0);
        let check = || {
            calls.fetch_add(1, Ordering::SeqCst);
            publication_candidate(&config_dir)
        };
        let cancellation = solstone_core_generate::Cancellation::from_fn(&check);
        let admitted = solstone_core_journal_io::JournalRoot::open(&journal).expect("root");
        let validation_witness =
            save_witness(&journal, "openai", "beta-key").expect("pre-probe witness");
        let error = super::save_key(
            &journal,
            &admitted,
            &cancellation,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "alpha-key",
            None,
            validation_witness,
        )
        .expect_err("cancelled at pre-rename");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert!(calls.load(Ordering::SeqCst) > 3);
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("config"),
            before_config
        );
        assert_eq!(
            fs::read(solstone_core_brain::brain_fingerprint_key_path(&journal)).expect("key"),
            before_key
        );
        assert!(!publication_candidate(&config_dir));
        assert!(!journal.join("config/actions").exists());
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn save_after_replacement_at_the_admitted_path_leaves_both_journals() {
        let journal = binding_journal(
            "replace-save",
            json!({"env":{"OPENAI_API_KEY":"alpha-key"}}),
        );
        plant_ready_key(&journal);
        let admitted = solstone_core_journal_io::JournalRoot::open(&journal).expect("root");
        let validation_witness =
            save_witness(&journal, "openai", "beta-key").expect("pre-probe witness");
        let moved_config = fs::read(journal.join("config/journal.json")).expect("config");
        let moved_key =
            fs::read(solstone_core_brain::brain_fingerprint_key_path(&journal)).expect("key");
        let moved = journal.with_file_name(format!(
            "{}-moved",
            journal.file_name().expect("name").to_string_lossy()
        ));
        let cancellation = solstone_core_generate::Cancellation::never();
        match fs::rename(&journal, &moved) {
            Ok(()) => {}
            // Windows refuses to rename a root while the validation witness holds
            // a handle inside it, so the replacement cannot begin and the save
            // publishes into the only root.
            Err(error) if cfg!(windows) && error.kind() == std::io::ErrorKind::PermissionDenied => {
                super::save_key(
                    &journal,
                    &admitted,
                    &cancellation,
                    "OPENAI_API_KEY",
                    "openai",
                    "beta-key",
                    Some(json!({"valid":true})),
                    "alpha-key",
                    None,
                    validation_witness,
                )
                .expect("the save still names the only root");
                assert!(!moved.exists());
                let config = read_config(&journal).expect("config reads");
                assert_eq!(config["env"]["OPENAI_API_KEY"], "beta-key");
                let _ = fs::remove_dir_all(&journal);
                return;
            }
            Err(error) => panic!("rename admitted root: {error}"),
        }
        fs::create_dir_all(journal.join("config")).expect("replacement config");
        fs::create_dir_all(journal.join("health")).expect("replacement health");
        let replacement_config = br#"{"env":{"OPENAI_API_KEY":"replacement-key"}}"#;
        let replacement_key = [4u8; 32];
        fs::write(journal.join("config/journal.json"), replacement_config).expect("replacement");
        fs::write(
            journal.join("health/brain-fingerprint.key"),
            replacement_key,
        )
        .expect("replacement key");
        let error = super::save_key(
            &journal,
            &admitted,
            &cancellation,
            "OPENAI_API_KEY",
            "openai",
            "beta-key",
            Some(json!({"valid":true})),
            "alpha-key",
            None,
            validation_witness,
        )
        .expect_err("replaced root");
        assert!(matches!(error, crate::MutationError::StaleProbe));
        assert_eq!(
            fs::read(journal.join("config/journal.json")).expect("replacement config"),
            replacement_config
        );
        assert_eq!(
            fs::read(journal.join("health/brain-fingerprint.key")).expect("replacement key"),
            replacement_key
        );
        assert!(!journal.join("config/actions").exists());
        assert_eq!(
            fs::read(moved.join("config/journal.json")).expect("moved config"),
            moved_config
        );
        assert_eq!(
            fs::read(solstone_core_brain::brain_fingerprint_key_path(&moved)).expect("moved key"),
            moved_key
        );
        let _ = fs::remove_dir_all(&journal);
        let _ = fs::remove_dir_all(&moved);
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

    #[test]
    fn provider_switch_without_a_model_uses_the_owners_remembered_model() {
        let journal = temporary_journal(
            "switch-anthropic-remembered",
            json!({"providers":{"active":{"provider":"google","model":"gemini-3.5-flash"},"byo_models":{"anthropic":"owner-claude-model"}}}),
        );
        let update = resolve_provider_update(
            &journal,
            "byo",
            &request_map(&[("provider", json!("anthropic"))]),
        )
        .expect("a remembered model resolves");
        assert_eq!(update.model.as_deref(), Some("owner-claude-model"));
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn provider_switch_without_any_owner_model_is_refused_not_defaulted() {
        let journal = temporary_journal(
            "switch-anthropic-no-model",
            json!({"providers":{"active":{"provider":"google","model":"gemini-3.5-flash"}}}),
        );
        match resolve_provider_update(
            &journal,
            "byo",
            &request_map(&[("provider", json!("anthropic"))]),
        ) {
            Err(ProviderRequestError::ModelMissing(detail)) => {
                assert!(
                    detail.contains("--model"),
                    "the CLI guidance names the flag"
                );
            }
            other => panic!("expected ModelMissing, got {other:?}"),
        }
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn payload_carries_no_built_in_cloud_model_catalog() {
        let journal = temporary_journal("no-model-catalog", json!({}));
        let payload = super::payload(&journal, &Map::new(), "", Value::Null);
        assert!(payload.get("model_tiers").is_none());
        let _ = fs::remove_dir_all(journal);
    }

    fn request_map(pairs: &[(&str, Value)]) -> Map<String, Value> {
        Map::from_iter(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone())),
        )
    }

    fn assert_invalid_input(result: Result<ProviderUpdate, ProviderRequestError>, expected: &str) {
        match result {
            Err(ProviderRequestError::InvalidInput(detail)) => assert_eq!(detail, expected),
            other => panic!("expected InvalidInput({expected:?}), got {other:?}"),
        }
    }

    fn assert_invalid_state(result: Result<ProviderUpdate, ProviderRequestError>, expected: &str) {
        match result {
            Err(ProviderRequestError::InvalidState(detail)) => assert_eq!(detail, expected),
            other => panic!("expected InvalidState({expected:?}), got {other:?}"),
        }
    }

    fn spp_active_config() -> Value {
        json!({
            "providers": {
                "active": {"provider": "local", "model": "private"},
                "local": {
                    "endpoint_url": "https://private.example/v1",
                    "served_model_id": "private",
                    "credential": "secret"
                }
            },
            "services": {
                "confidential": {
                    "endpoint_url": "https://private.example",
                    "served_model_id": "private",
                    "credential_fingerprint_sha256": "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
                }
            }
        })
    }

    #[test]
    fn resolve_rejects_unknown_lane_with_the_closed_set_message() {
        let journal = temporary_journal("resolve-unknown-lane", json!({}));
        assert_invalid_input(
            resolve_provider_update(&journal, "nope", &Map::new()),
            "Invalid lane: nope. Must be one of: byo, confidential, local",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_byo_without_a_provider() {
        let journal = temporary_journal("resolve-byo-missing", json!({}));
        assert_invalid_input(
            resolve_provider_update(&journal, "byo", &Map::new()),
            "No BYO provider selected. Must be one of: anthropic, chatgpt, google, local, or openai",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_unknown_byo_provider() {
        let journal = temporary_journal("resolve-byo-unknown", json!({}));
        assert_invalid_input(
            resolve_provider_update(
                &journal,
                "byo",
                &request_map(&[("provider", json!("nope"))]),
            ),
            "Invalid provider for BYO lane. Must be one of: anthropic, chatgpt, google, local, or openai",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_confidential_when_not_provisioned() {
        let journal = temporary_journal("resolve-confidential-enable", json!({}));
        assert_invalid_state(
            resolve_provider_update(&journal, "confidential", &Map::new()),
            "confidential processing isn't on yet. turn it on first.",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_local_when_confidential_is_provisioned() {
        let journal = temporary_journal(
            "resolve-local-provisioned",
            json!({"services": {"confidential": {"endpoint_url": "https://private.example"}}}),
        );
        assert_invalid_state(
            resolve_provider_update(&journal, "local", &Map::new()),
            "turn off confidential processing first, then switch to the bundled local model.",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_local_when_an_endpoint_is_configured() {
        let journal = temporary_journal(
            "resolve-local-endpoint",
            json!({
                "providers": {
                    "local": {
                        "endpoint_url": "http://127.0.0.1:1/v1",
                        "served_model_id": "served-model"
                    }
                }
            }),
        );
        assert_invalid_state(
            resolve_provider_update(&journal, "local", &Map::new()),
            "clear your own endpoint first to run the bundled local model.",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_byo_local_without_an_endpoint() {
        let journal = temporary_journal("resolve-byo-local", json!({}));
        assert_invalid_state(
            resolve_provider_update(
                &journal,
                "byo",
                &request_map(&[("provider", json!("local"))]),
            ),
            "save your own endpoint first.",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_model_outside_cloud_byo() {
        let journal = temporary_journal("resolve-model-local", json!({}));
        assert_invalid_input(
            resolve_provider_update(&journal, "local", &request_map(&[("model", json!("m"))])),
            "model is only valid with cloud BYO providers: anthropic, chatgpt, google, openai.",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_blank_cloud_model() {
        let journal = temporary_journal("resolve-blank-model", json!({}));
        assert_invalid_input(
            resolve_provider_update(
                &journal,
                "byo",
                &request_map(&[("provider", json!("google")), ("model", json!(""))]),
            ),
            "model must be a non-empty string.",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_unknown_google_alias_targets() {
        let journal = temporary_journal("resolve-alias-unknown", json!({}));
        assert_invalid_input(
            resolve_provider_update(
                &journal,
                "byo",
                &request_map(&[
                    ("provider", json!("google")),
                    ("model", json!("m")),
                    ("google_model_resolution_targets", json!(["nope"])),
                ]),
            ),
            "Invalid Google model resolution targets: nope. Must be one of: active, confidential_prior, remembered",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_google_alias_targets_on_non_google_saves() {
        let journal = temporary_journal("resolve-alias-openai", json!({}));
        assert_invalid_input(
            resolve_provider_update(
                &journal,
                "byo",
                &request_map(&[
                    ("provider", json!("openai")),
                    ("model", json!("m")),
                    (
                        "google_model_resolution_targets",
                        json!(["confidential_prior"]),
                    ),
                ]),
            ),
            "google_model_resolution_targets is only valid with Google BYO model saves.",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_rejects_non_list_google_alias_targets() {
        let journal = temporary_journal("resolve-alias-shape", json!({}));
        assert_invalid_input(
            resolve_provider_update(
                &journal,
                "byo",
                &request_map(&[
                    ("provider", json!("google")),
                    ("model", json!("m")),
                    ("google_model_resolution_targets", json!("active")),
                ]),
            ),
            "google_model_resolution_targets must be a list of: active, confidential_prior, remembered",
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_accepts_bundled_local() {
        let journal = temporary_journal("resolve-local-ok", json!({}));
        let update =
            resolve_provider_update(&journal, "local", &Map::new()).expect("local resolves");
        assert_eq!(update.lane, "local");
        assert_eq!(update.provider, "local");
        assert_eq!(update.model, None);
        assert!(update.resolution_targets.is_empty());
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_accepts_byo_openai_with_a_model() {
        let journal = temporary_journal("resolve-byo-ok", json!({}));
        let update = resolve_provider_update(
            &journal,
            "byo",
            &request_map(&[("provider", json!("openai")), ("model", json!(" gpt-5 "))]),
        )
        .expect("byo openai resolves");
        assert_eq!(update.lane, "byo");
        assert_eq!(update.provider, "openai");
        assert_eq!(update.model.as_deref(), Some("gpt-5"));
        assert!(update.resolution_targets.is_empty());
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_reports_unreadable_config() {
        let journal = temporary_journal("resolve-corrupt", json!({}));
        fs::write(
            journal.join("config/journal.json"),
            br#"{"setup": {"completed_at": 17672256"#,
        )
        .expect("corrupt config writes");
        match resolve_provider_update(&journal, "local", &Map::new()) {
            Err(ProviderRequestError::ConfigUnreadable(detail)) => {
                assert!(detail.contains("your settings file at "), "{detail}");
            }
            other => panic!("expected ConfigUnreadable, got {other:?}"),
        }
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn resolve_approves_byo_while_spp_is_active_and_update_providers_refuses() {
        let journal = temporary_journal("resolve-spp-byo", spp_active_config());
        let update = resolve_provider_update(
            &journal,
            "byo",
            &request_map(&[
                ("provider", json!("google")),
                ("model", json!("gemini-3.5-flash")),
            ]),
        )
        .expect("byo resolves while spp is active");
        assert_eq!(update.lane, "byo");
        assert_eq!(update.provider, "google");
        match update_providers(&journal, update, Value::Null) {
            Err(ProviderUpdateError::Confidential(detail)) => assert_eq!(
                detail,
                "turn off confidential processing first, then switch your thinking provider."
            ),
            other => panic!("expected Confidential, got {other:?}"),
        }
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
        let result = classify_key_probe(Err(ClientError::Protocol(Box::new(ProtocolFailure {
            error: ProtocolError {
                id: None,
                reason: "stub_failure".to_owned(),
                detail: "one-shot stub hard failure".to_owned(),
            },
            status: ChildStatus {
                exit_code: Some(70),
                signal: None,
            },
            stdout: CapturedStream::empty(),
            stderr: CapturedStream::empty(),
            stdin_closed_early: false,
        }))));
        assert_eq!(result["valid"], false);
        assert_eq!(result["reason_code"], "stub_failure");
        assert_eq!(
            result["error"],
            "protocol stub_failure (exit 70): one-shot stub hard failure; stdin_closed_early=false; stdout=; stderr="
        );
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
        let result = classify_model_probe(Err(ClientError::Protocol(Box::new(ProtocolFailure {
            error: ProtocolError {
                id: None,
                reason: "stub_failure".to_owned(),
                detail: "one-shot stub hard failure".to_owned(),
            },
            status: ChildStatus {
                exit_code: Some(70),
                signal: None,
            },
            stdout: CapturedStream::empty(),
            stderr: CapturedStream::empty(),
            stdin_closed_early: false,
        }))));
        assert_eq!(result["valid"], false);
        assert_eq!(result["reason_code"], "stub_failure");
        assert_eq!(
            result["error"],
            "protocol stub_failure (exit 70): one-shot stub hard failure; stdin_closed_early=false; stdout=; stderr="
        );
    }

    // The prompt, token budget, thinking budget, retry count, timeout, schema,
    // attempt index, and exclusive admission values retain the retired Python
    // validation request contract.
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
            timeout_s: Some(30.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: Some(0),
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    struct ValidationStub {
        _temporary: tempfile::TempDir,
        root: std::path::PathBuf,
        executable: std::path::PathBuf,
        raw_request: std::path::PathBuf,
        environment: std::path::PathBuf,
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    impl ValidationStub {
        fn new(name: &str, reason_code: &str) -> Self {
            let temporary = tempfile::Builder::new()
                .prefix(&format!("solstone-thinking-{name}-"))
                .tempdir()
                .expect("owned stub directory");
            let root = temporary.path().to_path_buf();
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
                _temporary: temporary,
                root,
                executable,
                raw_request,
                environment,
            }
        }

        fn new_generated(name: &str, text: &str) -> Self {
            let temporary = tempfile::Builder::new()
                .prefix(&format!("solstone-thinking-{name}-"))
                .tempdir()
                .expect("owned stub directory");
            let root = temporary.path().to_path_buf();
            let executable = root.join("stub.sh");
            let raw_request = root.join("request.json");
            let environment = root.join("environment.txt");
            let mut generated_response = generated();
            let GenerateResponse::Generated(ref mut inner) = generated_response else {
                unreachable!()
            };
            inner.text = text.to_owned();
            let response =
                encode_one_shot_response(&generated_response).expect("stub response encodes");
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
                _temporary: temporary,
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

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn endpoint_image_check_spawned_child_contract() {
        use crate::endpoint_image::{Hue, ImageCheck, check_with_hue};
        use solstone_core_generate::{ContentPart, OneShotClient};
        use solstone_core_generate_wire::parse_one_shot_request;

        let stub = ValidationStub::new_generated("endpoint-image-spawn", "orange");
        let parent_env_before = std::env::var("SOLSTONE_JOURNAL").ok();
        let journal_path = stub.root.join("test-journal");
        fs::create_dir_all(&journal_path).expect("journal dir creates");

        let mut client = OneShotClient::at_path(&stub.executable);
        let check = check_with_hue(&mut client, &journal_path, Hue::Orange);
        assert_eq!(check, ImageCheck::Read);

        let parent_env_after = std::env::var("SOLSTONE_JOURNAL").ok();
        assert_eq!(
            parent_env_before, parent_env_after,
            "parent SOLSTONE_JOURNAL must remain unchanged"
        );

        let (raw_req, env_text) = stub.records();
        let env_lines: Vec<&str> = env_text.lines().collect();
        assert!(
            env_lines.contains(&"SOLSTONE_GENERATE_PROVIDER_OVERRIDE=local"),
            "child must receive SOLSTONE_GENERATE_PROVIDER_OVERRIDE=local, got env:\n{env_text}"
        );
        let expected_journal_line = format!("SOLSTONE_JOURNAL={}", journal_path.display());
        assert!(
            env_lines.iter().any(|line| *line == expected_journal_line),
            "child must receive exact SOLSTONE_JOURNAL line {expected_journal_line}, got env:\n{env_text}"
        );

        let request =
            parse_one_shot_request(std::str::from_utf8(&raw_req).expect("valid utf-8 request"))
                .expect("request parses");
        assert_eq!(request.context, "settings.local.endpoint_image");
        assert_eq!(request.temperature, 0.0);
        assert_eq!(request.timeout_s, Some(20.0));
        assert_eq!(request.max_output_tokens, 512);
        assert!(!request.json_output);
        assert!(request.system_instruction.is_none());
        assert_eq!(request.contents.len(), 2);
        assert!(matches!(request.contents[0], ContentPart::Image { .. }));
        assert_eq!(
            request.contents[1],
            ContentPart::Text {
                text: "What color is this image? Answer with one word.".to_owned(),
            }
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn shell_quote(path: &std::path::Path) -> String {
        format!(
            "'{}'",
            path.display().to_string().replace('\'', "'\\\"'\\\"'")
        )
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn assert_environment_line(environment: &str, name: &str, value: &str) {
        assert!(
            environment
                .lines()
                .any(|line| line == format!("{name}={value}")),
            "environment includes {name}"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn assert_split_probe(reason_code: &str) -> (Value, Value) {
        const KEY: &str = "sk-candidate-not-in-request";
        let stub = ValidationStub::new(reason_code, reason_code);
        let validator = OneShotKeyValidator::at_path(&stub.executable);
        let key_result = validator.validate("openai", KEY).expect("key probe runs");
        assert_eq!(
            key_result["valid"], true,
            "actual key probe outcome: {key_result}"
        );
        assert_eq!(
            key_result["probe_reason_code"], reason_code,
            "actual key probe outcome: {key_result}"
        );
        let (key_request, key_environment) = stub.records();
        let model_result = validator
            .validate_model("openai", "candidate-model", KEY)
            .expect("model probe runs");
        assert_eq!(
            model_result["valid"], false,
            "actual model probe outcome: {model_result}"
        );
        assert_eq!(
            model_result["reason_code"], reason_code,
            "actual model probe outcome: {model_result}"
        );
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

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn key_probe_accepts_model_not_found_but_model_probe_rejects_it() {
        let (key_result, model_result) = assert_split_probe("model_not_found");
        assert_eq!(key_result["valid"], true);
        assert_eq!(key_result["probe_reason_code"], "model_not_found");
        assert_eq!(model_result["valid"], false);
        assert_eq!(model_result["reason_code"], "model_not_found");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn key_probe_accepts_quota_but_model_probe_rejects_it() {
        let (key_result, model_result) = assert_split_probe("provider_quota_exceeded");
        assert_eq!(key_result["valid"], true);
        assert_eq!(key_result["probe_reason_code"], "provider_quota_exceeded");
        assert_eq!(model_result["valid"], false);
        assert_eq!(model_result["reason_code"], "provider_quota_exceeded");
    }

    fn spp_ready_evidence(now: chrono::DateTime<chrono::Utc>) -> Value {
        let observed = now.to_rfc3339();
        let expires = (now + chrono::Duration::hours(2)).to_rfc3339();
        json!({
            "configuration": {"status": "ok", "observed_at": observed, "expires_at": expires},
            "generate": {"status": "ok", "observed_at": observed, "expires_at": expires},
            "lane_prerequisites": {
                "status": "ok",
                "observed_at": observed,
                "expires_at": expires,
            }
        })
    }

    #[test]
    fn confidential_attestation_surfaces_in_payload_for_refusal_states() {
        let config_json = json!({
            "services": {
                "confidential": {
                    "device": "abc",
                    "endpoint_url": "https://attested.example",
                    "served_model_id": "served-model",
                    "credential_fingerprint_sha256": "cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers": {
                "active": {"provider": "local", "model": "served-model"},
                "local": {
                    "endpoint_url": "https://attested.example",
                    "served_model_id": "served-model",
                    "credential": "endpoint-credential"
                }
            }
        });
        let config_map = config_json.as_object().unwrap().clone();

        // 1. Ready journal: begin_refresh -> finish_refresh -> record gateway_unreachable -> unreachable
        let ready_temp = tempfile::tempdir().unwrap();
        let ready_path = ready_temp.path();
        std::fs::create_dir_all(ready_path.join("config")).unwrap();
        std::fs::write(
            ready_path.join("config/journal.json"),
            serde_json::to_vec(&config_map).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(ready_path).unwrap();

        let now = chrono::Utc::now();
        let ready_permit = solstone_core_brain::begin_refresh(
            ready_path,
            now,
            Some("ready-run".to_owned()),
            None,
            false,
            None,
        )
        .expect("begin_refresh succeeds")
        .expect("permit exists");

        let finish_res = solstone_core_brain::finish_refresh(
            ready_path,
            ready_permit,
            spp_ready_evidence(now),
            now,
            None,
        );
        assert!(finish_res.is_ok());

        solstone_core_brain::record_confidential_attestation_refusal(
            ready_path,
            &config_map,
            "gateway_unreachable",
        );
        let p1 = super::payload(ready_path, &config_map, "served-model", Value::Null);
        let state1 = &p1["active_lane"]["confidential_attestation"]["state"];
        assert_eq!(state1, "unreachable");
        assert_ne!(state1, "verified");
        assert_ne!(state1, "off");
        assert_ne!(state1, "inactive");

        // 2. Mid-check journal: begin_refresh only -> record certificate_invalid -> failed
        let mid_check_temp = tempfile::tempdir().unwrap();
        let mid_check_path = mid_check_temp.path();
        std::fs::create_dir_all(mid_check_path.join("config")).unwrap();
        std::fs::write(
            mid_check_path.join("config/journal.json"),
            serde_json::to_vec(&config_map).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(mid_check_path).unwrap();

        let _mid_check_permit = solstone_core_brain::begin_refresh(
            mid_check_path,
            now,
            Some("mid-check-run".to_owned()),
            None,
            false,
            None,
        )
        .expect("begin_refresh succeeds")
        .expect("permit exists");

        solstone_core_brain::record_confidential_attestation_refusal(
            mid_check_path,
            &config_map,
            "certificate_invalid",
        );

        let check_now = chrono::Utc::now() + chrono::Duration::seconds(1);
        let inspection =
            solstone_core_brain::inspect_brain_state(mid_check_path, &config_map, check_now);
        assert_eq!(inspection.projection.aggregate_state, "unhealthy");
        assert_ne!(inspection.projection.aggregate_state, "checking");

        let p2 = super::payload(mid_check_path, &config_map, "served-model", Value::Null);
        let state2 = &p2["active_lane"]["confidential_attestation"]["state"];
        assert_eq!(state2, "failed");
        assert_ne!(state2, "verified");
        assert_ne!(state2, "off");
        assert_ne!(state2, "inactive");

        // 3. Record attestation_not_verified -> unreachable
        solstone_core_brain::record_confidential_attestation_refusal(
            mid_check_path,
            &config_map,
            "attestation_not_verified",
        );
        let p3 = super::payload(mid_check_path, &config_map, "served-model", Value::Null);
        let state3 = &p3["active_lane"]["confidential_attestation"]["state"];
        assert_eq!(state3, "unreachable");
        assert_ne!(state3, "verified");
        assert_ne!(state3, "off");
        assert_ne!(state3, "inactive");
    }

    #[test]
    fn transcription_verification_surfaces_in_payload_when_not_spp_lane() {
        let config_json = json!({
            "services": {
                "confidential": {
                    "device": "dev-1",
                    "endpoint_url": "https://attested.example",
                    "served_model_id": "served",
                    "credential_fingerprint_sha256": "cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers": {
                "active": {"provider": "openai", "model": "gpt-4o"},
                "local": {"endpoint_url": "https://attested.example", "served_model_id": "served", "credential": "endpoint-credential"}
            }
        });
        let config_map = config_json.as_object().unwrap().clone();

        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            serde_json::to_vec(&config_json).unwrap(),
        )
        .unwrap();

        // 1. gateway_unreachable -> unreachable / attestation_not_verified
        solstone_core_brain::record_transcription_verification(
            journal,
            "gateway_unreachable",
            "https://attested.example",
        )
        .unwrap();
        let p1 = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att1 = &p1["active_lane"]["confidential_attestation"];
        assert_eq!(att1["state"], "unreachable");
        assert_eq!(att1["reason"], "attestation_not_verified");

        // 2. certificate_invalid -> failed / attestation_rejected
        solstone_core_brain::record_transcription_verification(
            journal,
            "certificate_invalid",
            "https://attested.example",
        )
        .unwrap();
        let p2 = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att2 = &p2["active_lane"]["confidential_attestation"];
        assert_eq!(att2["state"], "failed");
        assert_eq!(att2["reason"], "attestation_rejected");

        // 3. attestation_not_verified -> unreachable / attestation_not_verified
        solstone_core_brain::record_transcription_verification(
            journal,
            "attestation_not_verified",
            "https://attested.example",
        )
        .unwrap();
        let p3 = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att3 = &p3["active_lane"]["confidential_attestation"];
        assert_eq!(att3["state"], "unreachable");
        assert_eq!(att3["reason"], "attestation_not_verified");

        // 3b. Stored retired code -> stale
        let tv_path = solstone_core_brain::transcription_verification_path(journal);
        std::fs::write(
            &tv_path,
            serde_json::to_vec(&serde_json::json!({
                "reason": "nvattest_install_in_progress",
                "observed_at": "2026-01-01T00:00:00Z",
                "endpoint": "https://attested.example",
            }))
            .unwrap(),
        )
        .unwrap();
        let p3b = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att3b = &p3b["active_lane"]["confidential_attestation"];
        assert_eq!(att3b["state"], "stale");
        assert_eq!(att3b["reason"], "nvattest_install_in_progress");

        // 4. No status file -> inactive / confidential_not_active
        solstone_core_brain::clear_transcription_verification(journal).unwrap();
        let p4 = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att4 = &p4["active_lane"]["confidential_attestation"];
        assert_eq!(att4["state"], "inactive");
        assert_eq!(att4["reason"], "confidential_not_active");

        // 5. Status endpoint mismatch -> inactive / confidential_not_active
        solstone_core_brain::record_transcription_verification(
            journal,
            "gateway_unreachable",
            "https://other.example",
        )
        .unwrap();
        let p5 = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att5 = &p5["active_lane"]["confidential_attestation"];
        assert_eq!(att5["state"], "inactive");
        assert_eq!(att5["reason"], "confidential_not_active");

        // 6. transcribe.confidential_audio false -> inactive / confidential_not_active even with matching status
        let mut disabled_audio_map = config_map.clone();
        disabled_audio_map.insert(
            "transcribe".to_owned(),
            json!({"confidential_audio": false}),
        );
        solstone_core_brain::record_transcription_verification(
            journal,
            "gateway_unreachable",
            "https://attested.example",
        )
        .unwrap();
        let p6 = super::payload(journal, &disabled_audio_map, "gpt-4o", Value::Null);
        let att6 = &p6["active_lane"]["confidential_attestation"];
        assert_eq!(att6["state"], "inactive");
        assert_eq!(att6["reason"], "confidential_not_active");

        // 7. No confidential Byo -> off / confidential_not_configured even if status exists
        let no_byo_config = json!({
            "providers": {
                "active": {"provider": "openai", "model": "gpt-4o"}
            }
        });
        let no_byo_map = no_byo_config.as_object().unwrap().clone();
        let p7 = super::payload(journal, &no_byo_map, "gpt-4o", Value::Null);
        let att7 = &p7["active_lane"]["confidential_attestation"];
        assert_eq!(att7["state"], "off");
        assert_eq!(att7["reason"], "confidential_not_configured");

        // 8. Malformed file (not-json) -> inactive
        let status_path = solstone_core_brain::transcription_verification_path(journal);
        fs::write(&status_path, b"not-json").unwrap();
        let p8 = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att8 = &p8["active_lane"]["confidential_attestation"];
        assert_eq!(att8["state"], "inactive");

        // 9. Unreadable file (status path is directory) -> inactive
        let _ = fs::remove_file(&status_path);
        fs::create_dir_all(&status_path).unwrap();
        let p9 = super::payload(journal, &config_map, "gpt-4o", Value::Null);
        let att9 = &p9["active_lane"]["confidential_attestation"];
        assert_eq!(att9["state"], "inactive");
        let _ = fs::remove_dir_all(&status_path);

        // 10. Active confidential lane (spp): follows brain record, ignores transcription verification
        let spp_config_json = json!({
            "services": {
                "confidential": {
                    "device": "dev-1",
                    "endpoint_url": "https://attested.example",
                    "served_model_id": "served",
                    "credential_fingerprint_sha256": "cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers": {
                "active": {"provider": "local", "model": "served"},
                "local": {"endpoint_url": "https://attested.example", "served_model_id": "served", "credential": "endpoint-credential"}
            }
        });
        let spp_map = spp_config_json.as_object().unwrap().clone();
        fs::write(
            journal.join("config/journal.json"),
            serde_json::to_vec(&spp_config_json).unwrap(),
        )
        .unwrap();
        solstone_core_brain::generate_fingerprint_key(journal).unwrap();
        let permit = solstone_core_brain::begin_refresh(
            journal,
            chrono::Utc::now(),
            Some("spp-test".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .unwrap();
        let _ = solstone_core_brain::finish_refresh(
            journal,
            permit,
            json!({
                "configuration":{"status":"ok","observed_at":"2026-05-14T15:30:00Z","expires_at":"2026-05-15T15:30:00Z"},
                "lane_prerequisites":{"status":"ok","observed_at":"2026-05-14T15:30:00Z","expires_at":"2026-05-15T15:30:00Z"},
                "generate":{"status":"ok","observed_at":"2026-05-14T15:30:00Z","expires_at":"2026-05-15T15:30:00Z"}
            }),
            chrono::Utc::now(),
            None,
        );
        solstone_core_brain::record_confidential_attestation_refusal(
            journal,
            &spp_map,
            "gateway_unreachable",
        );
        solstone_core_brain::record_transcription_verification(
            journal,
            "certificate_invalid",
            "https://attested.example",
        )
        .unwrap();
        let p10 = super::payload(journal, &spp_map, "served", Value::Null);
        let att10 = &p10["active_lane"]["confidential_attestation"];
        assert_eq!(att10["state"], "unreachable");
        assert_eq!(att10["reason"], "attestation_not_verified");
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

    #[test]
    fn an_endpoint_url_names_the_host_and_port_it_connects_to() {
        use super::socket_target;
        assert_eq!(
            socket_target("https://bedrock-runtime.us-east-1.amazonaws.com/openai").as_deref(),
            Some("bedrock-runtime.us-east-1.amazonaws.com:443")
        );
        assert_eq!(
            socket_target("http://mybox.local:8000/v1").as_deref(),
            Some("mybox.local:8000")
        );
        assert_eq!(
            socket_target("http://127.0.0.1").as_deref(),
            Some("127.0.0.1:80")
        );
        assert_eq!(
            socket_target("http://[::1]:8080/x").as_deref(),
            Some("[::1]:8080")
        );
        assert_eq!(socket_target("https://[::1]").as_deref(), Some("[::1]:443"));
        assert_eq!(
            socket_target("https://user:pw@host.example/v1").as_deref(),
            Some("host.example:443")
        );
        assert_eq!(socket_target("ftp://host.example"), None);
        assert_eq!(socket_target("https:///v1"), None);
    }

    #[test]
    #[cfg(feature = "test-hooks")]
    fn acceptance_16_chatgpt_provider_status() {
        let journal = temporary_journal(
            "status-chatgpt",
            json!({"providers": {"active": {"provider": "chatgpt", "model": "gpt-test"}}}),
        );
        let config = json!({"providers": {"active": {"provider": "chatgpt", "model": "gpt-test"}}});
        let config_map = config.as_object().unwrap().clone();

        // 1. Corrupt credential file => configured: false
        solstone_core_chatgpt_auth::write_unreadable_test_credential(&journal).unwrap();
        let p_corrupt = super::payload(&journal, &config_map, "gpt-test", Value::Null);
        let chatgpt_st = &p_corrupt["provider_status"]["chatgpt"];
        assert_eq!(chatgpt_st["configured"], false);
        assert_eq!(chatgpt_st["generate_ready"], false);

        // 2. Valid signed in credential => configured: true, generate_ready: true
        solstone_core_chatgpt_auth::write_test_credential(&journal, true).unwrap();
        let p_valid = super::payload(&journal, &config_map, "gpt-test", Value::Null);
        let chatgpt_st2 = &p_valid["provider_status"]["chatgpt"];
        assert_eq!(chatgpt_st2["configured"], true);
        assert_eq!(chatgpt_st2["generate_ready"], true);

        // 3. After mark_token_rejected => generate_ready: false
        let auth =
            solstone_core_chatgpt_auth::ChatGptAuthManager::with_default_transport(journal.clone());
        use solstone_core_chatgpt_auth::ChatGptCredential;
        auth.mark_token_rejected("test-access-token").unwrap();
        let p_rejected = super::payload(&journal, &config_map, "gpt-test", Value::Null);
        let chatgpt_st3 = &p_rejected["provider_status"]["chatgpt"];
        assert_eq!(chatgpt_st3["generate_ready"], false);

        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn acceptance_17_resolve_provider_update_with_stub_models() {
        let journal = temporary_journal("resolve-chatgpt", json!({}));

        let valid_model = solstone_core_chatgpt_auth::ChatGptModel {
            slug: "gpt-test".to_string(),
            display_name: "GPT Test".to_string(),
            description: None,
            priority: None,
        };

        let stub_ok = StubModelSource {
            status: Ok(solstone_core_chatgpt_auth::ChatGptStatus {
                state: solstone_core_chatgpt_auth::SignInState::SignedIn,
                signed_in: true,
                email: None,
                expires_at: None,
                plan_usage_declined: false,
                client_refused: false,
            }),
            models: Ok(vec![valid_model]),
            called_list: std::sync::atomic::AtomicBool::new(false),
        };

        // 1. Listed slug accepted
        let res = super::resolve_provider_update_with(
            &journal,
            "byo",
            &request_map(&[("provider", json!("chatgpt")), ("model", json!("gpt-test"))]),
            &stub_ok,
        );
        assert!(res.is_ok());
        let update = res.unwrap();
        assert_eq!(update.provider, "chatgpt");
        assert_eq!(update.model.as_deref(), Some("gpt-test"));

        // 2. Unlisted slug -> model_not_found
        let res_unlisted = super::resolve_provider_update_with(
            &journal,
            "byo",
            &request_map(&[
                ("provider", json!("chatgpt")),
                ("model", json!("unknown-model")),
            ]),
            &stub_ok,
        );
        match res_unlisted {
            Err(super::ProviderRequestError::Reason { reason_code, .. }) => {
                assert_eq!(reason_code, "model_not_found");
            }
            other => panic!("expected model_not_found, got {other:?}"),
        }

        // 3. Missing slug and nothing remembered -> ModelMissing
        let res_missing = super::resolve_provider_update_with(
            &journal,
            "byo",
            &request_map(&[("provider", json!("chatgpt"))]),
            &stub_ok,
        );
        assert!(matches!(
            res_missing,
            Err(super::ProviderRequestError::ModelMissing(_))
        ));

        // 4. Signed out -> chatgpt_sign_in_required
        let stub_signed_out = StubModelSource {
            status: Ok(solstone_core_chatgpt_auth::ChatGptStatus {
                state: solstone_core_chatgpt_auth::SignInState::SignedOut,
                signed_in: false,
                email: None,
                expires_at: None,
                plan_usage_declined: false,
                client_refused: false,
            }),
            models: Ok(vec![]),
            called_list: std::sync::atomic::AtomicBool::new(false),
        };
        let res_signed_out = super::resolve_provider_update_with(
            &journal,
            "byo",
            &request_map(&[("provider", json!("chatgpt")), ("model", json!("gpt-test"))]),
            &stub_signed_out,
        );
        match res_signed_out {
            Err(super::ProviderRequestError::Reason {
                reason_code,
                detail: _,
            }) => {
                assert_eq!(reason_code, "chatgpt_sign_in_required");
            }
            other => panic!("expected chatgpt_sign_in_required, got {other:?}"),
        }

        // 5. Models unavailable -> provider_unavailable
        let stub_unavailable = StubModelSource {
            status: Ok(solstone_core_chatgpt_auth::ChatGptStatus {
                state: solstone_core_chatgpt_auth::SignInState::SignedIn,
                signed_in: true,
                email: None,
                expires_at: None,
                plan_usage_declined: false,
                client_refused: false,
            }),
            models: Err(solstone_core_chatgpt_auth::CredentialError::Unavailable),
            called_list: std::sync::atomic::AtomicBool::new(false),
        };
        let res_unavail = super::resolve_provider_update_with(
            &journal,
            "byo",
            &request_map(&[("provider", json!("chatgpt")), ("model", json!("gpt-test"))]),
            &stub_unavailable,
        );
        match res_unavail {
            Err(super::ProviderRequestError::Reason { reason_code, .. }) => {
                assert_eq!(reason_code, "provider_unavailable");
            }
            other => panic!("expected provider_unavailable, got {other:?}"),
        }

        // 6. Refused client not enabled -> chatgpt_not_eligible
        let stub_not_eligible = StubModelSource {
            status: Ok(solstone_core_chatgpt_auth::ChatGptStatus {
                state: solstone_core_chatgpt_auth::SignInState::SignedIn,
                signed_in: true,
                email: None,
                expires_at: None,
                plan_usage_declined: false,
                client_refused: false,
            }),
            models: Err(solstone_core_chatgpt_auth::CredentialError::Refused(Some(
                "subscription_sharing_v2_client_not_enabled".to_string(),
            ))),
            called_list: std::sync::atomic::AtomicBool::new(false),
        };
        let res_not_eligible = super::resolve_provider_update_with(
            &journal,
            "byo",
            &request_map(&[("provider", json!("chatgpt")), ("model", json!("gpt-test"))]),
            &stub_not_eligible,
        );
        match res_not_eligible {
            Err(super::ProviderRequestError::Reason { reason_code, .. }) => {
                assert_eq!(reason_code, "chatgpt_not_eligible");
            }
            other => panic!("expected chatgpt_not_eligible, got {other:?}"),
        }

        let _ = fs::remove_dir_all(journal);
    }

    struct StubModelSource {
        status: Result<
            solstone_core_chatgpt_auth::ChatGptStatus,
            solstone_core_chatgpt_auth::CredentialError,
        >,
        models: Result<
            Vec<solstone_core_chatgpt_auth::ChatGptModel>,
            solstone_core_chatgpt_auth::CredentialError,
        >,
        called_list: std::sync::atomic::AtomicBool,
    }

    impl crate::chatgpt::ChatGptModelSource for StubModelSource {
        fn status(
            &self,
            _journal: &std::path::Path,
        ) -> Result<
            solstone_core_chatgpt_auth::ChatGptStatus,
            solstone_core_chatgpt_auth::CredentialError,
        > {
            self.status.clone()
        }
        fn list(
            &self,
            _journal: &std::path::Path,
        ) -> Result<
            Vec<solstone_core_chatgpt_auth::ChatGptModel>,
            solstone_core_chatgpt_auth::CredentialError,
        > {
            self.called_list
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.models.clone()
        }
    }
}

/// Desktop key probe. `OneShotKeyValidator::at_path` is the process arm:
/// `OneShotClient::at_path`, the three override environment variables, and
/// `execute` of the canned validation request. This is not `respond`.
// The stub is a shebang script, which only a unix host executes. The unix
// gate is its own attribute so the CI topology check still sees the exact
// full-tests cfg.
#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod desktop_probe {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde_json::{Value, json};
    use solstone_core_generate::{Cancellation, ContentPart, GenerateRequest, GenerateResponse};
    use solstone_core_generate_wire::overrides::{
        API_KEY_OVERRIDE_ENV, BASE_URL_OVERRIDE_ENV, MODEL_OVERRIDE_ENV, PROVIDER_OVERRIDE_ENV,
    };

    use super::ManagedKeyValidator;

    const CASE: &str = "desktop_oneshot_at_path_posts_the_canned_probe_and_content_posts_nothing";
    const MARKER: &str = "SOLSTONE_DESKTOP_PROBE_CHILD";
    const JOURNAL_ENV: &str = "SOLSTONE_DESKTOP_PROBE_JOURNAL";
    const RECEIPT_LINE: &str = "SOLSTONE_DESKTOP_PROBE_RECEIPT ";
    const CANNED: &str = "Reply with the single word OK.";
    const CANDIDATE: &str = "openai-candidate-secret";
    const OTHER_KEY: &str = "anthropic-stored-key";
    const PROVIDER_BODY: &str = "DESKTOP-PROBE-BODY";
    const CONTENT_CANARY: &str = "desktop-content-canary";

    #[test]
    fn desktop_oneshot_at_path_posts_the_canned_probe_and_content_posts_nothing() {
        if std::env::var(MARKER).ok().as_deref() == Some(CASE) {
            let journal = PathBuf::from(std::env::var(JOURNAL_ENV).expect("probe journal"));
            let runtime = solstone_core_generate_wire::EndpointRuntime::default();
            let cancellation = Cancellation::never();
            let reason = reason_code(solstone_core_generate_wire::respond(
                &content_request(),
                &runtime,
                &journal,
                |_| None,
                &cancellation,
            ));
            println!("{RECEIPT_LINE}{reason}");
            return;
        }

        let parent_env = [
            API_KEY_OVERRIDE_ENV,
            PROVIDER_OVERRIDE_ENV,
            MODEL_OVERRIDE_ENV,
            BASE_URL_OVERRIDE_ENV,
        ]
        .map(|name| (name, std::env::var(name).ok()));

        let root = PathBuf::from("/var/tmp").join(format!(
            "solstone-desktop-probe-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let journal = root.join("journal");
        std::fs::create_dir_all(journal.join("config")).expect("journal config");
        let key = solstone_core_brain::generate_fingerprint_key(&journal).expect("binding key");
        let candidate_hmac =
            solstone_core_brain::managed_credential_binding_hmac(&key, "openai", CANDIDATE);
        let other_hmac =
            solstone_core_brain::managed_credential_binding_hmac(&key, "anthropic", OTHER_KEY);
        let mut openai_entry = serde_json::Map::new();
        openai_entry.insert("valid".to_owned(), Value::Bool(false));
        openai_entry.insert(
            "reason_code".to_owned(),
            Value::String("provider_key_invalid".to_owned()),
        );
        solstone_core_brain::attach_credential_binding(&mut openai_entry, &candidate_hmac);
        let mut anthropic_entry = serde_json::Map::new();
        anthropic_entry.insert("valid".to_owned(), Value::Bool(false));
        anthropic_entry.insert(
            "reason_code".to_owned(),
            Value::String("provider_key_invalid".to_owned()),
        );
        solstone_core_brain::attach_credential_binding(&mut anthropic_entry, &other_hmac);
        let config = json!({
            "env": {"ANTHROPIC_API_KEY": OTHER_KEY},
            "providers": {
                "active": {"provider": "anthropic", "model": "claude-binding"},
                "key_validation": {
                    "openai": openai_entry,
                    "anthropic": anthropic_entry
                }
            }
        });
        let config_path = journal.join("config/journal.json");
        std::fs::write(
            &config_path,
            serde_json::to_vec(&config).expect("config bytes"),
        )
        .expect("config");
        let config_before = std::fs::read(&config_path).expect("config");
        let active_before = config["providers"]["active"].clone();
        let other_before = config["providers"]["key_validation"]["anthropic"].clone();
        assert!(!config_text(&config_before).contains(CANDIDATE));

        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback");
        let port = listener.local_addr().expect("port").port();
        let receipt_path = root.join("receipt.json");
        let stub = root.join("desktop-probe-stub");
        assert_ne!(
            stub,
            std::env::current_exe().expect("test executable"),
            "the desktop probe stub is a temp executable"
        );
        std::fs::write(&stub, stub_script(port, &receipt_path)).expect("stub");
        make_executable(&stub);

        let validator = super::OneShotKeyValidator::at_path(&stub);
        let probe = std::thread::spawn(move || validator.validate("openai", CANDIDATE));
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("canned desktop probe");
        assert_eq!(body.matches(CANNED).count(), 1);
        assert!(!body.contains(CONTENT_CANARY));
        write_http(&mut stream, PROVIDER_BODY);
        drop(stream);
        let validation = probe.join().expect("probe thread").expect("probe");
        assert_eq!(validation.get("valid").and_then(Value::as_bool), Some(true));
        assert_eq!(std::fs::read(&config_path).expect("config"), config_before);

        let receipt: Value =
            serde_json::from_slice(&std::fs::read(&receipt_path).expect("receipt"))
                .expect("receipt json");
        assert_eq!(receipt["provider"], "openai");
        assert_eq!(receipt["model"], "solstone-key-check");
        assert_eq!(receipt["secret"], CANDIDATE);
        assert!(receipt_path.starts_with(&root));
        assert!(!receipt_path.starts_with(&journal));

        let child = spawn_content_child(&journal, port);
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let output = child.wait_with_output().expect("content child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "content child failed: {stdout}{stderr}"
        );
        let reason = stdout
            .lines()
            .find_map(|line| line.strip_prefix(RECEIPT_LINE))
            .expect("content receipt");
        assert_eq!(reason, "provider_key_invalid");

        let stored = std::fs::read(&config_path).expect("config");
        assert_eq!(stored, config_before);
        let parsed: Value = serde_json::from_slice(&stored).expect("config json");
        assert_eq!(parsed["providers"]["active"], active_before);
        assert_eq!(parsed["env"]["ANTHROPIC_API_KEY"], OTHER_KEY);
        assert_eq!(
            parsed["providers"]["key_validation"]["anthropic"],
            other_before
        );
        let text = config_text(&stored);
        assert!(!text.contains(CANDIDATE));
        assert!(!text.contains(PROVIDER_BODY));
        for (name, before) in parent_env {
            assert_eq!(std::env::var(name).ok(), before, "{name}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    fn content_request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "segment.thought".to_owned(),
            contents: vec![ContentPart::Text {
                text: CONTENT_CANARY.to_owned(),
            }],
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 32,
            timeout_s: Some(5.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: Some(0),
        }
    }

    fn reason_code(result: Result<GenerateResponse, String>) -> String {
        match result {
            Ok(GenerateResponse::Generated(_)) => "generated".to_owned(),
            Ok(GenerateResponse::Cancelled) => "cancelled".to_owned(),
            Ok(GenerateResponse::Refused(refusal)) => refusal
                .reason_code
                .as_ref()
                .map(|code| code.as_wire().to_owned())
                .unwrap_or_else(|| "refused".to_owned()),
            Err(error) => error,
        }
    }

    fn config_text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    // The stub is bash, not an interpreter the Rust gate forbids, so this case
    // runs the same way in the full gate as it does locally.
    fn stub_script(port: u16, receipt_path: &Path) -> String {
        format!(
            r#"#!/bin/bash
set -u
stdin=$(cat)
printf '{{"provider":"%s","model":"%s","secret":"%s"}}' \
    "${{{provider_env}:-}}" "${{{model_env}:-}}" "${{{secret_env}:-}}" > '{receipt_path}'
sentence='Reply with the single word OK.'
case "$stdin" in
    *"$sentence"*) payload=$sentence ;;
    *) payload=STDIN-LACKS-CANNED-SENTENCE ;;
esac
exec 3<>/dev/tcp/127.0.0.1/{port} || exit 1
printf 'POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' \
    "${{#payload}}" "$payload" >&3
response=$(cat <&3)
status_line=${{response%%$'\r'*}}
status=${{status_line#* }}
status=${{status%% *}}
if [ "$status" != 200 ]; then
    printf '%s' "$response" >&2
    exit 1
fi
printf '%s' '{{"schema":"solstone-generate-response-v2","outcome":"generated","text":"OK","model":"solstone-key-check","usage":{{}},"finish_reason":"stop"}}'
"#,
            provider_env = PROVIDER_OVERRIDE_ENV,
            model_env = MODEL_OVERRIDE_ENV,
            secret_env = API_KEY_OVERRIDE_ENV,
            receipt_path = receipt_path.display(),
            port = port,
        )
    }

    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("stub mode");
    }

    fn spawn_content_child(journal: &Path, port: u16) -> std::process::Child {
        Command::new(std::env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg(format!("providers::desktop_probe::{CASE}"))
            .arg("--nocapture")
            .env_clear()
            .env("PATH", "")
            .env(MARKER, CASE)
            .env(JOURNAL_ENV, journal)
            .env(API_KEY_OVERRIDE_ENV, CANDIDATE)
            .env(PROVIDER_OVERRIDE_ENV, "openai")
            .env(BASE_URL_OVERRIDE_ENV, format!("http://127.0.0.1:{port}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("content child")
    }

    fn accept_request(listener: &TcpListener, timeout: Duration) -> Option<(TcpStream, String)> {
        listener.set_nonblocking(true).expect("nonblocking");
        let deadline = Instant::now() + timeout;
        let accepted = loop {
            match listener.accept() {
                Ok((stream, _)) => break Some(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        listener.set_nonblocking(false).expect("blocking");
        let mut stream = accepted?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let body = read_http(&mut stream);
        Some((stream, body))
    }

    fn read_http(stream: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 2048];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    buffer.extend_from_slice(&chunk[..count]);
                    if let Some(split) = buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&buffer[..split]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buffer.len() >= split + 4 + length {
                            break;
                        }
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn write_http(stream: &mut TcpStream, body: &str) {
        let message = format!(
            "HTTP/1.1 200 Desktop\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(message.as_bytes()).expect("response");
        stream.flush().expect("flush");
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod refresh_dispatch {
    use std::io::Read;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde_json::{Value, json};
    use solstone_core_generate::{Cancellation, ContentPart, GenerateRequest, GenerateResponse};
    use solstone_core_generate_wire::overrides::BASE_URL_OVERRIDE_ENV;

    const CASE: &str = "ordinary_refresh_leaves_bound_rejection_and_posts_nothing";
    const MARKER: &str = "SOLSTONE_REFRESH_DISPATCH_CHILD";
    const JOURNAL_ENV: &str = "SOLSTONE_REFRESH_DISPATCH_JOURNAL";
    const RECEIPT: &str = "SOLSTONE_REFRESH_DISPATCH_RECEIPT ";
    const SECRET: &str = "sk-refresh-secret";
    const CANARY: &str = "refresh-canary";

    #[test]
    fn ordinary_refresh_leaves_bound_rejection_and_posts_nothing() {
        if std::env::var(MARKER).ok().as_deref() == Some(CASE) {
            let journal = PathBuf::from(std::env::var(JOURNAL_ENV).expect("journal"));
            let runtime = solstone_core_generate_wire::EndpointRuntime::default();
            let cancellation = Cancellation::never();
            let request = GenerateRequest {
                id: None,
                context: "segment.thought".to_owned(),
                contents: vec![ContentPart::Text {
                    text: CANARY.to_owned(),
                }],
                system_instruction: None,
                temperature: 0.2,
                max_output_tokens: 32,
                timeout_s: Some(30.0),
                json_output: false,
                json_schema: None,
                attempt_index: 0,
                exclusive_admission: false,
                transport_retries: Some(0),
            };
            let reason = match solstone_core_generate_wire::respond(
                &request,
                &runtime,
                &journal,
                |_| None,
                &cancellation,
            ) {
                Ok(GenerateResponse::Refused(refusal)) => refusal
                    .reason_code
                    .as_ref()
                    .map(|code| code.as_wire().to_owned())
                    .unwrap_or_else(|| "refused".to_owned()),
                Ok(GenerateResponse::Generated(_)) => "generated".to_owned(),
                Ok(GenerateResponse::Cancelled) => "cancelled".to_owned(),
                Err(error) => format!("dispatch_error:{error}"),
            };
            println!("{RECEIPT}{reason}");
            return;
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback");
        let port = listener.local_addr().expect("port").port();
        assert_refresh_blocks_content(&listener, port, "short", Some(b"not-32-bytes"));
        assert_refresh_blocks_content(&listener, port, "absent", None);
    }

    fn assert_refresh_blocks_content(
        listener: &TcpListener,
        port: u16,
        label: &str,
        key: Option<&[u8]>,
    ) {
        let journal = std::env::temp_dir().join(format!(
            "solstone-refresh-dispatch-{}-{}-{label}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(journal.join("config")).expect("config");
        let config = json!({
            "env": {"OPENAI_API_KEY": SECRET},
            "providers": {
                "active": {"provider": "openai", "model": "gpt-binding"},
                "key_validation": {
                    "openai": {
                        "valid": false,
                        "reason_code": "provider_key_invalid",
                        "binding_version": 1,
                        "credential_hmac": "ab"
                    }
                }
            }
        });
        let config_path = journal.join("config/journal.json");
        std::fs::write(
            &config_path,
            serde_json::to_vec(&config).expect("config bytes"),
        )
        .expect("config");
        let before = std::fs::read(&config_path).expect("config bytes");
        let key_path = solstone_core_brain::brain_fingerprint_key_path(&journal);
        std::fs::create_dir_all(key_path.parent().expect("health")).expect("health");
        if let Some(original) = key {
            std::fs::write(&key_path, original).expect("short key");
        }
        let error = solstone_core_brain::begin_refresh(
            &journal,
            chrono::Utc::now(),
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .expect_err("bound fingerprint material");
        assert!(matches!(
            error,
            solstone_core_brain::BeginRefreshError::Writer(
                solstone_core_brain::WriterError::Fingerprint(_)
            )
        ));
        match key {
            Some(original) => assert_eq!(std::fs::read(&key_path).expect("key"), original),
            None => assert!(!key_path.exists()),
        }

        let child = Command::new(std::env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg(format!("providers::refresh_dispatch::{CASE}"))
            .arg("--nocapture")
            .env_clear()
            .env("PATH", "")
            .env(MARKER, CASE)
            .env(JOURNAL_ENV, &journal)
            .env(BASE_URL_OVERRIDE_ENV, format!("http://127.0.0.1:{port}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("refresh child");
        assert!(accept_request(listener, Duration::from_secs(2)).is_none());
        let output = child.wait_with_output().expect("refresh child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}{stderr}");
        let reason = stdout
            .lines()
            .find(|line| line.starts_with(RECEIPT))
            .expect("receipt")
            .trim_start_matches(RECEIPT);
        assert_eq!(reason, "provider_key_invalid");
        assert_eq!(std::fs::read(&config_path).expect("config"), before);

        let persisted = super::persist_key_validations(&journal, &super::UnavailableValidator);
        assert!(matches!(persisted, Err(crate::MutationError::StaleProbe)));
        let stored = std::fs::read(&config_path).expect("config");
        assert_eq!(stored, before);
        let parsed: Value = serde_json::from_slice(&stored).expect("json");
        assert_eq!(
            parsed["providers"]["key_validation"]["openai"]["reason_code"],
            "provider_key_invalid"
        );
        match key {
            Some(original) => assert_eq!(std::fs::read(&key_path).expect("key"), original),
            None => assert!(!key_path.exists()),
        }
        let _ = std::fs::remove_dir_all(journal);
    }

    fn accept_request(listener: &TcpListener, timeout: Duration) -> Option<String> {
        listener.set_nonblocking(true).expect("nonblocking");
        let deadline = Instant::now() + timeout;
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking");
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buffer = [0u8; 1024];
                    let count = stream.read(&mut buffer).unwrap_or(0);
                    return Some(String::from_utf8_lossy(&buffer[..count]).into_owned());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }
}
