// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bind a provider authentication rejection to the credential that was sent.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use serde_json::{Map, Value};
use solstone_core_brain::{
    CredentialHoldTarget, StoredCredentialBinding, classify_credential_admission_bound,
};
#[cfg(test)]
use solstone_core_brain::{attach_credential_binding, managed_credential_binding_hmac};
use solstone_core_generate::{ContentPart, GenerateRequest, HEALTH_BRAIN_GENERATE_CONTEXT};
use solstone_core_journal_config::read_journal_config_bound;
use solstone_core_journal_io::{JournalRoot, ObjectIdentity};
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};

use crate::lane::resolve_lane_with;
use crate::overrides::{configured_api_key_with, non_blank_process_env};

/// Admission and retain context for one dispatch call.
///
/// `None` at a provider lookup means the call is journal-free: no admission and no retain.
pub(crate) struct ContentGate<'a> {
    pub journal: &'a Path,
    pub admit_content: bool,
    pub process_overrides: bool,
    pub cancellation: &'a solstone_core_generate::Cancellation<'a>,
}

pub(crate) struct RootWitness {
    root: Arc<JournalRoot>,
    identity: ObjectIdentity,
    admitted_path: PathBuf,
    credential: OnceLock<solstone_core_brain::CredentialValidationWitness>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedAdmission {
    MissingKey,
    Rejected,
    Allow,
}

const CANNED_TEXT: &str = "Reply with the single word OK.";

pub(crate) fn canned_probe(request: &GenerateRequest) -> bool {
    let context_matches = request.context == "settings.cloud.validate_key"
        || request.context == HEALTH_BRAIN_GENERATE_CONTEXT;
    context_matches
        && request.contents.len() == 1
        && matches!(
            &request.contents[0],
            ContentPart::Text { text } if text == CANNED_TEXT
        )
        && request.system_instruction.is_none()
        && request.json_schema.is_none()
        && !request.json_output
        && request.temperature == 0.0
        && request.max_output_tokens == 512
        && request.transport_retries == Some(0)
        && request.attempt_index == 0
        && !request.exclusive_admission
}

pub(crate) fn managed_admission(
    credential: Option<&str>,
    request: &GenerateRequest,
    admit_content: bool,
    binding: StoredCredentialBinding,
) -> ManagedAdmission {
    if credential.is_none() {
        return ManagedAdmission::MissingKey;
    }
    if admit_content && !canned_probe(request) && refuses_content(binding) {
        ManagedAdmission::Rejected
    } else {
        ManagedAdmission::Allow
    }
}

pub(crate) fn endpoint_admission_refuses(
    request: &GenerateRequest,
    admit_content: bool,
    binding: StoredCredentialBinding,
) -> bool {
    admit_content && !canned_probe(request) && refuses_content(binding)
}

fn refuses_content(binding: StoredCredentialBinding) -> bool {
    matches!(
        binding,
        StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt
    )
}

pub(crate) fn admission_binding(
    gate: &ContentGate<'_>,
    witness: Option<&RootWitness>,
    selected_config: &Map<String, Value>,
    target: CredentialHoldTarget<'_>,
) -> Result<StoredCredentialBinding, String> {
    let witness =
        witness.ok_or_else(|| "journal credential authority is unavailable".to_owned())?;
    checkpoint_witness(witness, gate.cancellation)?;
    let provider = match target {
        CredentialHoldTarget::Managed { provider, .. } => provider,
        CredentialHoldTarget::Endpoint { .. } => "local",
    };
    let selected_validation = selected_config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("key_validation"))
        .and_then(Value::as_object)
        .and_then(|entries| entries.get(provider));
    let mut binding = classify_credential_admission_bound(
        witness.root.clone(),
        gate.journal,
        target,
        selected_validation,
    )
    .map_err(|error| error.to_string())?;
    checkpoint_witness(witness, gate.cancellation)?;
    if !refuses_content(binding) {
        if witness.credential.get().is_none() {
            let captured = solstone_core_brain::capture_credential_validation_bound(
                witness.root.clone(),
                gate.journal,
                target,
            )
            .map_err(|error| error.to_string())?;
            witness
                .credential
                .set(captured)
                .map_err(|_| "credential admission already captured".to_owned())?;
        }
        // A rejection learned between the first authority read and witness
        // capture must still refuse this request before its content is sent.
        binding = classify_credential_admission_bound(
            witness.root.clone(),
            gate.journal,
            target,
            selected_validation,
        )
        .map_err(|error| error.to_string())?;
        if refuses_content(binding) {
            return Ok(binding);
        }
        let current = read_journal_config_bound(&witness.root)
            .map_err(|error| error.to_string())?
            .config
            .unwrap_or_default();
        let still_selected = match target {
            CredentialHoldTarget::Managed {
                provider,
                credential,
            } => {
                let env_var = match provider {
                    "openai" => "OPENAI_API_KEY",
                    "anthropic" => "ANTHROPIC_API_KEY",
                    "google" => "GOOGLE_API_KEY",
                    _ => return Err("unsupported credential provider".to_owned()),
                };
                still_sent(
                    &current,
                    gate.process_overrides,
                    env_var,
                    provider,
                    credential,
                )
            }
            CredentialHoldTarget::Endpoint {
                base_url,
                credential,
            } => {
                resolved_provider(&current, gate.process_overrides) == "local"
                    && endpoint_still_sent(&current, base_url, credential)
            }
        };
        if !still_selected {
            return Err("credential selection changed before content admission".to_owned());
        }
    }
    checkpoint_witness(witness, gate.cancellation)?;
    if let Some(credential) = witness.credential.get() {
        credential.checkpoint().map_err(|error| error.to_string())?;
    }
    Ok(binding)
}

/// Whether the final HTTP result is stored as `provider_key_invalid`.
///
/// A missing status is a connection failure and is not stored. This predicate does
/// not replace the live HTTP classifier.
pub(crate) fn retain_provider_key_rejection(
    provider: &str,
    status: Option<u16>,
    body: &str,
    _credential_present: bool,
    confidential: bool,
    bundled: bool,
) -> bool {
    let Some(status) = status else {
        return false;
    };
    match provider {
        "openai" | "anthropic" => status == 401,
        "google" => crate::google::classify_http_failure(status, body) == "provider_key_invalid",
        "local" => status == 401 && !confidential && !bundled,
        _ => false,
    }
}

