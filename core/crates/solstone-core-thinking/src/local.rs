// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only local-model projections.

use std::path::Path;

use serde_json::{Map, Value, json};
pub use solstone_core_brain::RuntimeRetryError;
use solstone_core_brain::derive_active_brain_lane;
use solstone_core_brain::{
    inspect_runtime_health, inspect_runtime_retry_token,
    request_runtime_retry as brain_request_runtime_retry,
};
use solstone_core_facets::append_action_log;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_local::install::{
    lease::is_held,
    metal_candidate,
    readiness::{inspect_local, inspect_local_installed},
    status::{is_in_flight, read_status},
};
use solstone_core_sense::memory::{MemoryProbe, SystemMemoryProbe};

use crate::MutationError;

pub const DEFAULT_MODEL: &str = "local/qwen3.5-4b";

pub fn default_model() -> &'static str {
    DEFAULT_MODEL
}

pub fn accepted_model(model: Option<&str>) -> Option<&'static str> {
    let candidate = model
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_MODEL);
    (candidate == DEFAULT_MODEL).then_some(DEFAULT_MODEL)
}

pub fn invalid_model(model: &str) -> Value {
    json!({"error":"one of those values couldn't be used.","reason_code":"invalid_request_value","detail":format!("Unknown local model: {model}. Must be one of: {}", default_model())})
}

pub fn models() -> Value {
    json!([{"name":DEFAULT_MODEL,"label":"qwen 3.5 4B VLM — 8 GB","min_ram_gb":8,"size_bytes":2_740_937_888_u64}])
}

pub fn availability(journal: &Path, model: &str) -> Value {
    let mut input = Map::from_iter([
        (
            String::from("journal"),
            Value::String(journal.display().to_string()),
        ),
        (String::from("model_id"), Value::String(model.to_owned())),
    ]);
    let readiness = if cfg!(target_os = "macos") {
        input.insert("backend".to_owned(), Value::String("metal".to_owned()));
        metal_candidate::inspect(&input).unwrap_or_else(|_| {
            json!({
                "host":{"platform_supported":false},
                "artifacts":{"binary_installed":false,"model_installed":false},
            })
        })
    } else {
        inspect_local(input)
    };
    availability_payload(model, readiness)
}

/// Read the bundled-local artifacts for the provider-status projection without
/// running host discovery. The explicit availability route remains the caller
/// that performs a current host check.
pub fn provider_status_availability(journal: &Path, model: &str) -> Value {
    availability_payload(model, inspect_local_installed(journal, model))
}

