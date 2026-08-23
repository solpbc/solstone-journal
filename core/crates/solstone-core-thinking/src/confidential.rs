// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Confidential processing operation state and configuration mutations.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use chrono::Utc;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_brain::derive_active_brain_lane;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};

use crate::MutationError;

pub const SERVICE_SPP: &str = "spp";
const OPERATION_GRACE_SECONDS: u64 = 30;
const LOCAL_MODEL: &str = "local/qwen3.5-4b";
const CREDENTIAL_FINGERPRINT_FIELD: &str = "credential_fingerprint_sha256";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Waiting,
    Enabled,
    Pending,
    Revoked,
    NeedsSubscription,
    EarlyAccess,
    Error,
    Other(String),
}

impl Phase {
    fn raw(&self) -> &str {
        match self {
            Self::Starting => "starting",
            Self::Waiting => "waiting",
            Self::Enabled => "enabled",
            Self::Pending => "pending",
            Self::Revoked => "revoked",
            Self::NeedsSubscription => "needs_subscription",
            Self::EarlyAccess => "early_access",
            Self::Error => "error",
            Self::Other(value) => value,
        }
    }

    fn terminal(&self) -> bool {
        match self {
            Self::Enabled
            | Self::NeedsSubscription
            | Self::Revoked
            | Self::EarlyAccess
            | Self::Error => true,
            Self::Starting | Self::Waiting | Self::Pending | Self::Other(_) => false,
        }
    }