pub(crate) fn capture_root_witness(journal: &Path) -> Option<RootWitness> {
    let root = Arc::new(JournalRoot::open(journal).ok()?);
    let identity = root.identity();
    let admitted_path = std::path::absolute(journal).ok()?;
    let witness = RootWitness {
        root,
        identity,
        admitted_path,
        credential: OnceLock::new(),
    };
    revalidate_witness(&witness).ok()?;
    Some(witness)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn retain_managed_rejection(
    gate: &ContentGate<'_>,
    witness: Option<&RootWitness>,
    request: &GenerateRequest,
    provider: &str,
    env_var: &str,
    sent_secret: &str,
    status: u16,
    body: &str,
) -> Result<(), RetainFailure> {
    if !gate.admit_content || canned_probe(request) {
        return Ok(());
    }
    if !retain_provider_key_rejection(provider, Some(status), body, true, false, false) {
        return Ok(());
    }
    let Some(witness) = witness else {
        return unpublished_rejection();
    };
    retain_rejection(
        gate,
        witness,
        CredentialHoldTarget::Managed {
            provider,
            credential: sent_secret,
        },
        |config| {
            still_sent(
                config,
                gate.process_overrides,
                env_var,
                provider,
                sent_secret,
            )
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn retain_endpoint_rejection(
    gate: &ContentGate<'_>,
    witness: Option<&RootWitness>,
    request: &GenerateRequest,
    sent_base: &str,
    sent_credential: &str,
    status: u16,
    body: &str,
) -> Result<(), RetainFailure> {
    if !gate.admit_content || canned_probe(request) {
        return Ok(());
    }
    if !retain_provider_key_rejection("local", Some(status), body, false, false, false) {
        return Ok(());
    }
    let Some(witness) = witness else {
        return unpublished_rejection();
    };
    retain_rejection(
        gate,
        witness,
        CredentialHoldTarget::Endpoint {
            base_url: sent_base,
            credential: sent_credential,
        },
        |config| {
            resolved_provider(config, gate.process_overrides) == "local"
                && endpoint_still_sent(config, sent_base, sent_credential)
        },
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RetainFailure {
    Withheld(String),
    Unpublished,
}

fn retain_err(error: impl std::fmt::Display) -> Result<(), RetainFailure> {
    let error = error.to_string();
    log::warn!("provider key rejection was not retained: {error}");
    Err(RetainFailure::Withheld(error))
}

fn unpublished_rejection() -> Result<(), RetainFailure> {
    log::warn!("provider key rejection was not retained: {UNPUBLISHED_REJECTION}");
    Err(RetainFailure::Unpublished)
}

fn revalidate_witness(witness: &RootWitness) -> Result<(), String> {
    witness
        .root
        .revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    if witness.root.identity() != witness.identity {
        return Err("admitted journal root changed".to_owned());
    }
    let current = JournalRoot::open(&witness.admitted_path).map_err(|error| error.to_string())?;
    if current.identity() != witness.identity {
        return Err("admitted journal path changed".to_owned());
    }
    Ok(())
}

fn checkpoint_witness(
    witness: &RootWitness,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<(), String> {
    revalidate_witness(witness)?;
    if cancellation.is_cancelled() {
        return Err("cancelled".to_owned());
    }
    Ok(())
}

fn resolved_provider(config: &Map<String, Value>, process_overrides: bool) -> String {
    if process_overrides {
        resolve_lane_with(config, non_blank_process_env).0
    } else {
        resolve_lane_with(config, |_| None).0
    }
}

fn resolved_secret(
    config: &Map<String, Value>,
    env_var: &str,
    process_overrides: bool,
) -> Option<String> {
    if process_overrides {
        configured_api_key_with(config, env_var, non_blank_process_env)
    } else {
        configured_api_key_with(config, env_var, |_| None)
    }
}

fn still_sent(
    config: &Map<String, Value>,
    process_overrides: bool,
    env_var: &str,
    sent_provider: &str,
    sent_secret: &str,
) -> bool {
    resolved_provider(config, process_overrides) == sent_provider
        && resolved_secret(config, env_var, process_overrides).as_deref() == Some(sent_secret)
}

pub(crate) const UNPUBLISHED_REJECTION: &str =
    "the provider rejected this credential, and the journal could not record that rejection";

pub(crate) fn note_unpublished_retain(
    result: Result<(), RetainFailure>,
    unpublished: &mut Option<String>,
) {
    if let Err(RetainFailure::Unpublished) = result {
        *unpublished = Some(UNPUBLISHED_REJECTION.to_owned());
    }
}

fn retain_rejection(
    gate: &ContentGate<'_>,
    witness: &RootWitness,
    target: CredentialHoldTarget<'_>,
    accept_config: impl Fn(&Map<String, Value>) -> bool,
) -> Result<(), RetainFailure> {
    let checkpoint = || checkpoint_witness(witness, gate.cancellation);
    let Some(credential) = witness.credential.get() else {
        return retain_err("credential rejection has no pre-request witness");
    };
    match solstone_core_brain::retain_current_credential_rejection_bound(
        gate.journal,
        target,
        credential,
        &checkpoint,
        &accept_config,
        None,
    ) {
        Ok(_) => Ok(()),
        Err(solstone_core_brain::CredentialRejectionFailure::Refused(error)) => {
            #[cfg(test)]
            eprintln!("credential retention refused: {error}");
            retain_err(error)
        }
        Err(solstone_core_brain::CredentialRejectionFailure::Unpublished) => {
            #[cfg(test)]
            eprintln!("credential retention unpublished");
            unpublished_rejection()
        }
    }
}

fn endpoint_still_sent(
    config: &Map<String, Value>,
    sent_base: &str,
    sent_credential: &str,
) -> bool {
    match resolve_local_endpoint(config) {
        LocalEndpointResolution::Byo(endpoint) => {
            !endpoint.is_confidential
                && !endpoint.is_bundled
                && endpoint.base_url == sent_base
                && endpoint.credential.as_deref().unwrap_or("") == sent_credential
        }
        LocalEndpointResolution::Bundled => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Direct retention unit controls capture their operation authority before
    // applying cancellation, namespace or storage faults, as admission does.
    fn capture_root_witness(journal: &Path) -> Option<RootWitness> {
        let witness = super::capture_root_witness(journal)?;
        if let Ok(captured) = solstone_core_brain::capture_credential_validation_bound(
            witness.root.clone(),
            journal,
            CredentialHoldTarget::Managed {
                provider: "openai",
                credential: "synthetic-key",
            },
        ) {
            witness.credential.set(captured).unwrap();
        }
        Some(witness)
    }

    fn content_request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "segment.thought".to_owned(),
            contents: vec![ContentPart::Text {
                text: "canary-body".to_owned(),
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

    fn canned_request(context: &str) -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: context.to_owned(),
            contents: vec![ContentPart::Text {
                text: CANNED_TEXT.to_owned(),
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

    #[test]
    fn retain_openai_and_anthropic_401_only() {
        for provider in ["openai", "anthropic"] {
            assert!(retain_provider_key_rejection(
                provider,
                Some(401),
                "{}",
                true,
                false,
                false
            ));
            assert!(!retain_provider_key_rejection(
                provider,
                Some(403),
                "{}",
                true,
                false,
                false
            ));
            assert!(!retain_provider_key_rejection(
                provider,
                Some(403),
                "API_KEY_INVALID",
                true,
                false,
                false
            ));
            assert!(!retain_provider_key_rejection(
                provider, None, "", true, false, false
            ));
        }
    }

    #[test]
    fn retain_google_401_and_api_key_invalid_marker() {
        assert!(retain_provider_key_rejection(
            "google",
            Some(401),
            "{}",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "google",
            Some(400),
            "{\"error\":\"API_KEY_INVALID\"}",
            true,
            false,
            false
        ));
        assert!(retain_provider_key_rejection(
            "google",
            Some(400),
            "{\"error\":{\"status\":\"API_KEY_INVALID\"}}",
            true,
            false,
            false
        ));
        assert!(retain_provider_key_rejection(
            "google",
            Some(400),
            "{\"error\":{\"status\":\"INVALID_ARGUMENT\",\"details\":[{\"reason\":\"API_KEY_INVALID\"}]}}",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "google",
            Some(403),
            "API_KEY_INVALID",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "google",
            Some(429),
            "API_KEY_INVALID",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "google",
            Some(500),
            "{\"error\":{\"message\":\"API_KEY_INVALID\"}}",
            true,
            false,
            false
        ));
    }

    #[test]
    fn retain_skips_opaque_google_403_quota_network_and_5xx() {
        assert_eq!(
            crate::google::classify_http_failure(403, "{}"),
            "provider_response_invalid"
        );
        assert!(!retain_provider_key_rejection(
            "google",
            Some(403),
            "{}",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "google",
            Some(429),
            "{}",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "google",
            Some(503),
            "{}",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "openai",
            Some(500),
            "{}",
            true,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "google", None, "", true, false, false
        ));
        assert!(retain_provider_key_rejection(
            "local",
            Some(401),
            "{}",
            false,
            false,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "local",
            Some(401),
            "{}",
            true,
            true,
            false
        ));
        assert!(!retain_provider_key_rejection(
            "local",
            Some(401),
            "{}",
            true,
            false,
            true
        ));
    }

    fn journal(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "solstone-binding-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("health")).expect("journal");
        path
    }

    fn write_key(path: &std::path::Path) -> [u8; 32] {
        let key = [7u8; 32];
        std::fs::write(path.join("health/brain-fingerprint.key"), key).expect("key");
        key
    }

    fn config_with(provider: &str, entry: Value) -> Map<String, Value> {
        let mut validation = Map::new();
        validation.insert(provider.to_owned(), entry);
        Map::from_iter([(
            "providers".to_owned(),
            serde_json::json!({ "key_validation": validation }),
        )])
    }

    fn classify_fixture_binding(
        journal: &Path,
        config: &Map<String, Value>,
        target: CredentialHoldTarget<'_>,
    ) -> StoredCredentialBinding {
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config/journal.json"),
            serde_json::to_vec(config).unwrap(),
        )
        .unwrap();
        classify_credential_admission_bound(
            Arc::new(JournalRoot::open(journal).unwrap()),
            journal,
            target,
            None,
        )
        .unwrap()
    }

    fn classify_managed_binding(
        journal: &Path,
        config: &Map<String, Value>,
        provider: &str,
        credential: &str,
    ) -> StoredCredentialBinding {
        classify_fixture_binding(
            journal,
            config,
            CredentialHoldTarget::Managed {
                provider,
                credential,
            },
        )
    }

    fn classify_endpoint_binding(
        journal: &Path,
        config: &Map<String, Value>,
        base_url: &str,
        credential: &str,
    ) -> StoredCredentialBinding {
        classify_fixture_binding(
            journal,
            config,
            CredentialHoldTarget::Endpoint {
                base_url,
                credential,
            },
        )
    }

    #[test]
    fn admission_refuses_missing_managed_key() {
        let request = content_request();
        let canned = canned_request("settings.cloud.validate_key");
        assert_eq!(
            managed_admission(None, &request, true, StoredCredentialBinding::Unbound),
            ManagedAdmission::MissingKey
        );
        assert_eq!(
            managed_admission(None, &canned, false, StoredCredentialBinding::Matches),
            ManagedAdmission::MissingKey
        );
    }

    #[test]
    fn admission_refuses_matching_and_corrupt_rejection() {
        let path = journal("matching");
        let key = write_key(&path);
        let hmac = managed_credential_binding_hmac(&key, "openai", "synthetic-key");
        let mut entry = Map::new();
        entry.insert("valid".to_owned(), Value::Bool(false));
        entry.insert(
            "reason_code".to_owned(),
            Value::String("provider_key_invalid".to_owned()),
        );
        attach_credential_binding(&mut entry, &hmac);
        let config = config_with("openai", Value::Object(entry));
        let binding = classify_managed_binding(&path, &config, "openai", "synthetic-key");
        assert_eq!(binding, StoredCredentialBinding::Matches);
        assert_eq!(
            managed_admission(Some("synthetic-key"), &content_request(), true, binding),
            ManagedAdmission::Rejected
        );

        let corrupt = config_with(
            "openai",
            serde_json::json!({
                "valid": false,
                "reason_code": "provider_key_invalid",
                "binding_version": 2,
                "credential_hmac": hmac
            }),
        );
        let binding = classify_managed_binding(&path, &corrupt, "openai", "synthetic-key");
        assert_eq!(binding, StoredCredentialBinding::Corrupt);
        assert_eq!(
            managed_admission(Some("synthetic-key"), &content_request(), true, binding),
            ManagedAdmission::Rejected
        );
        let bare = path.join("health/brain-fingerprint.key");
        assert!(bare.is_file());
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn admission_allows_other_provider_other_key_legacy_and_unavailable() {
        let path = journal("allow");
        let key = write_key(&path);
        let foreign = managed_credential_binding_hmac(&key, "openai", "other-key");
        let mut other_key = Map::new();
        other_key.insert("valid".to_owned(), Value::Bool(false));
        other_key.insert(
            "reason_code".to_owned(),
            Value::String("provider_key_invalid".to_owned()),
        );
        attach_credential_binding(&mut other_key, &foreign);
        let config = config_with("openai", Value::Object(other_key));
        assert_eq!(
            classify_managed_binding(&path, &config, "openai", "synthetic-key"),
            StoredCredentialBinding::DoesNotReject
        );
        assert_eq!(
            classify_managed_binding(&path, &config, "anthropic", "synthetic-key"),
            StoredCredentialBinding::Unbound
        );
        let legacy = config_with(
            "openai",
            serde_json::json!({
                "valid": false,
                "reason_code": "provider_key_invalid",
                "error": "rejected",
                "timestamp": "2026-01-01T00:00:00Z"
            }),
        );
        assert_eq!(
            classify_managed_binding(&path, &legacy, "openai", "synthetic-key"),
            StoredCredentialBinding::Unbound
        );
        let unavailable = config_with(
            "openai",
            serde_json::json!({
                "valid": false,
                "reason_code": "validation_unavailable",
                "binding_version": 1,
                "credential_hmac": foreign
            }),
        );
        assert_eq!(
            classify_managed_binding(&path, &unavailable, "openai", "synthetic-key"),
            StoredCredentialBinding::DoesNotReject
        );
        assert_eq!(
            managed_admission(
                Some("synthetic-key"),
                &content_request(),
                true,
                StoredCredentialBinding::DoesNotReject
            ),
            ManagedAdmission::Allow
        );
        assert_eq!(
            managed_admission(
                Some("synthetic-key"),
                &content_request(),
                true,
                StoredCredentialBinding::Unbound
            ),
            ManagedAdmission::Allow
        );
        let before = std::fs::read(path.join("health/brain-fingerprint.key")).unwrap();
        let _ = classify_managed_binding(&path, &config, "openai", "synthetic-key");
        assert_eq!(
            std::fs::read(path.join("health/brain-fingerprint.key")).unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn admission_allows_keyless_custom_without_evidence() {
        let request = content_request();
        assert!(!endpoint_admission_refuses(
            &request,
            true,
            StoredCredentialBinding::Unbound
        ));
        assert!(!endpoint_admission_refuses(
            &request,
            true,
            StoredCredentialBinding::DoesNotReject
        ));
        assert!(endpoint_admission_refuses(
            &request,
            true,
            StoredCredentialBinding::Matches
        ));
        let path = journal("keyless");
        let config = Map::new();
        assert_eq!(
            classify_endpoint_binding(&path, &config, "http://127.0.0.1:9", ""),
            StoredCredentialBinding::Unbound
        );
        assert!(!path.join("health/brain-fingerprint.key").exists());
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn canned_probe_shape_is_not_refused() {
        for context in ["settings.cloud.validate_key", HEALTH_BRAIN_GENERATE_CONTEXT] {
            let canned = canned_request(context);
            assert!(canned_probe(&canned));
            assert_eq!(
                managed_admission(
                    Some("synthetic-key"),
                    &canned,
                    true,
                    StoredCredentialBinding::Matches
                ),
                ManagedAdmission::Allow
            );
            assert!(!endpoint_admission_refuses(
                &canned,
                true,
                StoredCredentialBinding::Corrupt
            ));
        }
        let mut almost = canned_request("settings.cloud.validate_key");
        almost.contents = vec![ContentPart::Text {
            text: "Reply with OK".to_owned(),
        }];
        assert!(!canned_probe(&almost));
        assert_eq!(
            managed_admission(
                Some("synthetic-key"),
                &almost,
                true,
                StoredCredentialBinding::Matches
            ),
            ManagedAdmission::Rejected
        );
        almost = canned_request("segment.thought");
        almost.contents = vec![ContentPart::Text {
            text: CANNED_TEXT.to_owned(),
        }];
        assert!(!canned_probe(&almost));
    }

    #[test]
    fn retain_managed_rejection_without_content_admission_writes_nothing() {
        let path = journal("retain-config");
        std::fs::create_dir_all(path.join("config")).expect("config directory");
        let config = serde_json::json!({
            "env": {"OPENAI_API_KEY": "synthetic-key"},
            "providers": {"active": {"provider": "openai", "model": "gpt-binding"}}
        });
        std::fs::write(
            path.join("config/journal.json"),
            serde_json::to_vec(&config).expect("config bytes"),
        )
        .expect("config writes");
        let before = std::fs::read(path.join("config/journal.json")).expect("config bytes");
        let witness = capture_root_witness(&path).expect("root witness");
        let cancellation = solstone_core_generate::Cancellation::never();
        let gate = ContentGate {
            journal: &path,
            admit_content: false,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert!(retained.is_ok());
        let after = std::fs::read(path.join("config/journal.json")).expect("config bytes");
        assert_eq!(after, before);
        assert!(!String::from_utf8_lossy(&after).contains("key_validation"));
        assert!(!path.join("health/brain-fingerprint.key").is_file());
        let _ = std::fs::remove_dir_all(path);
    }

    fn planted_openai(name: &str) -> std::path::PathBuf {
        let path = journal(name);
        std::fs::create_dir_all(path.join("config")).expect("config directory");
        std::fs::write(
            path.join("config/journal.json"),
            serde_json::to_vec(&serde_json::json!({
                "env": {"OPENAI_API_KEY": "synthetic-key"},
                "providers": {"active": {"provider": "openai", "model": "gpt-binding"}}
            }))
            .expect("config bytes"),
        )
        .expect("config writes");
        std::fs::write(path.join("health/brain-fingerprint.key"), [9u8; 32]).expect("ready key");
        path
    }

    #[test]
    fn retain_cancellation_before_publication_refuses_the_write() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let path = planted_openai("retain-cancel-publish");
        let before = std::fs::read(path.join("config/journal.json")).expect("config");
        let key_before = std::fs::read(path.join("health/brain-fingerprint.key")).expect("key");
        let witness = capture_root_witness(&path).expect("root witness");
        let calls = AtomicUsize::new(0);
        let check = || calls.fetch_add(1, Ordering::SeqCst) >= 2;
        let cancellation = solstone_core_generate::Cancellation::from_fn(&check);
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert_ne!(retained.expect_err("cancelled"), RetainFailure::Unpublished);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("config"),
            before
        );
        assert_eq!(
            std::fs::read(path.join("health/brain-fingerprint.key")).expect("key"),
            key_before
        );
        assert!(!path.join("config/actions").exists());
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn retain_cancellation_at_post_lock_refuses_before_publication() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let path = planted_openai("retain-cancel-lock");
        let before = std::fs::read(path.join("config/journal.json")).expect("config");
        let witness = capture_root_witness(&path).expect("root witness");
        let calls = AtomicUsize::new(0);
        let check = || calls.fetch_add(1, Ordering::SeqCst) >= 1;
        let cancellation = solstone_core_generate::Cancellation::from_fn(&check);
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert_ne!(retained.expect_err("cancelled"), RetainFailure::Unpublished);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("config"),
            before
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[cfg(windows)]
    #[test]
    fn admitted_root_witness_blocks_rename_until_dropped() {
        let path = planted_openai("witness-rename-lifetime");
        let before = std::fs::read(path.join("config/journal.json")).expect("config");
        let key_before = std::fs::read(path.join("health/brain-fingerprint.key")).expect("key");
        let witness = capture_root_witness(&path).expect("root witness");
        assert!(
            witness.credential.get().is_some(),
            "credential witness captured"
        );
        let moved = path.with_file_name(format!(
            "{}-moved",
            path.file_name().expect("name").to_string_lossy()
        ));
        let error = std::fs::rename(&path, &moved).expect_err("live witness retains its root");
        assert!(matches!(error.raw_os_error(), Some(5 | 32)), "{error}");
        assert!(path.is_dir());
        assert!(!moved.exists());
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("config"),
            before
        );
        assert_eq!(
            std::fs::read(path.join("health/brain-fingerprint.key")).expect("key"),
            key_before
        );
        assert!(!path.join("config/actions").exists());

        drop(witness);
        std::fs::rename(&path, &moved).expect("rename after whole witness drops");
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(moved.join("config/journal.json")).expect("moved config"),
            before
        );
        assert_eq!(
            std::fs::read(moved.join("health/brain-fingerprint.key")).expect("moved key"),
            key_before
        );
        assert!(!moved.join("config/actions").exists());
        std::fs::remove_dir_all(moved).expect("cleanup moved journal");
    }

    #[cfg(unix)]
    #[test]
    fn retain_after_admitted_root_rename_creates_nothing() {
        let path = planted_openai("retain-rename");
        let before = std::fs::read(path.join("config/journal.json")).expect("config");
        let witness = capture_root_witness(&path).expect("root witness");
        let moved = path.with_file_name(format!(
            "{}-moved",
            path.file_name().expect("name").to_string_lossy()
        ));
        std::fs::rename(&path, &moved).expect("rename admitted root");
        let other = path.with_file_name(format!(
            "{}-other",
            path.file_name().expect("name").to_string_lossy()
        ));
        std::fs::create_dir_all(other.join("config")).expect("other config");
        std::fs::create_dir_all(other.join("health")).expect("other health");
        let other_config = br#"{"env":{"OPENAI_API_KEY":"other-key"}}"#;
        let other_key = [6u8; 32];
        std::fs::write(other.join("config/journal.json"), other_config).expect("other config");
        std::fs::write(other.join("health/brain-fingerprint.key"), other_key).expect("other key");
        let cancellation = solstone_core_generate::Cancellation::never();
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert_ne!(
            retained.expect_err("replaced root"),
            RetainFailure::Unpublished
        );
        assert!(!path.exists());
        assert!(!path.join("health/brain-fingerprint.key").exists());
        assert!(!path.join("config/journal.json").exists());
        assert!(!path.join("config/journal.json.lock").exists());
        assert!(!path.join("config/actions").exists());
        assert_eq!(
            std::fs::read(moved.join("config/journal.json")).expect("moved config"),
            before
        );
        assert_eq!(
            std::fs::read(other.join("config/journal.json")).expect("other config"),
            other_config
        );
        assert_eq!(
            std::fs::read(other.join("health/brain-fingerprint.key")).expect("other key"),
            other_key
        );
        let _ = std::fs::remove_dir_all(&moved);
        let _ = std::fs::remove_dir_all(&other);
    }

    fn publication_candidate(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir).ok().is_some_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                solstone_core_journal_io::is_publication_candidate_name(&entry.file_name())
            })
        })
    }

    #[test]
    fn retain_cancellation_at_pre_rename_leaves_the_destination() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let path = planted_openai("retain-prerename");
        let before = std::fs::read(path.join("config/journal.json")).expect("config");
        let key_before = std::fs::read(path.join("health/brain-fingerprint.key")).expect("key");
        let witness = capture_root_witness(&path).expect("root witness");
        let config_dir = path.join("config");
        let calls = AtomicUsize::new(0);
        let check = || {
            calls.fetch_add(1, Ordering::SeqCst);
            publication_candidate(&config_dir)
        };
        let cancellation = solstone_core_generate::Cancellation::from_fn(&check);
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert_ne!(retained.expect_err("cancelled"), RetainFailure::Unpublished);
        assert!(calls.load(Ordering::SeqCst) > 3);
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("config"),
            before
        );
        assert_eq!(
            std::fs::read(path.join("health/brain-fingerprint.key")).expect("key"),
            key_before
        );
        assert!(!publication_candidate(&config_dir));
        assert!(!path.join("config/actions").exists());
        let _ = std::fs::remove_dir_all(path);
    }

    #[cfg(unix)]
    #[test]
    fn retain_after_replacement_at_the_admitted_path_leaves_both_journals() {
        let path = planted_openai("retain-replace");
        let moved_config = std::fs::read(path.join("config/journal.json")).expect("config");
        let moved_key = std::fs::read(path.join("health/brain-fingerprint.key")).expect("key");
        let witness = capture_root_witness(&path).expect("root witness");
        let moved = path.with_file_name(format!(
            "{}-moved",
            path.file_name().expect("name").to_string_lossy()
        ));
        std::fs::rename(&path, &moved).expect("rename admitted root");
        std::fs::create_dir_all(path.join("config")).expect("replacement config");
        std::fs::create_dir_all(path.join("health")).expect("replacement health");
        let replacement_config = br#"{"env":{"OPENAI_API_KEY":"replacement-key"}}"#;
        let replacement_key = [4u8; 32];
        std::fs::write(path.join("config/journal.json"), replacement_config).expect("replacement");
        std::fs::write(path.join("health/brain-fingerprint.key"), replacement_key)
            .expect("replacement key");
        let cancellation = solstone_core_generate::Cancellation::never();
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert_ne!(
            retained.expect_err("replaced root"),
            RetainFailure::Unpublished
        );
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("replacement config"),
            replacement_config
        );
        assert_eq!(
            std::fs::read(path.join("health/brain-fingerprint.key")).expect("replacement key"),
            replacement_key
        );
        assert!(!path.join("config/actions").exists());
        assert_eq!(
            std::fs::read(moved.join("config/journal.json")).expect("moved config"),
            moved_config
        );
        assert_eq!(
            std::fs::read(moved.join("health/brain-fingerprint.key")).expect("moved key"),
            moved_key
        );
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_dir_all(&moved);
    }

    #[test]
    fn config_only_retain_binds_the_config_secret() {
        let path = planted_openai("config-only");
        let witness = capture_root_witness(&path).expect("root witness");
        let cancellation = solstone_core_generate::Cancellation::never();
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: false,
            cancellation: &cancellation,
        };
        retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        )
        .expect("config secret is retained");
        let config = std::fs::read_to_string(path.join("config/journal.json")).expect("config");
        let stored: serde_json::Value = serde_json::from_str(&config).expect("json");
        let hmac = stored["providers"]["key_validation"]["openai"]["credential_hmac"]
            .as_str()
            .expect("hmac")
            .to_owned();
        assert_eq!(
            hmac,
            managed_credential_binding_hmac(&[9u8; 32], "openai", "synthetic-key")
        );
        let before = std::fs::read(path.join("config/journal.json")).expect("config bytes");
        retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "other-secret",
            401,
            "{}",
        )
        .expect_err("the original operation cannot acquire a new credential generation");
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("config bytes"),
            before
        );
        let reread =
            std::fs::read_to_string(path.join("config/journal.json")).expect("config text");
        assert!(!reread.contains("other-secret"));
        assert!(reread.contains(&hmac));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn retain_absent_key_with_existing_binding_returns_err() {
        let path = journal("retain-absent-binding");
        std::fs::create_dir_all(path.join("config")).expect("config");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "env": {"OPENAI_API_KEY": "synthetic-key"},
            "providers": {
                "active": {"provider": "openai", "model": "gpt-binding"},
                "key_validation": {
                    "openai": {
                        "valid": false,
                        "reason_code": "provider_key_invalid",
                        "credential_hmac": "abcd"
                    }
                }
            }
        }))
        .expect("config bytes");
        std::fs::write(path.join("config/journal.json"), &bytes).expect("config");
        let key_path = path.join("health/brain-fingerprint.key");
        assert!(!key_path.exists());
        let witness = capture_root_witness(&path).expect("root witness");
        let cancellation = solstone_core_generate::Cancellation::never();
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: false,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert_eq!(
            retained.expect_err("unpublished"),
            RetainFailure::Unpublished
        );
        assert!(!key_path.exists());
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("config"),
            bytes
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn unavailable_key_cannot_authorize_rejection_and_preserves_the_file() {
        let path = journal("retain-short-key");
        std::fs::create_dir_all(path.join("config")).expect("config");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "env": {"OPENAI_API_KEY": "synthetic-key"},
            "providers": {"active": {"provider": "openai", "model": "gpt-binding"}}
        }))
        .expect("config bytes");
        std::fs::write(path.join("config/journal.json"), &bytes).expect("config");
        let key_path = path.join("health/brain-fingerprint.key");
        let original = b"not-32-bytes".to_vec();
        std::fs::write(&key_path, &original).expect("short key");
        let witness = capture_root_witness(&path).expect("root witness");
        let cancellation = solstone_core_generate::Cancellation::never();
        let gate = ContentGate {
            journal: &path,
            admit_content: true,
            process_overrides: false,
            cancellation: &cancellation,
        };
        let retained = retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            "synthetic-key",
            401,
            "{}",
        );
        assert!(matches!(retained, Err(RetainFailure::Withheld(_))));
        assert_eq!(std::fs::read(&key_path).expect("key bytes"), original);
        assert_eq!(
            std::fs::read(path.join("config/journal.json")).expect("config"),
            bytes
        );
        let _ = std::fs::remove_dir_all(path);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod loopback {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde_json::{Map, Value, json};
    use solstone_core_generate::{Cancellation, ContentPart, GenerateRequest, GenerateResponse};

    use super::CANNED_TEXT;

    const MARKER: &str = "SOLSTONE_BINDING_CHILD";
    const JOURNAL_ENV: &str = "SOLSTONE_BINDING_JOURNAL";
    const RECEIPT: &str = "SOLSTONE_BINDING_RECEIPT ";
    const CANARY: &str = "binding-canary";
    const SECRET: &str = "sk-binding-secret";

    fn is_child(case: &str) -> bool {
        match std::env::var(MARKER) {
            Ok(seen) if seen == case => true,
            Ok(seen) => panic!("stale binding marker {seen} for {case}"),
            Err(_) => false,
        }
    }

    fn content_request() -> GenerateRequest {
        GenerateRequest {
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
        }
    }

    fn canned_request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "settings.cloud.validate_key".to_owned(),
            contents: vec![ContentPart::Text {
                text: CANNED_TEXT.to_owned(),
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

    fn reason_code(result: Result<GenerateResponse, String>) -> String {
        match result {
            Ok(GenerateResponse::Generated(_)) => "generated".to_owned(),
            Ok(GenerateResponse::Cancelled) => "cancelled".to_owned(),
            Ok(GenerateResponse::Refused(refusal)) => refusal
                .reason_code
                .as_ref()
                .map(|code| code.as_wire().to_owned())
                .unwrap_or_else(|| "refused".to_owned()),
            Err(_) => "dispatch_error".to_owned(),
        }
    }

    fn emit(value: &Value) {
        println!("{RECEIPT}{value}");
    }

    fn binding_present(journal: &Path) -> bool {
        std::fs::read_to_string(journal.join("config/journal.json"))
            .map(|text| text.contains("credential_hmac"))
            .unwrap_or(false)
    }

    fn journal_dir(name: &str) -> PathBuf {
        let path = PathBuf::from("/var/tmp").join(format!(
            "solstone-binding-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("config")).expect("journal directory");
        path
    }

    fn write_config(journal: &Path, config: &Value) {
        std::fs::write(
            journal.join("config/journal.json"),
            serde_json::to_vec(config).expect("config bytes"),
        )
        .expect("config writes");
    }

    fn managed_config(provider: &str, env_var: &str, key: Option<&str>, model: &str) -> Value {
        let mut env = Map::new();
        if let Some(key) = key {
            env.insert(env_var.to_owned(), json!(key));
        }
        json!({
            "env": env,
            "providers": {"active": {"provider": provider, "model": model}}
        })
    }

    fn plant_managed_rejection(journal: &Path, provider: &str, credential: &str) -> Value {
        let key = solstone_core_brain::generate_fingerprint_key(journal).expect("binding key");
        let hmac = solstone_core_brain::managed_credential_binding_hmac(&key, provider, credential);
        let mut entry = Map::new();
        entry.insert("valid".to_owned(), Value::Bool(false));
        entry.insert(
            "reason_code".to_owned(),
            Value::String("provider_key_invalid".to_owned()),
        );
        solstone_core_brain::attach_credential_binding(&mut entry, &hmac);
        Value::Object(entry)
    }

    fn child_journal() -> PathBuf {
        PathBuf::from(std::env::var(JOURNAL_ENV).expect("binding journal"))
    }

    fn respond_content() -> String {
        let journal = child_journal();
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        reason_code(crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        ))
    }

    fn respond_canned() -> String {
        let journal = child_journal();
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        reason_code(crate::respond(
            &canned_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        ))
    }

    fn respond_context() -> String {
        let journal = child_journal();
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let priority =
            solstone_core_journal_config::work_priority::WorkPriority::from_environment()
                .expect("work priority");
        let context = crate::GenerateCallContext {
            priority,
            cancellation: &cancellation,
        };
        reason_code(crate::respond_with_context(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &context,
        ))
    }

    struct OwnedChild(Option<Child>);

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn spawn_child(case: &str, journal: &Path, extra: &[(&str, &str)]) -> OwnedChild {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .arg("--exact")
            .arg(format!("credential_binding::loopback::{case}"))
            .arg("--nocapture")
            .env_clear()
            .env("PATH", "")
            .env(MARKER, case)
            .env(JOURNAL_ENV, journal)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra {
            command.env(key, value);
        }
        OwnedChild(Some(command.spawn().expect("binding child")))
    }

    fn finish_child(mut child: OwnedChild) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if child
                .0
                .as_mut()
                .unwrap()
                .try_wait()
                .expect("binding child status")
                .is_some()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "binding child exceeded its deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child
            .0
            .take()
            .unwrap()
            .wait_with_output()
            .expect("binding child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.is_empty() {
            eprintln!("binding fixture child stderr: {stderr}");
        }
        assert!(
            output.status.success(),
            "binding child failed: {stdout}{stderr}"
        );
        let line = stdout
            .lines()
            .find(|line| line.starts_with(RECEIPT))
            .expect("binding receipt");
        serde_json::from_str(&line[RECEIPT.len()..]).expect("binding receipt json")
    }

    struct LoopbackListener {
        socket: TcpListener,
        received: Mutex<Vec<ReceivedRequest>>,
    }

    impl LoopbackListener {
        fn received(&self) -> Vec<ReceivedRequest> {
            self.received.lock().expect("received requests").clone()
        }
    }

    fn bind_listener() -> (LoopbackListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let port = listener.local_addr().expect("loopback port").port();
        (
            LoopbackListener {
                socket: listener,
                received: Mutex::new(Vec::new()),
            },
            port,
        )
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ReceivedRequest {
        method: String,
        target: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl ReceivedRequest {
        fn empty_models_discovery(&self) -> bool {
            self.method == "GET" && self.target == "/v1/models" && self.body.is_empty()
        }

        fn header(&self, name: &str) -> Option<&str> {
            let mut values = self
                .headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str());
            let first = values.next()?;
            values.next().is_none().then_some(first)
        }

        fn header_count(&self, name: &str) -> usize {
            self.headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .count()
        }

        fn matches_content(&self, target: &str, header: &str, value: &str, secret: &str) -> bool {
            self.method == "POST"
                && self.target == target
                && self.header(header) == Some(value)
                && self.body.contains(CANARY)
                && !self.body.contains(secret)
                && serde_json::from_str::<Value>(&self.body).is_ok()
        }
    }

    fn parse_received_request(buffer: &[u8]) -> Result<ReceivedRequest, &'static str> {
        let split = buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or("incomplete headers")?;
        let headers = std::str::from_utf8(&buffer[..split]).map_err(|_| "invalid headers")?;
        let mut lines = headers.lines();
        let request_line = lines.next().ok_or("missing request line")?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().ok_or("missing method")?.to_owned();
        let target = parts.next().ok_or("missing target")?.to_owned();
        if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
            return Err("invalid request line");
        }
        let mut length = None;
        let mut received_headers = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':').ok_or("invalid header")?;
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err("unsupported transfer encoding");
            }
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err("duplicate content length");
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "invalid content length")?,
                );
            }
            received_headers.push((name.to_owned(), value.trim().to_owned()));
        }
        let start = split + 4;
        let end = start
            .checked_add(length.unwrap_or(0))
            .ok_or("content length overflow")?;
        if end != buffer.len() {
            return Err("incomplete or extra request bytes");
        }
        let body = std::str::from_utf8(&buffer[start..end])
            .map_err(|_| "invalid body UTF-8")?
            .to_owned();
        Ok(ReceivedRequest {
            method,
            target,
            headers: received_headers,
            body,
        })
    }

    fn read_http(stream: &mut TcpStream) -> ReceivedRequest {
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
        parse_received_request(&buffer).expect("complete native HTTP request")
    }

    fn write_http(stream: &mut TcpStream, status: u16, body: &str) {
        let message = format!(
            "HTTP/1.1 {status} Binding\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(message.as_bytes());
    }

    fn accept_request(
        listener: &LoopbackListener,
        timeout: Duration,
    ) -> Option<(TcpStream, String)> {
        listener
            .socket
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return None;
            }
            match listener.socket.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("blocking stream");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("read timeout");
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .expect("write timeout");
                    let request = read_http(&mut stream);
                    listener
                        .received
                        .lock()
                        .expect("received requests")
                        .push(request.clone());
                    if request.empty_models_discovery() {
                        // The endpoint's existing served-window discovery is
                        // metadata, not an owner-content POST. Give it a neutral
                        // response without consuming the content status queue.
                        write_http(&mut stream, 200, r#"{"data":[]}"#);
                        continue;
                    }
                    return Some((stream, request.body));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("loopback accept failed: {error}"),
            }
        }
    }

    #[test]
    fn loopback_request_parser_separates_auth_and_refuses_incomplete_content() {
        let raw = b"POST /v1/responses HTTP/1.1\r\nAuthorization: Bearer fixture-secret\r\nContent-Length: 6\r\n\r\ncanary";
        let request = parse_received_request(raw).unwrap();
        assert_eq!(request.body, "canary");
        assert_eq!(
            request.header("authorization"),
            Some("Bearer fixture-secret")
        );
        assert!(!request.body.contains("fixture-secret"));
        let injected = b"POST /v1/responses HTTP/1.1\r\nContent-Length: 14\r\n\r\nfixture-secret";
        assert!(
            parse_received_request(injected)
                .unwrap()
                .body
                .contains("fixture-secret")
        );
        assert!(parse_received_request(&raw[..raw.len() - 1]).is_err());
        assert!(
            parse_received_request(
                b"POST /x HTTP/1.1\r\nContent-Length: 9\r\nContent-Length: 0\r\n\r\n"
            )
            .is_err()
        );
        let metadata = parse_received_request(b"GET /v1/models HTTP/1.1\r\n\r\n").unwrap();
        assert!(metadata.empty_models_discovery());
        for raw in [
            b"POST /v1/models HTTP/1.1\r\n\r\n".as_slice(),
            b"GET /v1/models HTTP/1.1\r\nContent-Length: 6\r\n\r\ncanary",
            b"GET /v1/models-other HTTP/1.1\r\n\r\n",
        ] {
            assert!(
                !parse_received_request(raw)
                    .unwrap()
                    .empty_models_discovery()
            );
        }
    }

    #[test]
    fn loopback_content_witness_rejects_wrong_route_auth_and_body_secret() {
        let body = json!({"input": CANARY}).to_string();
        let raw = format!(
            "POST /v1/responses HTTP/1.1\r\nAuthorization: Bearer {SECRET}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let request = parse_received_request(raw.as_bytes()).unwrap();
        let authorization = format!("Bearer {SECRET}");
        let matches = |request: &ReceivedRequest| {
            request.matches_content("/v1/responses", "authorization", &authorization, SECRET)
        };
        assert!(matches(&request));
        let mut wrong_method = request.clone();
        wrong_method.method = "GET".to_owned();
        let mut wrong_route = request.clone();
        wrong_route.target = "/v1/models".to_owned();
        let mut no_auth = request.clone();
        no_auth.headers.clear();
        let mut wrong_auth = request.clone();
        wrong_auth.headers[0].1 = "Bearer another-key".to_owned();
        let mut duplicate_auth = request.clone();
        duplicate_auth
            .headers
            .push(("authorization".to_owned(), authorization.clone()));
        let mut leaked = request.clone();
        leaked.body = json!({"input": CANARY, "leaked": SECRET}).to_string();
        for mutant in [
            wrong_method,
            wrong_route,
            no_auth,
            wrong_auth,
            duplicate_auth,
            leaked,
        ] {
            assert!(!matches(&mutant), "mutated HTTP witness was accepted");
        }
        let mut keyless = request;
        keyless
            .headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
        assert_eq!(keyless.header_count("authorization"), 0);
        keyless
            .headers
            .push(("Authorization".to_owned(), "Bearer one".to_owned()));
        keyless
            .headers
            .push(("authorization".to_owned(), "Bearer two".to_owned()));
        assert_ne!(
            keyless.header_count("authorization"),
            0,
            "duplicate auth cannot count as absent"
        );
    }

    #[test]
    fn loopback_models_discovery_is_retained_separately_from_content() {
        let (listener, port) = bind_listener();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("metadata connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        assert!(accept_request(&listener, Duration::from_secs(1)).is_none());
        let response = client.join().expect("metadata client");
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with(r#"{"data":[]}"#));
        let received = listener.received();
        assert_eq!(received.len(), 1);
        assert!(received[0].empty_models_discovery());
        assert!(received[0].body.is_empty());
    }

    fn successful_body(provider: &str) -> String {
        match provider {
            "openai" => json!({
                "model": "gpt-binding", "status": "completed",
                "output": [{"content": [{"type": "output_text", "text": "binding output"}]}],
                "usage": {"input_tokens": 2, "output_tokens": 2, "total_tokens": 4}
            }),
            "anthropic" => json!({
                "model": "claude-binding", "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "binding output"}],
                "usage": {"input_tokens": 2, "output_tokens": 2}
            }),
            "google" => json!({
                "modelVersion": "gemini-binding",
                "candidates": [{"content": {"parts": [{"text": "binding output"}]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 2, "candidatesTokenCount": 2, "totalTokenCount": 4}
            }),
            "local" => json!({
                "model": "served-model",
                "choices": [{"message": {"content": "binding output"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 2, "completion_tokens": 2, "total_tokens": 4}
            }),
            _ => panic!("unexpected fixture provider"),
        }.to_string()
    }

    #[test]
    fn loopback_authorized_native_routes_complete_generated_output() {
        const CASE: &str = "loopback_authorized_native_routes_complete_generated_output";
        if is_child(CASE) {
            let journal = child_journal();
            let runtime = crate::EndpointRuntime::default();
            let cancellation = Cancellation::never();
            let result = crate::respond(
                &content_request(),
                &runtime,
                &journal,
                |_| None,
                &cancellation,
            )
            .expect("native Generate result");
            let GenerateResponse::Generated(output) = result else {
                panic!("expected actual Generated response, got {result:?}");
            };
            emit(&json!({"reason": "generated", "text": output.text}));
            return;
        }
        for (provider, env_var, model, target, header, credential) in [
            (
                "openai",
                "OPENAI_API_KEY",
                "gpt-binding",
                "/v1/responses",
                "authorization",
                Some(SECRET),
            ),
            (
                "anthropic",
                "ANTHROPIC_API_KEY",
                "claude-binding",
                "/v1/messages",
                "x-api-key",
                Some(SECRET),
            ),
            (
                "google",
                "GOOGLE_API_KEY",
                "gemini-binding",
                "/v1beta/models/gemini-binding:generateContent",
                "x-goog-api-key",
                Some(SECRET),
            ),
            (
                "local",
                "",
                "served-model",
                "/v1/chat/completions",
                "authorization",
                Some(SECRET),
            ),
            (
                "local",
                "",
                "served-model",
                "/v1/chat/completions",
                "authorization",
                None,
            ),
        ] {
            let journal = journal_dir("native-success");
            let (listener, port) = bind_listener();
            let (name, base) = base_env(port);
            let config = if provider == "local" {
                let mut config = json!({"providers": {
                    "active": {"provider": "local", "model": model},
                    "local": {"endpoint_url": base, "served_model_id": model}
                }});
                if let Some(credential) = credential {
                    config["providers"]["local"]["credential"] = json!(credential);
                }
                config
            } else {
                managed_config(provider, env_var, credential, model)
            };
            write_config(&journal, &config);
            let child = spawn_child(CASE, &journal, &[(&name, &base)]);
            let (mut stream, body) =
                accept_request(&listener, Duration::from_secs(5)).expect("native content request");
            assert!(body.contains(CANARY));
            let requests = listener.received();
            let content: Vec<_> = requests
                .iter()
                .filter(|request| !request.empty_models_discovery())
                .collect();
            assert_eq!(content.len(), 1, "one actual content POST");
            let request = content[0];
            if let Some(credential) = credential {
                let authorization = if header == "authorization" {
                    format!("Bearer {credential}")
                } else {
                    credential.to_owned()
                };
                assert!(
                    request.matches_content(target, header, &authorization, credential),
                    "provider {provider}: {request:?}"
                );
            } else {
                assert_eq!(request.method, "POST");
                assert_eq!(request.target, target);
                assert_eq!(request.header_count("authorization"), 0);
                assert!(serde_json::from_str::<Value>(&request.body).is_ok());
            }
            if provider == "local" {
                let metadata: Vec<_> = requests
                    .iter()
                    .filter(|request| request.empty_models_discovery())
                    .collect();
                assert!(
                    !metadata.is_empty(),
                    "actual endpoint discovery is counted separately"
                );
                assert!(metadata.iter().all(|request| request.body.is_empty()));
            } else {
                assert_eq!(requests.len(), 1);
            }
            write_http(&mut stream, 200, &successful_body(provider));
            let receipt = finish_child(child);
            assert_eq!(receipt["reason"], "generated");
            assert_eq!(receipt["text"], "binding output");
            assert!(!binding_present(&journal));
            std::fs::remove_dir_all(journal).unwrap();
        }
    }

    fn base_env(port: u16) -> (String, String) {
        (
            crate::overrides::BASE_URL_OVERRIDE_ENV.to_owned(),
            format!("http://127.0.0.1:{port}"),
        )
    }

    fn config_text(journal: &Path) -> String {
        std::fs::read_to_string(journal.join("config/journal.json")).unwrap_or_default()
    }

    #[test]
    fn loopback_first_401_posts_canary_then_same_credential_posts_nothing() {
        const CASE: &str = "loopback_first_401_posts_canary_then_same_credential_posts_nothing";
        if is_child(CASE) {
            let journal = child_journal();
            let first = respond_content();
            let second = respond_content();
            emit(&json!({"reasons":[first, second], "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("first-401");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("first post");
        assert!(body.contains(CANARY));
        assert!(!body.contains(SECRET));
        write_http(&mut stream, 401, "{}");
        drop(stream);
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let receipt = finish_child(child);
        assert_eq!(receipt["reasons"][0], "provider_key_invalid");
        assert_eq!(receipt["reasons"][1], "provider_key_invalid");
        assert_eq!(receipt["bound"], true);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_unavailable_unbound_key_refuses_before_content_and_preserves_material() {
        const CASE: &str =
            "loopback_unavailable_unbound_key_refuses_before_content_and_preserves_material";
        if is_child(CASE) {
            emit(&json!({"reason": respond_content()}));
            return;
        }
        let journal = journal_dir("unavailable-binding-key");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        std::fs::create_dir(journal.join("health")).unwrap();
        let key = journal.join("health/brain-fingerprint.key");
        std::fs::write(&key, b"unavailable protected bytes").unwrap();
        let config_before = std::fs::read(journal.join("config/journal.json")).unwrap();
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        assert!(accept_request(&listener, Duration::from_secs(1)).is_none());
        assert_eq!(finish_child(child)["reason"], "provider_unavailable");
        assert!(listener.received().is_empty());
        assert_eq!(std::fs::read(&key).unwrap(), b"unavailable protected bytes");
        assert_eq!(
            std::fs::read(journal.join("config/journal.json")).unwrap(),
            config_before
        );
        assert!(!journal.join("health/brain.json").exists());
        std::fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn loopback_model_only_change_still_blocks_and_corrected_key_posts() {
        const CASE: &str = "loopback_model_only_change_still_blocks_and_corrected_key_posts";
        if is_child(CASE) {
            let phase = std::env::var("SOLSTONE_BINDING_PHASE").unwrap_or_default();
            let journal = child_journal();
            let reason = respond_content();
            emit(&json!({"phase": phase, "reason": reason, "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("model-only");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(
            CASE,
            &journal,
            &[(&name, &base), ("SOLSTONE_BINDING_PHASE", "first")],
        );
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("first post");
        assert!(body.contains(CANARY));
        write_http(&mut stream, 401, "{}");
        let receipt = finish_child(child);
        assert_eq!(receipt["reason"], "provider_key_invalid");
        assert_eq!(receipt["bound"], true);
        let mut config: Value =
            serde_json::from_str(&config_text(&journal)).expect("stored config");
        config["providers"]["active"]["model"] = json!("gpt-binding-other");
        write_config(&journal, &config);
        let blocked = spawn_child(
            CASE,
            &journal,
            &[(&name, &base), ("SOLSTONE_BINDING_PHASE", "blocked")],
        );
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let blocked = finish_child(blocked);
        assert_eq!(blocked["reason"], "provider_key_invalid");
        config["env"]["OPENAI_API_KEY"] = json!("sk-binding-corrected");
        config["providers"]
            .as_object_mut()
            .expect("providers")
            .remove("key_validation");
        write_config(&journal, &config);
        let corrected = spawn_child(
            CASE,
            &journal,
            &[(&name, &base), ("SOLSTONE_BINDING_PHASE", "corrected")],
        );
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("corrected post");
        assert!(body.contains(CANARY));
        assert!(!body.contains("sk-binding-corrected"));
        write_http(&mut stream, 200, "{}");
        let _ = finish_child(corrected);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_other_provider_posts() {
        const CASE: &str = "loopback_other_provider_posts";
        if is_child(CASE) {
            let journal = child_journal();
            emit(&json!({"reason": respond_content(), "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("other-provider");
        let rejection = plant_managed_rejection(&journal, "openai", SECRET);
        let mut config = managed_config(
            "anthropic",
            "ANTHROPIC_API_KEY",
            Some(SECRET),
            "claude-binding",
        );
        config["providers"]["key_validation"] = json!({"openai": rejection});
        config["env"]["OPENAI_API_KEY"] = json!(SECRET);
        write_config(&journal, &config);
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("anthropic post");
        assert!(body.contains(CANARY));
        write_http(&mut stream, 200, "{}");
        let receipt = finish_child(child);
        assert_ne!(receipt["reason"], "provider_key_invalid");
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_respond_with_context_ignores_generate_override() {
        const CASE: &str = "loopback_respond_with_context_ignores_generate_override";
        if is_child(CASE) {
            emit(&json!({"reason": respond_context()}));
            return;
        }
        let journal = journal_dir("context-override");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", None, "gpt-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let key_name = crate::overrides::API_KEY_OVERRIDE_ENV.to_owned();
        let child = spawn_child(CASE, &journal, &[(&name, &base), (&key_name, SECRET)]);
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let receipt = finish_child(child);
        assert_eq!(receipt["reason"], "provider_key_missing");
        assert!(!binding_present(&journal));
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_opaque_403_429_5xx_and_network_do_not_stick() {
        const CASE: &str = "loopback_opaque_403_429_5xx_and_network_do_not_stick";
        if is_child(CASE) {
            let journal = child_journal();
            let mut reasons = Vec::new();
            for _ in 0..5 {
                reasons.push(respond_content());
            }
            emit(&json!({"reasons": reasons, "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("unsticky");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        for (status, body) in [(403, "{}"), (429, "{}"), (500, "{}"), (0, ""), (200, "{}")] {
            let (mut stream, request) =
                accept_request(&listener, Duration::from_secs(5)).expect("provider post");
            assert!(request.contains(CANARY));
            if status == 0 {
                drop(stream);
            } else {
                write_http(&mut stream, status, body);
            }
        }
        let receipt = finish_child(child);
        assert_eq!(receipt["bound"], false);
        assert_eq!(receipt["reasons"].as_array().expect("reasons").len(), 5);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_canned_child_and_respond_with_config_post_while_content_does_not() {
        const CASE: &str =
            "loopback_canned_child_and_respond_with_config_post_while_content_does_not";
        if is_child(CASE) {
            let journal = child_journal();
            let content = respond_content();
            let canned = respond_canned();
            emit(
                &json!({"content": content, "canned": canned, "bound": binding_present(&journal)}),
            );
            return;
        }
        let journal = journal_dir("canned");
        let rejection = plant_managed_rejection(&journal, "openai", SECRET);
        let mut config = managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding");
        config["providers"]["key_validation"] = json!({"openai": rejection});
        write_config(&journal, &config);
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("canned post");
        assert!(body.contains(CANNED_TEXT));
        assert!(!body.contains(CANARY));
        write_http(&mut stream, 200, "{}");
        assert!(accept_request(&listener, Duration::from_secs(1)).is_none());
        let receipt = finish_child(child);
        assert_eq!(receipt["content"], "provider_key_invalid");
        assert_eq!(receipt["bound"], true);

        let (endpoint_listener, endpoint_port) = bind_listener();
        let endpoint = format!("http://127.0.0.1:{endpoint_port}");
        let endpoint_key =
            solstone_core_brain::load_existing_fingerprint_key(&journal).expect("fingerprint key");
        let endpoint_hmac = solstone_core_brain::endpoint_credential_binding_hmac(
            &endpoint_key,
            &endpoint,
            "endpoint-secret",
        );
        let mut entry = Map::new();
        entry.insert("valid".to_owned(), Value::Bool(false));
        entry.insert(
            "reason_code".to_owned(),
            Value::String("provider_key_invalid".to_owned()),
        );
        solstone_core_brain::attach_credential_binding(&mut entry, &endpoint_hmac);
        let local_config = json!({
            "providers": {
                "active": {"provider": "local", "model": "served-model"},
                "local": {
                    "endpoint_url": endpoint,
                    "served_model_id": "served-model",
                    "credential": "endpoint-secret"
                },
                "key_validation": {"local": Value::Object(entry)}
            }
        });
        let local_config = local_config.as_object().expect("config").clone();
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let cleanup = journal.clone();
        let posted = std::thread::spawn(move || {
            crate::respond_with_config(
                &content_request(),
                &runtime,
                &journal,
                &local_config,
                |_| None,
                &cancellation,
            )
        });
        assert!(
            accept_request(&endpoint_listener, Duration::from_secs(2)).is_none(),
            "bound arbitrary content does not post"
        );
        let blocked = posted.join().expect("config respond");
        assert_eq!(reason_code(blocked), "provider_key_invalid");
        let _ = std::fs::remove_dir_all(cleanup);

        let fresh = journal_dir("config-content");
        let fresh_endpoint = endpoint.clone();
        let unbound = json!({
            "providers": {
                "active": {"provider": "local", "model": "served-model"},
                "local": {
                    "endpoint_url": fresh_endpoint,
                    "served_model_id": "served-model",
                    "credential": "endpoint-secret"
                }
            }
        });
        let unbound = unbound.as_object().expect("config").clone();
        write_config(&fresh, &Value::Object(unbound.clone()));
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let fresh_journal = fresh.clone();
        let first = std::thread::spawn(move || {
            crate::respond_with_config(
                &content_request(),
                &runtime,
                &fresh_journal,
                &unbound,
                |_| None,
                &cancellation,
            )
        });
        let (mut stream, body) =
            accept_request(&endpoint_listener, Duration::from_secs(5)).expect("unbound post");
        assert!(body.contains(CANARY));
        write_http(&mut stream, 401, "{}");
        let refused = first.join().expect("first config respond");
        assert_eq!(reason_code(refused), "provider_key_invalid");
        assert!(binding_present(&fresh));
        let stored: Value = serde_json::from_str(&config_text(&fresh)).expect("stored config");
        let key = solstone_core_brain::load_existing_fingerprint_key(&fresh).expect("key");
        let expected = solstone_core_brain::endpoint_credential_binding_hmac(
            &key,
            &endpoint,
            "endpoint-secret",
        );
        assert_eq!(
            stored["providers"]["key_validation"]["local"]["credential_hmac"],
            expected
        );
        let unbound = stored.as_object().expect("object").clone();
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let fresh_journal = fresh.clone();
        let second = std::thread::spawn(move || {
            crate::respond_with_config(
                &content_request(),
                &runtime,
                &fresh_journal,
                &unbound,
                |_| None,
                &cancellation,
            )
        });
        assert!(accept_request(&endpoint_listener, Duration::from_secs(2)).is_none());
        let again = second.join().expect("second config respond");
        assert_eq!(reason_code(again), "provider_key_invalid");
        let _ = std::fs::remove_dir_all(fresh);
    }

    #[test]
    fn loopback_stale_401_after_key_provider_or_root_change_writes_nothing() {
        const CASE: &str = "loopback_stale_401_after_key_provider_or_root_change_writes_nothing";
        if is_child(CASE) {
            let journal = child_journal();
            emit(&json!({"reason": respond_content(), "bound": binding_present(&journal)}));
            return;
        }
        stale_after(&|journal| {
            let mut config: Value = serde_json::from_str(&config_text(journal)).expect("config");
            config["env"]["OPENAI_API_KEY"] = json!("sk-binding-replaced");
            write_config(journal, &config);
            journal.to_path_buf()
        });
        stale_after(&|journal| {
            let mut config: Value = serde_json::from_str(&config_text(journal)).expect("config");
            config["providers"]["active"]["provider"] = json!("anthropic");
            write_config(journal, &config);
            journal.to_path_buf()
        });
        stale_after(&|journal| {
            let renamed = journal.with_extension("renamed");
            std::fs::rename(journal, &renamed).expect("rename journal");
            renamed
        });
    }

    fn stale_after(mutate: &dyn Fn(&Path) -> PathBuf) {
        const CASE: &str = "loopback_stale_401_after_key_provider_or_root_change_writes_nothing";
        let journal = journal_dir("stale");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("stale post");
        assert!(body.contains(CANARY));
        let kept = mutate(&journal);
        write_http(&mut stream, 401, "{}");
        let receipt = finish_child(child);
        assert_eq!(receipt["reason"], "provider_key_invalid");
        assert_eq!(receipt["bound"], false);
        assert!(!config_text(&kept).contains("credential_hmac"));
        let _ = std::fs::remove_dir_all(&journal);
        let _ = std::fs::remove_dir_all(&kept);
    }

    #[test]
    fn loopback_cancelled_call_writes_nothing() {
        let journal = journal_dir("cancelled");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let check = || true;
        let cancellation = Cancellation::from_fn(&check);
        let runtime = crate::EndpointRuntime::default();
        let response = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("cancelled respond");
        assert!(matches!(response, GenerateResponse::Cancelled));
        assert!(!binding_present(&journal));
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_confidential_attestation_refusal_fails_before_content() {
        let (listener, port) = bind_listener();
        let endpoint = format!("http://127.0.0.1:{port}");
        let journal = journal_dir("confidential");
        let credential = "endpoint-credential";
        let fingerprint = sha256_hex(credential);
        write_config(
            &journal,
            &json!({
                "providers": {
                    "active": {"provider": "local", "model": "served-model"},
                    "local": {
                        "endpoint_url": endpoint,
                        "served_model_id": "served-model",
                        "credential": credential
                    }
                },
                "services": {
                    "confidential": {
                        "endpoint_url": endpoint,
                        "served_model_id": "served-model",
                        "credential_fingerprint_sha256": fingerprint
                    }
                }
            }),
        );
        let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let bodies_thread = bodies.clone();
        let stop_thread = stop.clone();
        let server = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                if let Some((mut stream, body)) =
                    accept_request(&listener, Duration::from_millis(50))
                {
                    bodies_thread.lock().expect("bodies").push(body);
                    write_http(&mut stream, 401, "{}");
                }
            }
        });
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let response = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("confidential respond");
        match response {
            GenerateResponse::Refused(refusal) => assert!(matches!(
                refusal.reason,
                solstone_core_generate::RefusalReason::AttestationNotVerified
                    | solstone_core_generate::RefusalReason::AttestationFailed
                    | solstone_core_generate::RefusalReason::AttestationStale
            )),
            other => panic!("attestation refusal expected, got {other:?}"),
        }
        stop.store(true, Ordering::Relaxed);
        let _ = server.join();
        let seen = bodies.lock().expect("bodies");
        assert!(seen.iter().all(|body| !body.contains(CANARY)));
        assert!(!binding_present(&journal));
        let _ = std::fs::remove_dir_all(journal);
    }

    fn sha256_hex(value: &str) -> String {
        ring::digest::digest(&ring::digest::SHA256, value.as_bytes())
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn loopback_keyed_custom_401_blocks_second_post_and_keyless_still_posts() {
        let (listener, port) = bind_listener();
        let endpoint = format!("http://127.0.0.1:{port}");
        let journal = journal_dir("custom");
        write_config(
            &journal,
            &json!({
                "providers": {
                    "active": {"provider": "local", "model": "served-model"},
                    "local": {
                        "endpoint_url": endpoint,
                        "served_model_id": "served-model",
                        "credential": "endpoint-secret"
                    }
                }
            }),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
        let statuses = Arc::new(Mutex::new(std::collections::VecDeque::<u16>::new()));
        let stop_thread = stop.clone();
        let bodies_thread = bodies.clone();
        let statuses_thread = statuses.clone();
        let server = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                if let Some((mut stream, body)) =
                    accept_request(&listener, Duration::from_millis(50))
                {
                    bodies_thread.lock().expect("bodies").push(body);
                    let status = statuses_thread
                        .lock()
                        .expect("statuses")
                        .pop_front()
                        .unwrap_or(200);
                    write_http(&mut stream, status, "{}");
                }
            }
        });
        statuses.lock().expect("statuses").push_back(401);
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let first = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("first custom respond");
        assert_eq!(reason_code(Ok(first)), "provider_key_invalid");
        assert!(binding_present(&journal));
        assert_eq!(bodies.lock().expect("bodies").len(), 1);
        assert!(bodies.lock().expect("bodies")[0].contains(CANARY));
        let second = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("second custom respond");
        assert_eq!(reason_code(Ok(second)), "provider_key_invalid");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(bodies.lock().expect("bodies").len(), 1);
        let mut config: Value = serde_json::from_str(&config_text(&journal)).expect("config");
        config["providers"]["local"]
            .as_object_mut()
            .expect("local")
            .remove("credential");
        write_config(&journal, &config);
        statuses.lock().expect("statuses").push_back(200);
        let third = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("keyless respond");
        assert_ne!(reason_code(Ok(third)), "provider_key_invalid");
        assert_eq!(bodies.lock().expect("bodies").len(), 2);
        assert!(bodies.lock().expect("bodies")[1].contains(CANARY));
        stop.store(true, Ordering::Relaxed);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_reread_journal_blocks_new_respond() {
        const CASE: &str = "loopback_reread_journal_blocks_new_respond";
        if is_child(CASE) {
            let journal = child_journal();
            emit(&json!({"reason": respond_content(), "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("reread");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let first = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("first process");
        assert!(body.contains(CANARY));
        write_http(&mut stream, 401, "{}");
        let receipt = finish_child(first);
        assert_eq!(receipt["reason"], "provider_key_invalid");
        assert_eq!(receipt["bound"], true);
        let second = spawn_child(CASE, &journal, &[(&name, &base)]);
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let receipt = finish_child(second);
        assert_eq!(receipt["reason"], "provider_key_invalid");
        assert_eq!(receipt["bound"], true);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_google_opaque_403_does_not_bind_and_structured_400_does() {
        const CASE: &str = "loopback_google_opaque_403_does_not_bind_and_structured_400_does";
        if is_child(CASE) {
            let journal = child_journal();
            let mut reasons = Vec::new();
            for _ in 0..3 {
                reasons.push(respond_content());
            }
            emit(&json!({"reasons": reasons, "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("google-class");
        write_config(
            &journal,
            &managed_config("google", "GOOGLE_API_KEY", Some(SECRET), "gemini-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("opaque 403");
        assert!(body.contains(CANARY));
        write_http(&mut stream, 403, "{}");
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("structured 400");
        assert!(body.contains(CANARY));
        write_http(
            &mut stream,
            400,
            r#"{"error":{"status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#,
        );
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let receipt = finish_child(child);
        assert_eq!(receipt["reasons"][0], "provider_response_invalid");
        assert_eq!(receipt["reasons"][1], "provider_key_invalid");
        assert_eq!(receipt["reasons"][2], "provider_key_invalid");
        assert_eq!(receipt["bound"], true);
        let stored = config_text(&journal);
        assert!(stored.contains("provider_key_invalid"));
        assert!(!stored.contains("API_KEY_INVALID"));
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_override_secret_is_retained_and_blocks_the_same_override() {
        const CASE: &str = "loopback_override_secret_is_retained_and_blocks_the_same_override";
        const OVERRIDE: &str = "sk-binding-override";
        if is_child(CASE) {
            let journal = child_journal();
            emit(&json!({"reason": respond_content(), "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("override-secret");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let key_name = crate::overrides::API_KEY_OVERRIDE_ENV.to_owned();
        let first = spawn_child(CASE, &journal, &[(&name, &base), (&key_name, OVERRIDE)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("override post");
        assert!(body.contains(CANARY));
        assert!(!body.contains(OVERRIDE));
        assert!(!body.contains(SECRET));
        write_http(&mut stream, 401, "{}");
        let receipt = finish_child(first);
        assert_eq!(receipt["reason"], "provider_key_invalid");
        assert_eq!(receipt["bound"], true);
        let key = solstone_core_brain::load_existing_fingerprint_key(&journal).expect("key");
        let stored: Value = serde_json::from_str(&config_text(&journal)).expect("config");
        assert_eq!(stored["env"]["OPENAI_API_KEY"], SECRET);
        assert_eq!(
            stored["providers"]["key_validation"]["openai"]["credential_hmac"],
            solstone_core_brain::managed_credential_binding_hmac(&key, "openai", OVERRIDE)
        );
        assert_ne!(
            stored["providers"]["key_validation"]["openai"]["credential_hmac"],
            solstone_core_brain::managed_credential_binding_hmac(&key, "openai", SECRET)
        );
        let blocked = spawn_child(CASE, &journal, &[(&name, &base), (&key_name, OVERRIDE)]);
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let blocked = finish_child(blocked);
        assert_eq!(blocked["reason"], "provider_key_invalid");
        let config_call = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("config key still posts");
        assert!(body.contains(CANARY));
        write_http(
            &mut stream,
            200,
            r#"{"model":"gpt-binding","status":"completed","output":[{"content":[{"type":"output_text","text":"OK"}]}]}"#,
        );
        let posted = finish_child(config_call);
        assert_eq!(posted["reason"], "generated");
        let after: Value = serde_json::from_str(&config_text(&journal)).expect("config");
        assert_eq!(after["env"]["OPENAI_API_KEY"], SECRET);
        assert_eq!(
            after["providers"]["key_validation"]["openai"]["credential_hmac"],
            stored["providers"]["key_validation"]["openai"]["credential_hmac"]
        );
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_context_endpoint_binds_config_credential_not_override() {
        const CASE: &str = "loopback_context_endpoint_binds_config_credential_not_override";
        const OTHER: &str = "sk-binding-other";
        if is_child(CASE) {
            emit(&json!({"reason": respond_context()}));
            return;
        }
        let (listener, port) = bind_listener();
        let endpoint = format!("http://127.0.0.1:{port}");
        let journal = journal_dir("context-endpoint");
        write_config(
            &journal,
            &json!({
                "providers": {
                    "active": {"provider": "local", "model": "served-model"},
                    "local": {
                        "endpoint_url": endpoint,
                        "served_model_id": "served-model",
                        "credential": SECRET
                    }
                }
            }),
        );
        let key_name = crate::overrides::API_KEY_OVERRIDE_ENV.to_owned();
        let base_name = crate::overrides::BASE_URL_OVERRIDE_ENV.to_owned();
        let child = spawn_child(
            CASE,
            &journal,
            &[(&key_name, OTHER), (&base_name, "http://127.0.0.1:1")],
        );
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("config endpoint post");
        assert!(body.contains(CANARY));
        assert!(!body.contains(OTHER));
        write_http(&mut stream, 401, "{}");
        let receipt = finish_child(child);
        assert_eq!(receipt["reason"], "provider_key_invalid");
        let key = solstone_core_brain::load_existing_fingerprint_key(&journal).expect("key");
        let stored: Value = serde_json::from_str(&config_text(&journal)).expect("config");
        assert_eq!(
            stored["providers"]["key_validation"]["local"]["credential_hmac"],
            solstone_core_brain::endpoint_credential_binding_hmac(&key, &endpoint, SECRET)
        );
        assert_ne!(
            stored["providers"]["key_validation"]["local"]["credential_hmac"],
            solstone_core_brain::endpoint_credential_binding_hmac(&key, &endpoint, OTHER)
        );
        assert!(!config_text(&journal).contains(OTHER));
        let blocked = spawn_child(
            CASE,
            &journal,
            &[(&key_name, OTHER), (&base_name, "http://127.0.0.1:1")],
        );
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        let blocked = finish_child(blocked);
        assert_eq!(blocked["reason"], "provider_key_invalid");
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_keyless_custom_401_blocks_the_second_post() {
        let (listener, port) = bind_listener();
        let endpoint = format!("http://127.0.0.1:{port}");
        let journal = journal_dir("keyless-401");
        write_config(
            &journal,
            &json!({
                "providers": {
                    "active": {"provider": "local", "model": "served-model"},
                    "local": {
                        "endpoint_url": endpoint,
                        "served_model_id": "served-model"
                    }
                }
            }),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
        let statuses = Arc::new(Mutex::new(std::collections::VecDeque::<u16>::new()));
        let stop_thread = stop.clone();
        let bodies_thread = bodies.clone();
        let statuses_thread = statuses.clone();
        let server = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                if let Some((mut stream, body)) =
                    accept_request(&listener, Duration::from_millis(50))
                {
                    bodies_thread.lock().expect("bodies").push(body);
                    let status = statuses_thread
                        .lock()
                        .expect("statuses")
                        .pop_front()
                        .unwrap_or(200);
                    write_http(&mut stream, status, "{}");
                }
            }
        });
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        statuses.lock().expect("statuses").push_back(200);
        let posted = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("keyless 200");
        let posted_reason = reason_code(Ok(posted));
        assert_ne!(posted_reason, "provider_key_invalid");
        assert_ne!(posted_reason, "provider_key_missing");
        assert!(!binding_present(&journal));
        assert_eq!(bodies.lock().expect("bodies").len(), 1);
        statuses.lock().expect("statuses").push_back(401);
        let refused = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("keyless 401");
        assert_eq!(reason_code(Ok(refused)), "provider_key_missing");
        assert!(binding_present(&journal));
        let stored: Value = serde_json::from_str(&config_text(&journal)).expect("config");
        assert_eq!(
            stored["providers"]["key_validation"]["local"]["reason_code"],
            "provider_key_invalid"
        );
        let key = solstone_core_brain::load_existing_fingerprint_key(&journal).expect("key");
        assert_eq!(
            stored["providers"]["key_validation"]["local"]["credential_hmac"],
            solstone_core_brain::endpoint_credential_binding_hmac(&key, &endpoint, "")
        );
        let blocked = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("bound keyless");
        assert_eq!(reason_code(Ok(blocked)), "provider_key_invalid");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(bodies.lock().expect("bodies").len(), 2);
        let mut config = stored;
        config["providers"]["local"]["credential"] = json!("later-secret");
        write_config(&journal, &config);
        statuses.lock().expect("statuses").push_back(200);
        let changed = crate::respond(
            &content_request(),
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        )
        .expect("changed credential");
        assert_ne!(reason_code(Ok(changed)), "provider_key_invalid");
        assert_eq!(bodies.lock().expect("bodies").len(), 3);
        assert!(bodies.lock().expect("bodies")[2].contains(CANARY));
        stop.store(true, Ordering::Relaxed);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_oneshot_child_override_posts_canned_and_content_posts_nothing() {
        const CASE: &str = "loopback_oneshot_child_override_posts_canned_and_content_posts_nothing";
        const OVERRIDE: &str = "sk-binding-override";
        if is_child(CASE) {
            let journal = child_journal();
            let content = respond_content();
            let canned = respond_canned();
            emit(&json!({
                "content": content,
                "canned": canned,
                "bound": binding_present(&journal)
            }));
            return;
        }
        let journal = journal_dir("oneshot-override");
        let rejection = plant_managed_rejection(&journal, "openai", OVERRIDE);
        let mut config = managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding");
        config["providers"]["key_validation"] = json!({"openai": rejection});
        write_config(&journal, &config);
        let before = config_text(&journal);
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let key_name = crate::overrides::API_KEY_OVERRIDE_ENV.to_owned();
        let child = spawn_child(CASE, &journal, &[(&name, &base), (&key_name, OVERRIDE)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("canned override post");
        assert!(body.contains(CANNED_TEXT));
        assert!(!body.contains(CANARY));
        write_http(&mut stream, 200, "{}");
        assert!(accept_request(&listener, Duration::from_secs(1)).is_none());
        let receipt = finish_child(child);
        assert_eq!(receipt["content"], "provider_key_invalid");
        assert_ne!(receipt["canned"], "provider_key_invalid");
        assert_eq!(config_text(&journal), before);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_embedded_respond_with_config_posts_canned_and_content_posts_nothing() {
        let (listener, port) = bind_listener();
        let endpoint = format!("http://127.0.0.1:{port}");
        let journal = journal_dir("embedded-config");
        let key = solstone_core_brain::generate_fingerprint_key(&journal).expect("key");
        let hmac = solstone_core_brain::endpoint_credential_binding_hmac(&key, &endpoint, SECRET);
        let config = json!({
            "env": {"ANTHROPIC_API_KEY": "anthropic-stored-key"},
            "providers": {
                "active": {"provider": "local", "model": "served-model"},
                "local": {
                    "endpoint_url": endpoint,
                    "served_model_id": "served-model",
                    "credential": SECRET
                },
                "key_validation": {
                    "local": {
                        "valid": false,
                        "reason_code": "provider_key_invalid",
                        "binding_version": 1,
                        "credential_hmac": hmac
                    },
                    "anthropic": {
                        "valid": false,
                        "reason_code": "provider_key_invalid",
                        "binding_version": 1,
                        "credential_hmac": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    }
                }
            }
        });
        write_config(&journal, &config);
        let config_before = std::fs::read(journal.join("config/journal.json")).expect("config");
        let active_before = config["providers"]["active"].clone();
        let other_before = config["providers"]["key_validation"]["anthropic"].clone();
        let config = config.as_object().expect("config").clone();
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let journal_for_content = journal.clone();
        let content_config = config.clone();
        let content = std::thread::spawn(move || {
            crate::respond_with_config(
                &content_request(),
                &runtime,
                &journal_for_content,
                &content_config,
                |_| None,
                &cancellation,
            )
        });
        assert!(accept_request(&listener, Duration::from_secs(2)).is_none());
        assert_eq!(
            reason_code(content.join().expect("content")),
            "provider_key_invalid"
        );
        let runtime = crate::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let journal_for_canned = journal.clone();
        let canned = std::thread::spawn(move || {
            crate::respond_with_config(
                &canned_request(),
                &runtime,
                &journal_for_canned,
                &config,
                |_| None,
                &cancellation,
            )
        });
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("canned config post");
        assert!(body.contains(CANNED_TEXT));
        assert!(!body.contains(CANARY));
        write_http(&mut stream, 200, "{}");
        assert_ne!(
            reason_code(canned.join().expect("canned")),
            "provider_key_invalid"
        );
        let stored = std::fs::read(journal.join("config/journal.json")).expect("config");
        assert_eq!(stored, config_before);
        let parsed: Value = serde_json::from_slice(&stored).expect("config json");
        assert_eq!(parsed["providers"]["active"], active_before);
        assert_eq!(
            parsed["providers"]["key_validation"]["anthropic"],
            other_before
        );
        assert_eq!(parsed["env"]["ANTHROPIC_API_KEY"], "anthropic-stored-key");
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_interrupted_retain_is_not_durable_in_a_later_process() {
        const CASE: &str = "loopback_interrupted_retain_is_not_durable_in_a_later_process";
        if is_child(CASE) {
            let journal = child_journal();
            emit(&json!({"reason": respond_content(), "bound": binding_present(&journal)}));
            return;
        }
        let journal = journal_dir("interrupted");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        std::fs::create_dir_all(journal.join("health")).expect("health");
        std::fs::write(journal.join("health/brain-fingerprint.key"), [9u8; 32]).expect("key");
        let before = std::fs::read(journal.join("config/journal.json")).expect("config");
        let witness = super::capture_root_witness(&journal).expect("witness");
        witness
            .credential
            .set(
                solstone_core_brain::capture_credential_validation_bound(
                    witness.root.clone(),
                    &journal,
                    solstone_core_brain::CredentialHoldTarget::Managed {
                        provider: "openai",
                        credential: SECRET,
                    },
                )
                .expect("pre-operation credential witness"),
            )
            .unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let check = || calls.fetch_add(1, Ordering::SeqCst) >= 2;
        let cancellation = Cancellation::from_fn(&check);
        let gate = super::ContentGate {
            journal: &journal,
            admit_content: true,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = super::retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            SECRET,
            401,
            "{}",
        );
        assert!(retained.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            std::fs::read(journal.join("config/journal.json")).expect("config"),
            before
        );
        assert!(!binding_present(&journal));
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("later process posts");
        assert!(body.contains(CANARY));
        write_http(
            &mut stream,
            200,
            r#"{"model":"gpt-binding","status":"completed","output":[{"content":[{"type":"output_text","text":"OK"}]}]}"#,
        );
        let receipt = finish_child(child);
        assert_eq!(receipt["reason"], "generated");
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn loopback_renamed_root_retain_creates_nothing_at_the_old_path() {
        let journal = journal_dir("rename-retain");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        let before = std::fs::read(journal.join("config/journal.json")).expect("config");
        let witness = super::capture_root_witness(&journal).expect("witness");
        witness
            .credential
            .set(
                solstone_core_brain::capture_credential_validation_bound(
                    witness.root.clone(),
                    &journal,
                    solstone_core_brain::CredentialHoldTarget::Managed {
                        provider: "openai",
                        credential: SECRET,
                    },
                )
                .expect("pre-operation credential witness"),
            )
            .unwrap();
        let moved = journal.with_extension("moved");
        std::fs::rename(&journal, &moved).expect("rename");
        let other = journal.with_extension("other");
        std::fs::create_dir_all(other.join("config")).expect("other");
        let other_bytes = br#"{"env":{"OPENAI_API_KEY":"other-key"}}"#;
        std::fs::write(other.join("config/journal.json"), other_bytes).expect("other config");
        let cancellation = Cancellation::never();
        let gate = super::ContentGate {
            journal: &journal,
            admit_content: true,
            process_overrides: true,
            cancellation: &cancellation,
        };
        let retained = super::retain_managed_rejection(
            &gate,
            Some(&witness),
            &content_request(),
            "openai",
            "OPENAI_API_KEY",
            SECRET,
            401,
            "{}",
        );
        assert!(retained.is_err());
        assert!(!journal.exists());
        assert!(!journal.join("config/journal.json").exists());
        assert!(!journal.join("config/journal.json.lock").exists());
        assert!(!journal.join("health/brain-fingerprint.key").exists());
        assert!(!journal.join("config/actions").exists());
        assert_eq!(
            std::fs::read(moved.join("config/journal.json")).expect("moved"),
            before
        );
        assert_eq!(
            std::fs::read(other.join("config/journal.json")).expect("other"),
            other_bytes
        );
        let _ = std::fs::remove_dir_all(&moved);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[cfg(unix)]
    #[test]
    fn loopback_config_failure_retains_in_brain_and_refuses_a_new_process() {
        use std::os::unix::fs::PermissionsExt;
        const CASE: &str = "loopback_config_failure_retains_in_brain_and_refuses_a_new_process";
        if is_child(CASE) {
            emit(&json!({"reason": respond_content()}));
            return;
        }
        let journal = journal_dir("brain-fallback");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        std::fs::create_dir_all(journal.join("health")).expect("health");
        std::fs::write(journal.join("health/brain-fingerprint.key"), [9u8; 32]).expect("key");
        let before = std::fs::read(journal.join("config/journal.json")).expect("config");
        std::fs::set_permissions(
            journal.join("config"),
            std::fs::Permissions::from_mode(0o555),
        )
        .expect("refuse config publication");
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) = accept_request(&listener, Duration::from_secs(5))
            .expect("first actual provider request");
        assert!(body.contains(CANARY));
        write_http(&mut stream, 401, r#"{"error":"rejected"}"#);
        assert_eq!(finish_child(child)["reason"], "provider_key_invalid");
        assert_eq!(
            std::fs::read(journal.join("config/journal.json")).unwrap(),
            before
        );
        let brain_bytes = std::fs::read(journal.join("health/brain.json")).expect("Brain fallback");
        let brain: Value = serde_json::from_slice(&brain_bytes).unwrap();
        assert_eq!(
            brain["credential_holds"]["openai"]["credential_hmac"],
            solstone_core_brain::managed_credential_binding_hmac(&[9u8; 32], "openai", SECRET)
        );
        assert_eq!(
            brain["credential_holds"]["openai"]["marker_id"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert!(!String::from_utf8_lossy(&brain_bytes).contains(SECRET));
        std::fs::set_permissions(
            journal.join("config"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let reconstructed = spawn_child(CASE, &journal, &[(&name, &base)]);
        assert_eq!(
            finish_child(reconstructed)["reason"],
            "provider_key_invalid"
        );
        assert!(
            accept_request(&listener, Duration::from_millis(100)).is_none(),
            "retained Brain rejection must refuse before another content request"
        );
        assert_eq!(
            std::fs::read(journal.join("config/journal.json")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read(journal.join("health/brain-fingerprint.key")).unwrap(),
            [9u8; 32]
        );
        std::fs::remove_dir_all(journal).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn loopback_unpublished_retain_returns_err_and_a_later_process_posts() {
        use std::os::unix::fs::PermissionsExt;
        const CASE: &str = "loopback_unpublished_retain_returns_err_and_a_later_process_posts";
        const BODY_CANARY: &str = "PROVIDER-BODY-CANARY";
        if is_child(CASE) {
            let journal = child_journal();
            let runtime = crate::EndpointRuntime::default();
            let cancellation = Cancellation::never();
            let result = crate::respond(
                &content_request(),
                &runtime,
                &journal,
                |_| None,
                &cancellation,
            );
            let (kind, message) = match result {
                Ok(GenerateResponse::Generated(_)) => ("generated".to_owned(), String::new()),
                Ok(GenerateResponse::Cancelled) => ("cancelled".to_owned(), String::new()),
                Ok(GenerateResponse::Refused(refusal)) => (
                    "refused".to_owned(),
                    refusal
                        .reason_code
                        .as_ref()
                        .map(|code| code.as_wire().to_owned())
                        .unwrap_or_default(),
                ),
                Err(error) => ("error".to_owned(), error),
            };
            let following = if kind == "error" {
                // Reconstruct the per-call runtime in the surviving process.
                // It must still consult Brain's process-lifetime hold.
                respond_content()
            } else {
                "not_run".to_owned()
            };
            emit(&json!({"kind": kind, "message": message, "following": following}));
            return;
        }
        let journal = journal_dir("unpublished");
        write_config(
            &journal,
            &managed_config("openai", "OPENAI_API_KEY", Some(SECRET), "gpt-binding"),
        );
        std::fs::create_dir_all(journal.join("health")).expect("health");
        std::fs::write(journal.join("health/brain-fingerprint.key"), [9u8; 32]).expect("key");
        let before = std::fs::read(journal.join("config/journal.json")).expect("config");
        std::fs::set_permissions(
            journal.join("config"),
            std::fs::Permissions::from_mode(0o555),
        )
        .expect("config directory mode");
        std::fs::set_permissions(
            journal.join("health"),
            std::fs::Permissions::from_mode(0o555),
        )
        .expect("health directory mode");
        let (listener, port) = bind_listener();
        let (name, base) = base_env(port);
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("definitive rejection post");
        assert!(body.contains(CANARY));
        write_http(&mut stream, 401, &format!(r#"{{"error":"{BODY_CANARY}"}}"#));
        let receipt = finish_child(child);
        assert_eq!(receipt["kind"], "error");
        let message = receipt["message"].as_str().expect("error message");
        assert_eq!(message, super::UNPUBLISHED_REJECTION);
        assert_eq!(receipt["following"], "provider_key_invalid");
        assert!(!message.contains(SECRET));
        assert!(!message.contains(BODY_CANARY));
        assert_eq!(
            std::fs::read(journal.join("config/journal.json")).expect("config"),
            before
        );
        std::fs::set_permissions(
            journal.join("config"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("restore config directory");
        assert!(!journal.join("health/brain.json").exists());
        std::fs::set_permissions(
            journal.join("health"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("restore health directory");
        let child = spawn_child(CASE, &journal, &[(&name, &base)]);
        let (mut stream, body) =
            accept_request(&listener, Duration::from_secs(5)).expect("later process posts");
        assert!(body.contains(CANARY));
        write_http(
            &mut stream,
            200,
            r#"{"model":"gpt-binding","status":"completed","output":[{"content":[{"type":"output_text","text":"OK"}]}]}"#,
        );
        let receipt = finish_child(child);
        assert_eq!(receipt["kind"], "generated");
        let _ = std::fs::remove_dir_all(journal);
    }
}