fn availability_payload(model: &str, readiness: Value) -> Value {
    let host = &readiness["host"];
    let artifacts = &readiness["artifacts"];
    let readiness_status = readiness["status"].as_str().unwrap_or("");
    let platform_supported = host["platform_supported"].as_bool().unwrap_or(false);
    let binary_present = artifacts
        .get("binary_installed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let model_present = artifacts["model_installed"].as_bool().unwrap_or(false);
    let memory = SystemMemoryProbe;
    let total_memory_gb = memory.total_bytes().map(bytes_to_gb);
    let available_memory_gb = memory.available_bytes().map(bytes_to_gb);
    let min_ram_gb = 8;
    let download_bytes = 3_413_361_504_u64;
    let (available, reason) = if !platform_supported {
        (
            false,
            "local thinking needs supported hardware on this computer.",
        )
    } else if readiness_status == "host-ineligible" {
        (false, "local runtime cannot start on this computer")
    } else if !binary_present {
        (false, "local runtime is not installed")
    } else if !model_present {
        (false, "local model files are not installed")
    } else {
        (true, "")
    };
    json!({"model":model,"platform_supported":platform_supported,"total_memory_gb":total_memory_gb,"available_memory_gb":available_memory_gb,"min_ram_gb":min_ram_gb,"binary_present":binary_present,"model_present":model_present,"available":available,"reason":reason,"warning":"","download_bytes":download_bytes})
}

pub fn bootstrap_status(journal: &Path, _model: &str) -> Value {
    match read_status(journal, "local") {
        Ok(mut status) => {
            if cfg!(target_os = "macos") && !metal_candidate::status_targets_native(&status) {
                return idle_bootstrap_status();
            }
            if is_in_flight(&status.install_state) && matches!(is_held(journal, "local"), Ok(false))
            {
                status.install_state = "failed".to_owned();
                status.install_error = Some("install_interrupted".to_owned());
            }
            json!({"name":status.provider,"install_state":status.install_state,"last_transition_at":status.last_transition_at,"last_progress_at":status.last_progress_at,"progress_bytes_received":if is_in_flight(&status.install_state) { status.progress_bytes_received } else { None },"progress_bytes_total":if is_in_flight(&status.install_state) { status.progress_bytes_total } else { None },"install_error":status.install_error})
        }
        Err(_) => idle_bootstrap_status(),
    }
}

fn idle_bootstrap_status() -> Value {
    json!({"name":"local","install_state":"idle","last_transition_at":null,"last_progress_at":null,"progress_bytes_received":null,"progress_bytes_total":null,"install_error":null})
}

pub fn runtime(journal: &Path) -> Value {
    let inspection = inspect_runtime_health(journal);
    if inspection.status != "ok" {
        return json!({"status":inspection.status,"phase":if inspection.status == "corrupt" {"state-corrupt"} else {"state-unavailable"},"reason_code":inspection.reason_code,"health_revision":null,"desired_fingerprint_sha256":null,"retry_revision":null,"retry_pending":false,"can_retry":false,"poll":false,"updated_at":null});
    }
    let record = inspection.record.unwrap_or(Value::Null);
    let phase = record["phase"].as_str().unwrap_or("stopped");
    let poll = matches!(
        phase,
        "observing"
            | "artifact-not-ready"
            | "host-blocked"
            | "starting"
            | "warming"
            | "backoff"
            | "retry-requested"
            | "stop-deferred"
            | "stopping"
    );
    let retry_inspection = inspect_runtime_retry_token(journal, "local");
    let retry_record = retry_inspection.record.unwrap_or(Value::Null);
    let retry_revision = retry_record["revision"].as_u64().unwrap_or(0);
    let retry_pending = !retry_record["token_id"].is_null();
    json!({"status":runtime_status_for_phase(phase),"phase":phase,"reason_code":record["reason_code"],"health_revision":record["revision"],"desired_fingerprint_sha256":record["desired_fingerprint_sha256"],"retry_revision":retry_revision,"retry_pending":retry_pending,"can_retry":phase=="failed" && !record["desired_fingerprint_sha256"].is_null() && !retry_pending,"poll":poll,"updated_at":record["updated_at"]})
}

/// `status` is whether the runtime can run, not whether the health record was readable.
/// Inspection failures already return above. A readable blocked record is not `ok`.
fn runtime_status_for_phase(phase: &str) -> &'static str {
    match phase {
        "ready" | "starting" | "warming" | "observing" | "backoff" | "retry-requested"
        | "stop-deferred" | "stopping" => "ok",
        "host-blocked" | "artifact-not-ready" | "not-desired" | "stopped" => "blocked",
        "failed" | "cleanup-failed" => "failed",
        "state-corrupt" => "corrupt",
        "state-unavailable" | "ready-proof-unavailable" => "unavailable",
        _ => "unknown",
    }
}

/// Request a runtime retry token for the bundled local provider, then return
/// the refreshed runtime projection. The five-check CAS lives in
/// `solstone-core-brain` beside `inspect_runtime_health`, per the
/// domain-ownership table (CLAUDE.md §7 L2) -- this is transport only.
pub fn request_runtime_retry(
    journal: &Path,
    health_revision: u64,
    retry_revision: u64,
    desired_fingerprint_sha256: &str,
) -> Result<Value, RuntimeRetryError> {
    brain_request_runtime_retry(
        journal,
        "local",
        health_revision,
        retry_revision,
        desired_fingerprint_sha256,
        Map::new(),
    )?;
    Ok(runtime(journal))
}

/// Every reachable disposition of a `POST /api/local/bootstrap` request.
#[derive(Debug, Clone, PartialEq)]
pub enum BootstrapResponse {
    /// The model is already installed and ready.
    Installed,
    /// An install attempt (of any origin) is already in flight.
    InFlight(String),
    /// The install lease is held by someone else and nothing is in flight.
    Busy(String),
    /// A BYO local endpoint is configured; the bundled model can't bootstrap.
    ByoEndpointActive,
    /// The host can't run the bundled model at all (proof unavailable / host
    /// ineligible), carrying the reference's reason code as the detail.
    HostIneligible(String),
    /// A fresh install is eligible to start, but this wave ships no native
    /// install-spawn primitive (Fact 8) -- report a truthful failure rather
    /// than a false in-progress response.
    SpawnUnavailable,
    /// A readiness/lease/status inspector itself failed.
    Unavailable(String),
}

pub fn start_bootstrap(
    journal: &Path,
    config: &Map<String, Value>,
    model: &str,
) -> BootstrapResponse {
    if !matches!(
        resolve_local_endpoint(config),
        LocalEndpointResolution::Bundled
    ) {
        return BootstrapResponse::ByoEndpointActive;
    }
    let mut input = Map::from_iter([
        (
            "journal".to_owned(),
            Value::String(journal.display().to_string()),
        ),
        ("model_id".to_owned(), Value::String(model.to_owned())),
    ]);
    let readiness = if cfg!(target_os = "macos") {
        input.insert("backend".to_owned(), Value::String("metal".to_owned()));
        metal_candidate::inspect(&input).unwrap_or_else(|_| {
            json!({
                "ready":false,
                "status":"proof-unavailable",
                "reason_code":"readiness_unavailable",
            })
        })
    } else {
        inspect_local(input)
    };
    classify_bootstrap(journal, &readiness)
}