    fn product(&self) -> String {
        match self {
            Self::Starting => "starting".to_owned(),
            Self::Waiting => "waiting".to_owned(),
            Self::Enabled => "not_verified".to_owned(),
            Self::EarlyAccess => "early_access".to_owned(),
            Self::Error => "repair_needed".to_owned(),
            Self::Pending | Self::Revoked | Self::NeedsSubscription | Self::Other(_) => {
                self.raw().to_owned()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffResult {
    pub phase: Phase,
    pub guidance: Option<String>,
    pub retryable: bool,
    pub subscribe_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationHandle {
    generation: u64,
}

#[derive(Debug, Clone)]
struct OperationEntry {
    kind: String,
    phase: Phase,
    guidance: Option<String>,
    retryable: bool,
    portal_url: Option<String>,
    subscribe_url: Option<String>,
    started: Instant,
    ended: Option<Instant>,
    generation: u64,
}

#[derive(Default)]
struct RegistryState {
    next_generation: u64,
    entries: HashMap<String, OperationEntry>,
}

pub struct OperationRegistry {
    state: Mutex<RegistryState>,
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self {
            state: Mutex::new(RegistryState::default()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationBusy;

impl OperationRegistry {
    pub fn start_operation(
        &self,
        service: &str,
        kind: &str,
        portal_url: Option<String>,
    ) -> Result<(OperationHandle, Value), OperationBusy> {
        let now = Instant::now();
        let mut state = self
            .state
            .lock()
            .expect("operation registry lock is not poisoned");
        sweep(&mut state.entries, now);
        if state
            .entries
            .get(service)
            .is_some_and(|entry| entry.ended.is_none())
        {
            return Err(OperationBusy);
        }
        state.next_generation = state.next_generation.checked_add(1).ok_or(OperationBusy)?;
        let generation = state.next_generation;
        let entry = OperationEntry {
            kind: kind.to_owned(),
            phase: Phase::Starting,
            guidance: None,
            retryable: false,
            portal_url,
            subscribe_url: None,
            started: now,
            ended: None,
            generation,
        };
        let payload = payload(&entry, now, false);
        state.entries.insert(service.to_owned(), entry);
        Ok((OperationHandle { generation }, payload))
    }

    pub fn mark_waiting(&self, service: &str, handle: OperationHandle) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("operation registry lock is not poisoned");
        let Some(entry) = state.entries.get_mut(service) else {
            return false;
        };
        if entry.generation != handle.generation || entry.ended.is_some() {
            return false;
        }
        entry.phase = Phase::Waiting;
        true
    }

    pub fn finish(&self, service: &str, handle: OperationHandle, result: HandoffResult) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("operation registry lock is not poisoned");
        let Some(entry) = state.entries.get_mut(service) else {
            return false;
        };
        if entry.generation != handle.generation || entry.ended.is_some() {
            return false;
        }
        entry.phase = result.phase;
        entry.guidance = result.guidance;
        entry.retryable = result.retryable;
        entry.subscribe_url = result.subscribe_url;
        entry.ended = Some(Instant::now());
        true
    }

    pub fn operation(&self, service: &str) -> Value {
        self.operation_with_phase_vocabulary(service, true)
    }

    /// Returns an operation using its service-neutral lifecycle phase names.
    ///
    /// SPP's public UI intentionally uses product-specific replacements such
    /// as `not_verified`; other services share this registry but retain the
    /// Python operation registry's raw phase vocabulary.
    pub fn operation_raw(&self, service: &str) -> Value {
        self.operation_with_phase_vocabulary(service, false)
    }

    /// Removes one service operation, primarily for deterministic route tests.
    pub fn clear_operation(&self, service: &str) {
        self.state
            .lock()
            .expect("operation registry lock is not poisoned")
            .entries
            .remove(service);
    }

    fn operation_with_phase_vocabulary(&self, service: &str, product: bool) -> Value {
        let now = Instant::now();
        let mut state = self
            .state
            .lock()
            .expect("operation registry lock is not poisoned");
        sweep(&mut state.entries, now);
        state
            .entries
            .get(service)
            .map(|entry| payload(entry, now, product))
            .unwrap_or(Value::Null)
    }
}

fn sweep(entries: &mut HashMap<String, OperationEntry>, now: Instant) {
    entries.retain(|_, entry| {
        entry
            .ended
            .is_none_or(|ended| now.duration_since(ended).as_secs() <= OPERATION_GRACE_SECONDS)
    });
}

fn payload(entry: &OperationEntry, now: Instant, remap: bool) -> Value {
    let phase = if remap {
        entry.phase.product()
    } else {
        entry.phase.raw().to_owned()
    };
    json!({
        "kind": entry.kind,
        "phase": phase,
        "guidance": entry.guidance,
        "retryable": entry.retryable,
        "portal_url": if entry.phase.terminal() { Value::Null } else { entry.portal_url.clone().map(Value::String).unwrap_or(Value::Null) },
        "subscribe_url": entry.subscribe_url.clone().map(Value::String).unwrap_or(Value::Null),
        "elapsed_ms": now.duration_since(entry.started).as_millis() as u64,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffCode {
    Approved,
    Pending,
    Revoked,
    Expired,
    Malformed,
    NetworkError,
    LocalError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    OutOfDomain,
}

pub fn outcome_from_token(
    token: &str,
    detail: Option<String>,
) -> Result<(HandoffCode, Option<String>), TokenError> {
    let code = match token {
        "consent_link_expired" | "consent_timeout" => HandoffCode::Expired,
        "nonce_invalid" | "unexpected_payload" => HandoffCode::Malformed,
        "portal_unreachable" | "tls_verification_failed" | "relay_unreachable" => {
            HandoffCode::NetworkError
        }
        "write_failed" | "journal_not_initialized" => HandoffCode::LocalError,
        "already_enabled"
        | "manual_key_present"
        | "already_disabled"
        | "spl_already_enabled"
        | "spl_already_disabled"
        | "unknown_service" => return Err(TokenError::OutOfDomain),
        _ => {
            return Ok((
                HandoffCode::LocalError,
                detail.or_else(|| Some(token.to_owned())),
            ));
        }
    };
    Ok((code, detail))
}

pub fn handoff_result(code: HandoffCode) -> HandoffResult {
    match code {
        HandoffCode::Approved => HandoffResult { phase: Phase::Enabled, guidance: None, retryable: false, subscribe_url: None },
        HandoffCode::Pending => HandoffResult { phase: Phase::Pending, guidance: Some("Keep the approval page open while the request finishes.".to_owned()), retryable: false, subscribe_url: None },
        HandoffCode::Revoked => HandoffResult { phase: Phase::Revoked, guidance: Some("Consent was not granted. Start a new enable flow when ready.".to_owned()), retryable: false, subscribe_url: None },
        HandoffCode::Expired => HandoffResult { phase: Phase::Error, guidance: Some("This enable link is no longer active. Start a new enable flow.".to_owned()), retryable: true, subscribe_url: None },
        HandoffCode::Malformed => HandoffResult { phase: Phase::Error, guidance: Some("The service response was not understood. Update solstone and try again.".to_owned()), retryable: false, subscribe_url: None },
        HandoffCode::NetworkError => HandoffResult { phase: Phase::Error, guidance: Some("The service could not be reached. Check network access and try again.".to_owned()), retryable: true, subscribe_url: None },
        HandoffCode::LocalError => HandoffResult { phase: Phase::Error, guidance: Some("Local service state could not be written. Check journal permissions and try again.".to_owned()), retryable: true, subscribe_url: None },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisableOutcome {
    pub was_enabled: bool,
    pub credential_preserved: bool,
}

pub fn confidential_enabled(config: &Map<String, Value>) -> bool {
    derive_active_brain_lane(config).lane.as_deref() == Some("spp")
}

#[derive(Debug)]
pub enum ProvisionError {
    Invalid,
    Mutation(MutationError),
}

pub fn provision_confidential_handoff(
    journal: &Path,
    handoff: &Map<String, Value>,
) -> Result<(), ProvisionError> {
    let values = validate_handoff(handoff).ok_or(ProvisionError::Invalid)?;
    mutate_journal_config(journal, Default::default(), |config| {
        let existing_providers = config.get("providers").and_then(Value::as_object);
        let prior_local = existing_providers
            .and_then(|providers| providers.get("local"))
            .and_then(Value::as_object)
            .filter(|local| !local.is_empty())
            .cloned();
        let prior_active = existing_providers
            .and_then(|providers| providers.get("active"))
            .and_then(Value::as_object)
            .filter(|active| !active.is_empty())
            .cloned();
        let credential = values["credential"].as_str().expect("validated credential");
        let next_local = json!({"endpoint_url":values["endpoint_url"],"served_model_id":values["served_model_id"],"credential":credential});
        let next_active = json!({"provider":"local","model":LOCAL_MODEL});
        let next_service = json!({"account_id":values["account_id"],"endpoint_url":values["endpoint_url"],"served_model_id":values["served_model_id"],"credential_created_at":values["created_at"],"enabled_at":Utc::now().to_rfc3339(),CREDENTIAL_FINGERPRINT_FIELD:fingerprint(credential),"prior_active":prior_active,"prior_local_endpoint":prior_local});
        let changed = existing_providers.and_then(|providers| providers.get("local")) != Some(&next_local)
            || existing_providers.and_then(|providers| providers.get("active")) != Some(&next_active)
            || config.get("services").and_then(Value::as_object).and_then(|services| services.get("confidential")) != Some(&next_service);
        {
            let providers = object_at(config, "providers");
            let local = object_at(providers, "local");
            local.insert("endpoint_url".to_owned(), values["endpoint_url"].clone());
            local.insert(
                "served_model_id".to_owned(),
                values["served_model_id"].clone(),
            );
            local.insert("credential".to_owned(), Value::String(credential.to_owned()));
            providers.insert("active".to_owned(), next_active);
        }
        object_at(config, "services").insert("confidential".to_owned(), next_service);
        JournalConfigMutation { changed, value: () }
    })
    .map_err(MutationError::config)
    .map_err(ProvisionError::Mutation)
    .map(|_| ())
}

pub fn disable_confidential(journal: &Path) -> Result<DisableOutcome, MutationError> {
    mutate_journal_config(journal, Default::default(), |config| {
        let Some(block) = config
            .get("services")
            .and_then(Value::as_object)
            .and_then(|services| services.get("confidential"))
            .and_then(Value::as_object)
            .cloned()
        else {
            return JournalConfigMutation {
                changed: false,
                value: DisableOutcome {
                    was_enabled: false,
                    credential_preserved: false,
                },
            };
        };
        let providers = object_at(config, "providers");
        if providers.get("active") == Some(&json!({"provider":"local","model":LOCAL_MODEL})) {
            match block
                .get("prior_active")
                .and_then(Value::as_object)
                .filter(|prior| !prior.is_empty())
            {
                Some(prior) => {
                    providers.insert("active".to_owned(), Value::Object(prior.clone()));
                }
                None => {
                    providers.remove("active");
                }
            }
        }
        let current_credential = providers
            .get("local")
            .and_then(Value::as_object)
            .and_then(|local| local.get("credential"))
            .and_then(Value::as_str);
        let stored_fingerprint = block
            .get(CREDENTIAL_FINGERPRINT_FIELD)
            .and_then(Value::as_str);
        let mut credential_preserved = true;
        if current_credential
            .zip(stored_fingerprint)
            .is_some_and(|(credential, stored)| fingerprint(credential) == stored)
        {
            let prior = block
                .get("prior_local_endpoint")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            providers.insert("local".to_owned(), Value::Object(prior));
            credential_preserved = false;
        }
        object_at(config, "services").remove("confidential");
        JournalConfigMutation {
            changed: true,
            value: DisableOutcome {
                was_enabled: true,
                credential_preserved,
            },
        }
    })
    .map_err(MutationError::config)
    .map(|transaction| transaction.value)
}

fn validate_handoff(handoff: &Map<String, Value>) -> Option<Map<String, Value>> {
    let mut values = Map::new();
    for field in [
        "endpoint_url",
        "served_model_id",
        "credential",
        "account_id",
        "created_at",
    ] {
        let value = handoff
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())?;
        values.insert(field.to_owned(), Value::String(value.to_owned()));
    }
    let endpoint = normalize_endpoint_url(values["endpoint_url"].as_str().expect("string"))?;
    values.insert("endpoint_url".to_owned(), Value::String(endpoint));
    Some(values)
}

fn normalize_endpoint_url(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    let valid = ["http://", "https://"].iter().any(|prefix| {
        value
            .strip_prefix(prefix)
            .is_some_and(|host| !host.is_empty() && !host.starts_with('/'))
    });
    valid.then(|| {
        value
            .strip_suffix("/v1")
            .unwrap_or(value)
            .trim_end_matches('/')
            .to_owned()
    })
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

fn fingerprint(credential: &str) -> String {
    format!("{:x}", Sha256::digest(credential.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_generation_cannot_change_a_replacement_operation() {
        let registry = OperationRegistry::default();
        let (first, _) = registry
            .start_operation(
                SERVICE_SPP,
                "enable",
                Some("https://portal.example".to_owned()),
            )
            .expect("first operation starts");
        assert!(registry.finish(
            SERVICE_SPP,
            first,
            HandoffResult {
                phase: Phase::Error,
                guidance: None,
                retryable: true,
                subscribe_url: None,
            },
        ));
        let (second, _) = registry
            .start_operation(
                SERVICE_SPP,
                "enable",
                Some("https://portal.example".to_owned()),
            )
            .expect("replacement operation starts");

        assert!(!registry.mark_waiting(SERVICE_SPP, first));
        assert!(!registry.finish(
            SERVICE_SPP,
            first,
            HandoffResult {
                phase: Phase::Enabled,
                guidance: None,
                retryable: false,
                subscribe_url: None,
            },
        ));
        assert!(registry.mark_waiting(SERVICE_SPP, second));
        assert_eq!(registry.operation(SERVICE_SPP)["phase"], "waiting");
    }

    #[test]
    fn exhausted_generation_refuses_to_reuse_an_identity() {
        let registry = OperationRegistry {
            state: Mutex::new(RegistryState {
                next_generation: u64::MAX,
                entries: HashMap::new(),
            }),
        };

        assert!(
            registry
                .start_operation(SERVICE_SPP, "enable", None)
                .is_err()
        );
    }

    #[test]
    fn token_mapping_preserves_supplied_detail_and_rejects_other_services() {
        let (_, detail) =
            outcome_from_token("unmapped", Some("detail".to_owned())).expect("mapped");
        assert_eq!(detail.as_deref(), Some("detail"));
        let (_, detail) = outcome_from_token("unmapped", None).expect("mapped");
        assert_eq!(detail.as_deref(), Some("unmapped"));
        assert_eq!(
            outcome_from_token("unknown_service", None),
            Err(TokenError::OutOfDomain)
        );
    }
}
