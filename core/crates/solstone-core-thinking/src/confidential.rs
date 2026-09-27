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
    entries.retain(|service, entry| {
        held_until_next_start(service, entry)
            || entry
                .ended
                .is_none_or(|ended| now.duration_since(ended).as_secs() <= OPERATION_GRACE_SECONDS)
    });
}

/// A confidential turn-on that failed stays readable until the owner starts
/// another one. The owner is usually in their browser when it fails, so a
/// message that lived only for the grace window was gone before they got back.
fn held_until_next_start(service: &str, entry: &OperationEntry) -> bool {
    service == SERVICE_SPP && entry.ended.is_some() && entry.phase == Phase::Error
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

// What the owner reads when turning confidential processing on doesn't finish.
// Every one of these is a handoff or local failure, never a verification
// result, so each opens by saying plainly that it isn't on.
const PENDING_GUIDANCE: &str = "finish turning it on in your browser.";
const REVOKED_GUIDANCE: &str =
    "confidential processing isn't on. turn it on again when you're ready.";
const EXPIRED_GUIDANCE: &str = "confidential processing isn't on. the link to turn it on expired before you finished in your browser. turn it on again to start over.";
const MALFORMED_GUIDANCE: &str = "confidential processing isn't on. the services portal sent a reply your journal couldn't read. turn it on again. if it keeps happening, update your journal.";
const NETWORK_ERROR_GUIDANCE: &str = "confidential processing isn't on. your journal couldn't reach the services portal. check the connection on the computer your journal is on, then turn it on again.";
const LOCAL_ERROR_GUIDANCE: &str =
    "confidential processing isn't on. your journal couldn't save the setting. turn it on again.";

pub fn handoff_result(code: HandoffCode) -> HandoffResult {
    match code {
        HandoffCode::Approved => HandoffResult {
            phase: Phase::Enabled,
            guidance: None,
            retryable: false,
            subscribe_url: None,
        },
        HandoffCode::Pending => HandoffResult {
            phase: Phase::Pending,
            guidance: Some(PENDING_GUIDANCE.to_owned()),
            retryable: false,
            subscribe_url: None,
        },
        HandoffCode::Revoked => HandoffResult {
            phase: Phase::Revoked,
            guidance: Some(REVOKED_GUIDANCE.to_owned()),
            retryable: false,
            subscribe_url: None,
        },
        HandoffCode::Expired => HandoffResult {
            phase: Phase::Error,
            guidance: Some(EXPIRED_GUIDANCE.to_owned()),
            retryable: true,
            subscribe_url: None,
        },
        HandoffCode::Malformed => HandoffResult {
            phase: Phase::Error,
            guidance: Some(MALFORMED_GUIDANCE.to_owned()),
            retryable: false,
            subscribe_url: None,
        },
        HandoffCode::NetworkError => HandoffResult {
            phase: Phase::Error,
            guidance: Some(NETWORK_ERROR_GUIDANCE.to_owned()),
            retryable: true,
            subscribe_url: None,
        },
        HandoffCode::LocalError => HandoffResult {
            phase: Phase::Error,
            guidance: Some(LOCAL_ERROR_GUIDANCE.to_owned()),
            retryable: true,
            subscribe_url: None,
        },
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

/// Disables confidential processing and restores or preserves provider settings.
///
/// Denominator: `disable_confidential` is the sole remover of `services.confidential`
/// across the codebase. Other writers (`provision_confidential_handoff`,
/// `update_providers`, `update_endpoint`, `clear_endpoint`, `thinking_migration`)
/// only insert, update, or refuse changes while the lane is active; they do not remove
/// `services.confidential`. In-memory handoff phase changes and fingerprinting a cloned block
/// do not remove it, and migration does not remove it.
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

        let service_url = block.get("endpoint_url").and_then(Value::as_str);
        let service_fp = block
            .get(CREDENTIAL_FINGERPRINT_FIELD)
            .and_then(Value::as_str);

        let providers = object_at(config, "providers");
        let current_local = providers
            .get("local")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let current_credential = current_local
            .get("credential")
            .and_then(Value::as_str)
            .map(|s| s.to_owned());
        let current_url = current_local.get("endpoint_url").and_then(Value::as_str);

        let prior_local = block
            .get("prior_local_endpoint")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let prior_active = block
            .get("prior_active")
            .and_then(Value::as_object)
            .filter(|o| !o.is_empty())
            .cloned();

        let fingerprint_matches = match (&current_credential, service_fp) {
            (Some(cred), Some(fp)) => fingerprint(cred) == fp,
            _ => false,
        };

        let address_matches = match (current_url, service_url) {
            (Some(c_url), Some(s_url)) => same_service_address(c_url, s_url),
            _ => false,
        };

        let (mut candidate, is_restore_path) = if fingerprint_matches && address_matches {
            (prior_local, true)
        } else {
            (current_local, false)
        };

        let candidate_url = candidate.get("endpoint_url").and_then(Value::as_str);
        let candidate_address_matches = match (candidate_url, service_url) {
            (Some(c_url), Some(s_url)) => same_service_address(c_url, s_url),
            _ => false,
        };

        if candidate_address_matches {
            candidate.remove("endpoint_url");
            candidate.remove("served_model_id");
            candidate.remove("credential");
        } else {
            let candidate_fp_matches = candidate
                .get("credential")
                .and_then(Value::as_str)
                .zip(service_fp)
                .is_some_and(|(c, fp)| fingerprint(c) == fp);
            if candidate_fp_matches {
                candidate.remove("credential");
            }
        }

        let installed_credential = candidate.get("credential").and_then(Value::as_str);
        let credential_preserved = current_credential.as_deref().is_some()
            && installed_credential == current_credential.as_deref();

        let candidate_has_endpoint = candidate
            .get("endpoint_url")
            .and_then(Value::as_str)
            .is_some();

        providers.insert("local".to_owned(), Value::Object(candidate));

        // Active provider handling
        let active_is_spp_local =
            providers.get("active") == Some(&json!({"provider":"local","model":LOCAL_MODEL}));
        if active_is_spp_local {
            if candidate_has_endpoint {
                if let (true, Some(prior)) = (is_restore_path, prior_active) {
                    providers.insert("active".to_owned(), Value::Object(prior));
                }
            } else if let Some(prior) = &prior_active {
                let prior_provider = prior.get("provider").and_then(Value::as_str).unwrap_or("");
                if solstone_core_brain::is_cloud_byo_provider(prior_provider) {
                    providers.insert("active".to_owned(), Value::Object(prior.clone()));
                }
            }
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

pub(crate) fn same_service_address(left: &str, right: &str) -> bool {
    let parse = |s: &str| -> Option<(String, u16)> {
        let s = s.trim();
        let s = s.strip_suffix('/').unwrap_or(s);
        let s = s.strip_suffix("/v1").unwrap_or(s);
        let s = s.strip_suffix('/').unwrap_or(s);
        let (scheme, rest) = if let Some(rest) = s.strip_prefix("http://") {
            ("http", rest)
        } else {
            let rest = s.strip_prefix("https://")?;
            ("https", rest)
        };
        let authority = rest.split(&['/', '?', '#'][..]).next()?;
        if authority.is_empty() {
            return None;
        }
        let (host, port) = if let Some((h, p_str)) = authority.rsplit_once(':') {
            let port_num = p_str.parse::<u16>().ok()?;
            (h, port_num)
        } else {
            let default_port = if scheme == "https" { 443 } else { 80 };
            (authority, default_port)
        };
        if host.is_empty() {
            return None;
        }
        Some((host.to_ascii_lowercase(), port))
    };

    match (parse(left), parse(right)) {
        (Some((h1, p1)), Some((h2, p2))) => h1 == h2 && p1 == p2,
        _ => false,
    }
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
    fn a_failed_confidential_turn_on_outlives_the_grace_window() {
        let now = Instant::now();
        let long_ago = now
            .checked_sub(std::time::Duration::from_secs(OPERATION_GRACE_SECONDS * 10))
            .expect("clock reaches back past the grace window");
        let ended = |phase: Phase| OperationEntry {
            kind: "enable".to_owned(),
            phase,
            guidance: None,
            retryable: true,
            portal_url: None,
            subscribe_url: None,
            started: long_ago,
            ended: Some(long_ago),
            generation: 1,
        };
        let mut entries = HashMap::from([
            (SERVICE_SPP.to_owned(), ended(Phase::Error)),
            ("spl".to_owned(), ended(Phase::Error)),
        ]);
        sweep(&mut entries, now);
        assert!(entries.contains_key(SERVICE_SPP));
        assert!(!entries.contains_key("spl"));

        let mut entries = HashMap::from([(SERVICE_SPP.to_owned(), ended(Phase::Enabled))]);
        sweep(&mut entries, now);
        assert!(entries.is_empty());
    }

    #[test]
    fn a_held_failure_does_not_block_the_next_turn_on() {
        let registry = OperationRegistry::default();
        let (first, _) = registry
            .start_operation(SERVICE_SPP, "enable", None)
            .expect("first operation starts");
        assert!(registry.finish(SERVICE_SPP, first, handoff_result(HandoffCode::Expired)));
        assert_eq!(registry.operation(SERVICE_SPP)["phase"], "repair_needed");
        assert!(registry.operation(SERVICE_SPP)["guidance"].is_string());

        registry
            .start_operation(SERVICE_SPP, "enable", None)
            .expect("a held failure is replaced by the next start");
        assert_eq!(registry.operation(SERVICE_SPP)["phase"], "starting");
        assert!(registry.operation(SERVICE_SPP)["guidance"].is_null());
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

    #[test]
    fn same_service_address_rules() {
        assert!(same_service_address(
            "http://example.com",
            "http://example.com/"
        ));
        assert!(same_service_address(
            "http://example.com/v1",
            "http://example.com"
        ));
        assert!(same_service_address(
            "https://EXAMPLE.com/v1/",
            "https://example.com"
        ));
        assert!(same_service_address(
            "https://service.example:443",
            "https://service.example"
        ));
        assert!(same_service_address(
            "http://service.example:80",
            "http://service.example"
        ));
        assert!(same_service_address(
            "http://service.example:443",
            "https://service.example"
        ));

        // http port 80 does not match https port 443
        assert!(!same_service_address(
            "http://service.example",
            "https://service.example"
        ));
        assert!(!same_service_address(
            "http://service.example:80",
            "https://service.example:443"
        ));
        assert!(!same_service_address(
            "http://service.example:8080",
            "http://service.example:8081"
        ));
        assert!(!same_service_address(
            "ftp://service.example",
            "http://service.example"
        ));
        assert!(!same_service_address("", "http://service.example"));
    }

    fn write_config(journal: &Path, config: Value) {
        let path = solstone_core_journal_config::get_journal_config_path(journal);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    }

    fn read_config(journal: &Path) -> Value {
        let path = solstone_core_journal_config::get_journal_config_path(journal);
        let data = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&data).unwrap()
    }

    #[test]
    fn disable_confidential_matrix_and_corpus_cases() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();

        // 1. Restore path: current matches service endpoint & fingerprint -> prior local & prior active restored
        let service_fp = fingerprint("handoff-cred");
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://handoff.example/v1",
                        "served_model_id": "handoff-model",
                        "credential": "handoff-cred",
                        "extra": "keep"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        "served_model_id": "handoff-model",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {"provider": "openai", "model": "gpt-5"},
                        "prior_local_endpoint": {
                            "endpoint_url": "https://prior.example",
                            "served_model_id": "prior-model",
                            "credential": "prior-cred"
                        }
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(
            outcome,
            DisableOutcome {
                was_enabled: true,
                credential_preserved: false
            }
        );
        let cfg = read_config(journal);
        assert_eq!(cfg["services"].get("confidential"), None);
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "openai", "model": "gpt-5"})
        );
        assert_eq!(
            cfg["providers"]["local"],
            json!({
                "endpoint_url": "https://prior.example",
                "served_model_id": "prior-model",
                "credential": "prior-cred"
            })
        );

        // 2. Keep path: owner reconfigured local endpoint & credential -> local kept, credential preserved
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://custom.example",
                        "served_model_id": "custom-model",
                        "credential": "custom-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        "served_model_id": "handoff-model",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {"provider": "openai", "model": "gpt-5"},
                        "prior_local_endpoint": {}
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(
            outcome,
            DisableOutcome {
                was_enabled: true,
                credential_preserved: true
            }
        );
        let cfg = read_config(journal);
        assert_eq!(cfg["services"].get("confidential"), None);
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "local", "model": LOCAL_MODEL})
        );
        assert_eq!(
            cfg["providers"]["local"],
            json!({
                "endpoint_url": "https://custom.example",
                "served_model_id": "custom-model",
                "credential": "custom-cred"
            })
        );

        // 3. Keep path: owner reconfigured endpoint but left handoff credential -> credential scrubbed
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://custom.example",
                        "served_model_id": "custom-model",
                        "credential": "handoff-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        "served_model_id": "handoff-model",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {"provider": "openai", "model": "gpt-5"},
                        "prior_local_endpoint": {}
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(
            outcome,
            DisableOutcome {
                was_enabled: true,
                credential_preserved: false
            }
        );
        let cfg = read_config(journal);
        assert_eq!(
            cfg["providers"]["local"],
            json!({
                "endpoint_url": "https://custom.example",
                "served_model_id": "custom-model"
            })
        );

        // 4. Restore path with empty prior_active -> active stays local/LOCAL_MODEL
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://handoff.example",
                        "credential": "handoff-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {},
                        "prior_local_endpoint": {
                            "endpoint_url": "https://prior.example"
                        }
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(outcome.was_enabled, true);
        let cfg = read_config(journal);
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "local", "model": LOCAL_MODEL})
        );
        assert_eq!(
            cfg["providers"]["local"],
            json!({"endpoint_url": "https://prior.example"})
        );

        // 5. Restore path where prior_local had service address -> candidate scrubbed
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://handoff.example",
                        "credential": "handoff-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {"provider": "openai", "model": "gpt-5"},
                        "prior_local_endpoint": {
                            "endpoint_url": "https://handoff.example",
                            "served_model_id": "handoff-model",
                            "credential": "handoff-cred",
                            "parallel_slots": 4
                        }
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(outcome.was_enabled, true);
        let cfg = read_config(journal);
        // Candidate scrub removes endpoint_url, served_model_id, credential, but preserves parallel_slots
        assert_eq!(cfg["providers"]["local"], json!({"parallel_slots": 4}));
        // Since installed local has NO endpoint_url and prior_active is cloud (openai) -> active restored to openai
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "openai", "model": "gpt-5"})
        );

        // 6. Service endpoint, no prior_local, prior_active is cloud -> candidate scrubbed, active restored to cloud
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://handoff.example",
                        "served_model_id": "handoff-model",
                        "credential": "handoff-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {"provider": "anthropic", "model": "claude-3-5"},
                        "prior_local_endpoint": {}
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(outcome.was_enabled, true);
        let cfg = read_config(journal);
        assert_eq!(cfg["providers"]["local"], json!({}));
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "anthropic", "model": "claude-3-5"})
        );

        // 7. Service endpoint, no prior_local, prior_active was local -> active stays local (resolves to bundled)
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://handoff.example",
                        "credential": "handoff-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {"provider": "local", "model": "local/custom"},
                        "prior_local_endpoint": {}
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(outcome.was_enabled, true);
        let cfg = read_config(journal);
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "local", "model": LOCAL_MODEL})
        );

        // 8. Service endpoint, no prior_local, no prior_active -> active stays local
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://handoff.example",
                        "credential": "handoff-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(outcome.was_enabled, true);
        let cfg = read_config(journal);
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "local", "model": LOCAL_MODEL})
        );

        // 9. Confidential block absent -> was_enabled=false, credential_preserved=false, config untouched
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "openai", "model": "gpt-5"},
                    "local": {"endpoint_url": "https://my.example"}
                },
                "services": {}
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(
            outcome,
            DisableOutcome {
                was_enabled: false,
                credential_preserved: false
            }
        );
        let cfg = read_config(journal);
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "openai", "model": "gpt-5"})
        );
        assert_eq!(
            cfg["providers"]["local"],
            json!({"endpoint_url": "https://my.example"})
        );

        // 10. Port 443 explicit matches default 443 in disable_confidential
        write_config(
            journal,
            json!({
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://handoff.example:443",
                        "credential": "handoff-cred"
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": "https://handoff.example",
                        CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                        "prior_active": {"provider": "google", "model": "gemini-1.5"},
                        "prior_local_endpoint": {}
                    }
                }
            }),
        );
        let outcome = disable_confidential(journal).unwrap();
        assert_eq!(outcome.was_enabled, true);
        let cfg = read_config(journal);
        assert_eq!(
            cfg["providers"]["active"],
            json!({"provider": "google", "model": "gemini-1.5"})
        );
    }

    #[test]
    fn turn_off_leaves_neither_service_address_nor_service_credential() {
        let service_url = "https://service.example";
        let service_credential = "service-credential";
        let service_fp = fingerprint(service_credential);

        // Row 1: Clean restore.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://service.example",
                            "served_model_id": "service-model",
                            "credential": "service-credential",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {"provider": "openai", "model": "gpt-5"},
                            "prior_local_endpoint": {
                                "endpoint_url": "https://prior.example/v1",
                                "served_model_id": "prior-model",
                                "credential": "prior-credential",
                                "parallel_slots": 3
                            }
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(
                cfg["providers"]["local"],
                json!({
                    "endpoint_url": "https://prior.example/v1",
                    "served_model_id": "prior-model",
                    "credential": "prior-credential",
                    "parallel_slots": 3
                })
            );
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "openai", "model": "gpt-5"})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-cloud"));
            assert_eq!(resolution.provider, "openai");
        }

        // Row 2: Fingerprint differs, address is not the service.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://owner.example/v1",
                            "served_model_id": "owner-model",
                            "credential": "owner-secret",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {},
                            "prior_local_endpoint": {}
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, true);
            let cfg = read_config(journal);
            assert_eq!(
                cfg["providers"]["local"],
                json!({
                    "endpoint_url": "https://owner.example/v1",
                    "served_model_id": "owner-model",
                    "credential": "owner-secret",
                    "parallel_slots": 3
                })
            );
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-endpoint"));
            assert_eq!(resolution.provider, "local");
        }

        // Row 3: Local credential absent.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://service.example/v1/",
                            "served_model_id": "service-model",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {},
                            "prior_local_endpoint": {}
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(cfg["providers"]["local"], json!({"parallel_slots": 3}));
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("bundled"));
            assert_eq!(resolution.provider, "local");
        }

        // Row 4: Block has no fingerprint field.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://owner.example",
                            "served_model_id": "owner-model",
                            "credential": "owner-secret",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            "prior_active": {},
                            "prior_local_endpoint": {}
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, true);
            let cfg = read_config(journal);
            assert_eq!(
                cfg["providers"]["local"],
                json!({
                    "endpoint_url": "https://owner.example",
                    "served_model_id": "owner-model",
                    "credential": "owner-secret",
                    "parallel_slots": 3
                })
            );
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-endpoint"));
            assert_eq!(resolution.provider, "local");
        }

        // Row 5: Current address equals the service and credential is owner-secret (6 URL variants).
        for url_variant in [
            "https://service.example/v1/",
            "https://service.example/",
            "https://SERVICE.EXAMPLE",
            "https://service.example:443",
            "http://service.example:443",
            "  https://service.example/v1  ",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": url_variant,
                            "served_model_id": "service-model",
                            "credential": "owner-secret",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {},
                            "prior_local_endpoint": {}
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(cfg["providers"]["local"], json!({"parallel_slots": 3}));
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("bundled"));
            assert_eq!(resolution.provider, "local");
        }

        // Row 6: Same as https://service.example/v1/ in row 5, plus prior_active openai / gpt-5.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://service.example/v1/",
                            "served_model_id": "service-model",
                            "credential": "owner-secret",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {"provider": "openai", "model": "gpt-5"},
                            "prior_local_endpoint": {}
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(cfg["providers"]["local"], json!({"parallel_slots": 3}));
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "openai", "model": "gpt-5"})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-cloud"));
            assert_eq!(resolution.provider, "openai");
        }

        // Row 7: http://service.example does not match https://service.example.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "http://service.example",
                            "served_model_id": "owner-model",
                            "credential": "owner-secret",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {},
                            "prior_local_endpoint": {}
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, true);
            let cfg = read_config(journal);
            assert_eq!(
                cfg["providers"]["local"],
                json!({
                    "endpoint_url": "http://service.example",
                    "served_model_id": "owner-model",
                    "credential": "owner-secret",
                    "parallel_slots": 3
                })
            );
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-endpoint"));
            assert_eq!(resolution.provider, "local");
        }

        // Row 8: Saved previous endpoint is the service.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://service.example",
                            "served_model_id": "service-model",
                            "credential": "service-credential",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {},
                            "prior_local_endpoint": {
                                "endpoint_url": "https://SERVICE.example/v1/",
                                "served_model_id": "prior-model",
                                "credential": "service-credential",
                                "parallel_slots": 3
                            }
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(cfg["providers"]["local"], json!({"parallel_slots": 3}));
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("bundled"));
            assert_eq!(resolution.provider, "local");
        }

        // Row 9: Owner endpoint, service credential still installed.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://owner.example",
                            "served_model_id": "owner-model",
                            "credential": "service-credential",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {"provider": "openai", "model": "gpt-5"},
                            "prior_local_endpoint": {
                                "endpoint_url": "https://other.example",
                                "served_model_id": "other",
                                "credential": "other",
                                "parallel_slots": 9
                            }
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(
                cfg["providers"]["local"],
                json!({
                    "endpoint_url": "https://owner.example",
                    "served_model_id": "owner-model",
                    "parallel_slots": 3
                })
            );
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-endpoint"));
            assert_eq!(resolution.provider, "local");
        }

        // Row 10: Restored prior is a different address whose credential is the service credential.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://service.example",
                            "served_model_id": "service-model",
                            "credential": "service-credential",
                            "parallel_slots": 3
                        }
                    },
                    "services": {
                        "confidential": {
                            "endpoint_url": service_url,
                            "served_model_id": "service-model",
                            CREDENTIAL_FINGERPRINT_FIELD: service_fp,
                            "prior_active": {"provider": "openai", "model": "gpt-5"},
                            "prior_local_endpoint": {
                                "endpoint_url": "https://owner.example",
                                "served_model_id": "owner-model",
                                "credential": "service-credential",
                                "parallel_slots": 3
                            }
                        }
                    }
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(
                cfg["providers"]["local"],
                json!({
                    "endpoint_url": "https://owner.example",
                    "served_model_id": "owner-model",
                    "parallel_slots": 3
                })
            );
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "openai", "model": "gpt-5"})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-cloud"));
            assert_eq!(resolution.provider, "openai");
        }

        // Row 11: Already off.
        {
            let temp = tempfile::tempdir().unwrap();
            let journal = temp.path();
            write_config(
                journal,
                json!({
                    "providers": {
                        "active": {"provider": "local", "model": LOCAL_MODEL},
                        "local": {
                            "endpoint_url": "https://service.example",
                            "served_model_id": "service-model",
                            "credential": "service-credential",
                            "parallel_slots": 3
                        }
                    },
                    "services": {}
                }),
            );
            let outcome = disable_confidential(journal).unwrap();
            assert_eq!(outcome.was_enabled, false);
            assert_eq!(outcome.credential_preserved, false);
            let cfg = read_config(journal);
            assert_eq!(
                cfg["providers"]["local"],
                json!({
                    "endpoint_url": "https://service.example",
                    "served_model_id": "service-model",
                    "credential": "service-credential",
                    "parallel_slots": 3
                })
            );
            assert_eq!(
                cfg["providers"]["active"],
                json!({"provider": "local", "model": LOCAL_MODEL})
            );
            let resolution = derive_active_brain_lane(cfg.as_object().unwrap());
            assert_eq!(resolution.lane.as_deref(), Some("byo-endpoint"));
            assert_eq!(resolution.provider, "local");
        }
    }
}