fn classify_bootstrap(journal: &Path, readiness: &Value) -> BootstrapResponse {
    if readiness["ready"].as_bool() == Some(true) {
        return BootstrapResponse::Installed;
    }
    let readiness_status = readiness["status"].as_str().unwrap_or("");
    if matches!(readiness_status, "proof-unavailable" | "host-ineligible") {
        return BootstrapResponse::HostIneligible(
            readiness["reason_code"]
                .as_str()
                .unwrap_or(readiness_status)
                .to_owned(),
        );
    }
    let held = match is_held(journal, "local") {
        Ok(held) => held,
        Err(error) => return BootstrapResponse::Unavailable(error.to_string()),
    };
    let install_state = read_status(journal, "local")
        .map(|status| {
            if cfg!(target_os = "macos") && !metal_candidate::status_targets_native(&status) {
                "idle".to_owned()
            } else {
                status.install_state
            }
        })
        .unwrap_or_else(|_| "idle".to_owned());
    if is_in_flight(&install_state) {
        return BootstrapResponse::InFlight(install_state);
    }
    if held {
        return BootstrapResponse::Busy(install_state);
    }
    BootstrapResponse::SpawnUnavailable
}

#[derive(Debug)]
pub enum EndpointMutationError {
    Mutation(MutationError),
    Confidential(&'static str),
}

pub fn endpoint_payload(config: &Map<String, Value>) -> Value {
    let local = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("local"))
        .and_then(Value::as_object);
    let endpoint_url = local
        .and_then(|local| local.get("endpoint_url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let served_model_id = local
        .and_then(|local| local.get("served_model_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let credential = local
        .and_then(|local| local.get("credential"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    json!({"enabled":!endpoint_url.is_empty() && !served_model_id.is_empty(),"endpoint_url":endpoint_url,"served_model_id":served_model_id,"credential_configured":credential})
}

pub fn update_endpoint(
    journal: &Path,
    endpoint_url: String,
    served_model_id: String,
    credential: Option<Option<String>>,
) -> Result<Value, EndpointMutationError> {
    mutate_endpoint(
        journal,
        "local_endpoint_update",
        credential.is_some(),
        |local| {
            local.insert(
                "endpoint_url".to_owned(),
                Value::String(endpoint_url.clone()),
            );
            local.insert(
                "served_model_id".to_owned(),
                Value::String(served_model_id.clone()),
            );
            if let Some(credential) = &credential {
                match credential
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    Some(value) => {
                        local.insert(
                            "credential".to_owned(),
                            Value::String(value.trim().to_owned()),
                        );
                    }
                    None => {
                        local.remove("credential");
                    }
                }
            }
        },
        "Turn off confidential thinking first, then change your local endpoint.",
    )
}

pub fn clear_endpoint(journal: &Path) -> Result<Value, EndpointMutationError> {
    mutate_endpoint(
        journal,
        "local_endpoint_clear",
        true,
        |local| {
            for key in ["endpoint_url", "served_model_id", "credential"] {
                local.remove(key);
            }
        },
        "Turn off confidential thinking first, then clear your local endpoint.",
    )
}

fn mutate_endpoint(
    journal: &Path,
    action: &str,
    credential_touched: bool,
    change: impl FnOnce(&mut Map<String, Value>),
    confidential_detail: &'static str,
) -> Result<Value, EndpointMutationError> {
    let transaction = mutate_journal_config(journal, Default::default(), |config| {
        if derive_active_brain_lane(config).lane.as_deref() == Some("spp") {
            return JournalConfigMutation { changed: false, value: Err(confidential_detail) };
        }
        let providers = object_at(config, "providers");
        let local = object_at(providers, "local");
        let before = local.clone();
        change(local);
        let mut changes = Map::new();
        for key in ["endpoint_url", "served_model_id"] {
            let old = before.get(key).and_then(Value::as_str).unwrap_or("");
            let new = local.get(key).and_then(Value::as_str).unwrap_or("");
            if old != new { changes.insert(key.to_owned(), json!({"old":old,"new":new})); }
        }
        if credential_touched {
            let old = before.get("credential").and_then(Value::as_str).unwrap_or("");
            let new = local.get("credential").and_then(Value::as_str).unwrap_or("");
            if old != new { changes.insert("credential".to_owned(), json!({"old":if old.is_empty() { "" } else { "***" },"new":if new.is_empty() { "" } else { "***" }})); }
        }
        JournalConfigMutation { changed: !changes.is_empty(), value: Ok((changes, endpoint_payload(config))) }
    }).map_err(|error| EndpointMutationError::Mutation(MutationError::config(error)))?;
    let (changes, payload) = transaction
        .value
        .map_err(EndpointMutationError::Confidential)?;
    if !changes.is_empty() {
        append_action_log(
            journal,
            None,
            "app",
            "thinking",
            action,
            json!({"changed_fields":changes}),
        )
        .map_err(|error| {
            EndpointMutationError::Mutation(MutationError::ActionLog(error.to_string()))
        })?;
    }
    Ok(json!({"success":true,"local_endpoint":payload}))
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

fn bytes_to_gb(bytes: u64) -> f64 {
    ((bytes as f64 / 1024_f64.powi(3)) * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{
        BootstrapResponse, classify_bootstrap, default_model, runtime, runtime_status_for_phase,
        start_bootstrap,
    };

    fn temporary_journal(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "solstone-thinking-local-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("journal directory creates");
        path
    }

    #[test]
    fn bootstrap_refuses_when_a_byo_local_endpoint_is_active() {
        let journal = temporary_journal("byo-endpoint-active");
        let config = serde_json::Map::from_iter([(
            "providers".to_owned(),
            json!({"local":{"endpoint_url":"https://byo.example/v1","served_model_id":"served"}}),
        )]);
        let response = start_bootstrap(&journal, &config, default_model());
        assert_eq!(response, BootstrapResponse::ByoEndpointActive);
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn bootstrap_on_a_fresh_bundled_journal_never_reports_a_false_installed_or_in_flight_state() {
        let journal = temporary_journal("fresh-bundled");
        let config = serde_json::Map::new();
        let response = start_bootstrap(&journal, &config, default_model());
        // A brand-new journal has no install artifacts and no lease/status
        // records, so the only truthful outcomes are "the host can't run
        // this yet" or "an install would need to start" -- never a claim
        // that installation already happened or is already progressing.
        assert!(
            matches!(
                response,
                BootstrapResponse::HostIneligible(_)
                    | BootstrapResponse::SpawnUnavailable
                    | BootstrapResponse::Unavailable(_)
            ),
            "unexpected response for a fresh journal: {response:?}"
        );
        let _ = fs::remove_dir_all(journal);
    }

    #[test]
    fn bootstrap_reports_installed_when_readiness_is_ready() {
        let journal = temporary_journal("already-installed");
        let response = classify_bootstrap(&journal, &json!({"ready": true}));
        assert_eq!(response, BootstrapResponse::Installed);
        let _ = fs::remove_dir_all(journal);
    }

    fn write_runtime_health(journal: &std::path::Path, phase: &str, reason_code: &str) {
        let path = journal.join("health/providers/runtime/local.json");
        fs::create_dir_all(path.parent().expect("runtime directory")).expect("runtime directory");
        fs::write(
            path,
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "provider": "local",
                "revision": 3,
                "phase": phase,
                "reason_code": reason_code,
                "detail": {},
                "desired_fingerprint_sha256": null,
                "incarnation": null,
                "generation": 0,
                "attempt": 0,
                "process": null,
                "updated_at": "2026-08-18T00:00:00Z",
                "display_deadline_at": null,
                "owner": null
            }))
            .expect("health record serializes"),
        )
        .expect("health record writes");
    }

    #[test]
    fn runtime_status_is_not_ok_next_to_a_blocking_phase() {
        assert_eq!(runtime_status_for_phase("host-blocked"), "blocked");
        assert_eq!(runtime_status_for_phase("artifact-not-ready"), "blocked");
        assert_eq!(runtime_status_for_phase("failed"), "failed");
        assert_eq!(runtime_status_for_phase("ready"), "ok");

        let journal = temporary_journal("runtime-blocked");
        write_runtime_health(&journal, "host-blocked", "gpu-unavailable");
        let payload = runtime(&journal);
        assert_eq!(payload["status"], "blocked");
        assert_eq!(payload["phase"], "host-blocked");
        assert_eq!(payload["reason_code"], "gpu-unavailable");

        write_runtime_health(&journal, "artifact-not-ready", "artifact-missing");
        let payload = runtime(&journal);
        assert_eq!(payload["status"], "blocked");
        assert_eq!(payload["phase"], "artifact-not-ready");
        assert_eq!(payload["reason_code"], "artifact-missing");

        write_runtime_health(&journal, "ready", "probe-ready");
        let payload = runtime(&journal);
        assert_eq!(payload["status"], "ok");
        assert_eq!(payload["phase"], "ready");
        let _ = fs::remove_dir_all(journal);
    }
}
