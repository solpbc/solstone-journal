// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Thinking routes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Path as UrlPath, Query};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_handoff_nonce::mint_nonce;
use solstone_core_sol_link::ca::{jid_from_spki, load_ca};
use solstone_core_thinking::confidential::{
    HandoffCode, HandoffResult, OperationHandle, OperationRegistry, Phase, ProvisionError,
    SERVICE_SPP, TokenError, disable_confidential, handoff_result, outcome_from_token,
    provision_confidential_handoff,
};
use solstone_core_thinking::providers::{ManagedKeyValidator, UnavailableValidator};

use crate::{JournalRoot, asset_response, not_found_response};

const DEFAULT_PORTAL_URL: &str = "https://services.solstone.app";
const GENERIC_THINKING_ERROR: &str =
    "something went wrong - try again, and if it persists, check the health dashboard";
const NOT_VERIFIED_GUIDANCE: &str =
    "Hardware attestation is not yet verified. Thinking stays blocked until verification finishes.";
const PYTHON_QUOTE_COMPONENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'\"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Debug, Clone)]
pub enum PollOutcome {
    Continue,
    Failed {
        token: String,
        detail: Option<String>,
    },
    EarlyAccess,
    Success(serde_json::Map<String, Value>),
}

pub trait ConfidentialPoll: Send + Sync {
    fn poll(&self, base_url: &str, nonce: &str) -> PollOutcome;
}

#[derive(Clone)]
pub struct ConfidentialRuntimeOverride {
    pub portal_base_url: String,
    pub poll: Arc<dyn ConfidentialPoll>,
}

#[derive(Clone)]
struct ConfidentialRuntime {
    portal_base_url: String,
    poll: Arc<dyn ConfidentialPoll>,
}

struct PortalPoll;

impl ConfidentialPoll for PortalPoll {
    fn poll(&self, base_url: &str, nonce: &str) -> PollOutcome {
        let url = format!("{base_url}/handoff/{SERVICE_SPP}?nonce={nonce}");
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(35)))
            .timeout_recv_response(Some(Duration::from_secs(35)))
            .timeout_recv_body(Some(Duration::from_secs(35)))
            .timeout_global(Some(Duration::from_secs(35)))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = match agent.get(&url).header("Connection", "close").call() {
            Ok(response) => response,
            Err(error) => return classify_portal_call_error(error),
        };
        match response.status().as_u16() {
            204 => PollOutcome::Continue,
            400 => PollOutcome::Failed {
                token: "nonce_invalid".to_owned(),
                detail: None,
            },
            410 => PollOutcome::Failed {
                token: "consent_link_expired".to_owned(),
                detail: None,
            },
            200 => poll_success_body(response.into_body().read_to_string()),
            _ => PollOutcome::Failed {
                token: "unexpected_payload".to_owned(),
                detail: None,
            },
        }
    }
}

fn classify_portal_call_error(error: ureq::Error) -> PollOutcome {
    match error {
        ureq::Error::Timeout(_) => PollOutcome::Continue,
        error => PollOutcome::Failed {
            token: "portal_unreachable".to_owned(),
            detail: Some(error.to_string()),
        },
    }
}

fn poll_success_body(body: Result<String, ureq::Error>) -> PollOutcome {
    let body = match body {
        Ok(body) => body,
        Err(ureq::Error::Timeout(_)) => return PollOutcome::Continue,
        Err(_) => {
            return PollOutcome::Failed {
                token: "unexpected_payload".to_owned(),
                detail: None,
            };
        }
    };
    match serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value.as_object().cloned())
    {
        Some(payload) if payload.get("state").and_then(Value::as_str) == Some("early_access") => {
            PollOutcome::EarlyAccess
        }
        Some(payload) => PollOutcome::Success(payload),
        None => PollOutcome::Failed {
            token: "unexpected_payload".to_owned(),
            detail: None,
        },
    }
}

pub fn router(journal: Arc<JournalRoot>) -> Router {
    let confidential_runtime = ConfidentialRuntime {
        portal_base_url: std::env::var("SERVICES_PORTAL_URL")
            .unwrap_or_else(|_| DEFAULT_PORTAL_URL.to_owned())
            .trim_end_matches('/')
            .to_owned(),
        poll: Arc::new(PortalPoll),
    };
    Router::new()
        .route("/app/thinking/", get(shell))
        .route("/app/thinking", get(shell_redirect))
        .route("/app/thinking/workspace", get(workspace))
        .route("/app/thinking/static/{*rest}", get(thinking_static))
        .route("/app/thinking/background", get(background_not_found))
        .route("/app/thinking/api/state", get(state))
        .route("/app/thinking/api/providers", get(providers))
        .route(
            "/app/thinking/api/providers",
            post(update_providers).put(update_providers),
        )
        .route("/app/thinking/api/keys", get(keys).put(save_key))
        .route("/app/thinking/api/keys/check", post(check_key))
        .route(
            "/app/thinking/api/providers/local/status",
            get(local_status),
        )
        .route("/app/thinking/api/local/availability", get(availability))
        .route(
            "/app/thinking/api/local/bootstrap/status",
            get(bootstrap_status),
        )
        .route("/app/thinking/api/local/models", get(models))
        .route("/app/thinking/api/local/runtime", get(runtime))
        .route(
            "/app/thinking/api/local/runtime/retry",
            post(retry_local_runtime),
        )
        .route(
            "/app/thinking/api/local/bootstrap",
            post(start_local_bootstrap),
        )
        .route("/app/thinking/api/brain/check", post(check_brain))
        .route(
            "/app/thinking/api/confidential/enable",
            post(confidential_enable),
        )
        .route(
            "/app/thinking/api/confidential/disable",
            post(confidential_disable),
        )
        .route(
            "/app/thinking/api/confidential/recheck",
            post(confidential_recheck),
        )
        .route(
            "/app/thinking/api/local/endpoint",
            get(endpoint_get_not_allowed)
                .post(update_endpoint)
                .delete(clear_endpoint)
                .options(endpoint_options),
        )
        .route(
            "/app/thinking/api/generators",
            get(generators).put(update_generators),
        )
        .route(
            "/app/thinking/api/validate-keys",
            get(validate_keys).post(persist_key_validations),
        )
        .route("/app/thinking/api/validate-model", post(validate_model))
        .route(
            "/app/thinking/api/talents/{day}",
            get(crate::thinking_sol_reads::api_talents_day),
        )
        .route(
            "/app/thinking/api/run/{use_id}",
            get(crate::thinking_sol_reads::api_agent_run),
        )
        .route(
            "/app/thinking/api/output/{day}/{*path}",
            get(crate::thinking_sol_reads::api_output_file),
        )
        .route(
            "/app/thinking/api/preview/{*name}",
            get(crate::thinking_sol_reads::api_preview_prompt),
        )
        .route(
            "/app/thinking/api/index",
            get(crate::thinking_sol_reads::api_index),
        )
        .route(
            "/app/thinking/api/stats/{month}",
            get(crate::thinking_sol_reads::api_stats),
        )
        .route(
            "/app/thinking/api/badge-count",
            get(crate::thinking_sol_reads::api_badge_count),
        )
        .route(
            "/app/thinking/api/updated-days",
            get(crate::thinking_sol_reads::api_updated_days),
        )
        .route(
            "/app/thinking/api/set-owner",
            post(crate::thinking_sol_writes::api_set_owner),
        )
        .route(
            "/app/thinking/api/sol-init",
            post(crate::thinking_sol_writes::api_sol_init),
        )
        .layer(Extension(journal))
        .layer(Extension(Arc::new(
            crate::thinking_sol_reads::TalentRoots::production()
                .unwrap_or_else(|error| panic!("{error}")),
        )))
        .layer(Extension(Arc::new(OperationRegistry::default())))
        .layer(Extension(confidential_runtime))
}

async fn shell() -> Response {
    asset_response("/static/shell.html")
}
async fn shell_redirect() -> Response {
    let location = "/app/thinking/";
    Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(format!(
            "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"{location}\">{location}</a>. If not, click the link.\n"
        )))
        .expect("redirect builds")
}
async fn workspace() -> Response {
    asset_response("/app/thinking/workspace")
}
async fn thinking_static(UrlPath(rest): UrlPath<String>) -> Response {
    if rest.is_empty()
        || rest.contains('\0')
        || Path::new(&rest)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return not_found_response();
    }
    asset_response(&format!("/app/thinking/static/{rest}"))
}
/// `/background` is a fragment route Flask injects per-app only when that
/// app ships a background template (`solstone/apps/__init__.py`'s
/// `_inject_fragment_routes`); Thinking has none, so the reference's own
/// answer is a plain 404, not the 501 `app_response()` gives every other
/// unregistered Thinking path.
async fn background_not_found() -> Response {
    not_found_response()
}

async fn state(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => json_response(
            json!({"providers":solstone_core_thinking::providers::payload(&journal.0,&config,solstone_core_thinking::local::default_model(),operations.operation(SERVICE_SPP)),"keys":solstone_core_thinking::providers::keys(&config),"copy":solstone_core_thinking_copy::thinking_copy_payload()}),
        ),
        Err(response) => *response,
    }
}
async fn providers(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let journal = journal.as_ref();
    let Some(model) =
        solstone_core_thinking::local::accepted_model(query.get("local_model").map(String::as_str))
    else {
        return json_error(solstone_core_thinking::local::invalid_model(
            query
                .get("local_model")
                .map(String::as_str)
                .unwrap_or_default(),
        ));
    };
    match config(&journal.0) {
        Ok(config) => json_response(solstone_core_thinking::providers::payload(
            &journal.0,
            &config,
            model,
            operations.operation(SERVICE_SPP),
        )),
        Err(response) => *response,
    }
}
async fn keys(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => json_response(solstone_core_thinking::providers::keys(&config)),
        Err(response) => *response,
    }
}
async fn local_status(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => json_response(solstone_core_thinking::providers::local_status_only(
            &journal.0,
            &config,
            operations.operation(SERVICE_SPP),
        )),
        Err(response) => *response,
    }
}
async fn availability(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let journal = journal.as_ref();
    model_response(
        &journal.0,
        query.get("model").map(String::as_str),
        |model| solstone_core_thinking::local::availability(&journal.0, model),
    )
}
async fn bootstrap_status(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let journal = journal.as_ref();
    model_response(
        &journal.0,
        query.get("model").map(String::as_str),
        |model| solstone_core_thinking::local::bootstrap_status(&journal.0, model),
    )
}
async fn models() -> Response {
    json_response(solstone_core_thinking::local::models())
}
async fn runtime(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    json_response(solstone_core_thinking::local::runtime(&journal.0))
}

async fn retry_local_runtime(
    Extension(journal): Extension<Arc<JournalRoot>>,
    body: Bytes,
) -> Response {
    let Some(request) = request_object(&body) else {
        return missing_request_body();
    };
    const EXPECTED_FIELDS: [&str; 3] = [
        "health_revision",
        "retry_revision",
        "desired_fingerprint_sha256",
    ];
    let field_set_matches = request.len() == EXPECTED_FIELDS.len()
        && EXPECTED_FIELDS
            .iter()
            .all(|field| request.contains_key(*field));
    let fields = field_set_matches.then(|| {
        (
            request["health_revision"].as_u64().filter(|_| {
                request["health_revision"].is_u64() && !request["health_revision"].is_boolean()
            }),
            request["retry_revision"].as_u64().filter(|_| {
                request["retry_revision"].is_u64() && !request["retry_revision"].is_boolean()
            }),
            request["desired_fingerprint_sha256"]
                .as_str()
                .filter(|value| !value.is_empty()),
        )
    });
    let Some((Some(health_revision), Some(retry_revision), Some(desired_fingerprint_sha256))) =
        fields
    else {
        return invalid_request("runtime retry requires the current recovery state");
    };
    match solstone_core_thinking::local::request_runtime_retry(
        &journal.0,
        health_revision,
        retry_revision,
        desired_fingerprint_sha256,
    ) {
        Ok(value) => json_response(value),
        Err(
            solstone_core_thinking::local::RuntimeRetryError::HealthRevisionConflict
            | solstone_core_thinking::local::RuntimeRetryError::RetryRevisionConflict
            | solstone_core_thinking::local::RuntimeRetryError::DesiredFingerprintConflict
            | solstone_core_thinking::local::RuntimeRetryError::PhaseNotFailed
            | solstone_core_thinking::local::RuntimeRetryError::RetryAlreadyRequested,
        ) => invalid_state("local status changed; check again"),
        Err(
            solstone_core_thinking::local::RuntimeRetryError::Malformed(_)
            | solstone_core_thinking::local::RuntimeRetryError::Unavailable(_),
        ) => thinking_failure_with_detail("local status can't be changed right now; check again"),
        Err(
            solstone_core_thinking::local::RuntimeRetryError::InvalidProvider(_)
            | solstone_core_thinking::local::RuntimeRetryError::Random,
        ) => thinking_failure(),
    }
}

async fn start_local_bootstrap(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let journal = journal.as_ref();
    let Some(model) =
        solstone_core_thinking::local::accepted_model(query.get("model").map(String::as_str))
    else {
        return json_error(solstone_core_thinking::local::invalid_model(
            query.get("model").map(String::as_str).unwrap_or_default(),
        ));
    };
    let config = match config(&journal.0) {
        Ok(config) => config,
        Err(response) => return *response,
    };
    match solstone_core_thinking::local::start_bootstrap(&journal.0, &config, model) {
        solstone_core_thinking::local::BootstrapResponse::Installed => {
            json_response(json!({"install_state":"installed"}))
        }
        solstone_core_thinking::local::BootstrapResponse::InFlight(state) => {
            json_response(json!({"install_state":state}))
        }
        solstone_core_thinking::local::BootstrapResponse::Busy(state) => json_response_with_status(
            StatusCode::CONFLICT,
            json!({"install_state":state,"reason_code":"install_busy"}),
        ),
        solstone_core_thinking::local::BootstrapResponse::ByoEndpointActive => {
            invalid_request("BYO local endpoint is active")
        }
        solstone_core_thinking::local::BootstrapResponse::HostIneligible(reason) => {
            invalid_request(reason)
        }
        // The reference spawns a native installer subprocess here
        // (local_bootstrap.py:319); this wave ships no install-spawn
        // primitive (Fact 8), so a fresh-install-eligible request gets a
        // truthful failure instead of a false in-progress response.
        solstone_core_thinking::local::BootstrapResponse::SpawnUnavailable => {
            thinking_failure_with_detail(
                "local install can't be started from this build yet - use `journal` on this machine, or check back after an update",
            )
        }
        solstone_core_thinking::local::BootstrapResponse::Unavailable(_) => thinking_failure(),
    }
}

async fn check_brain(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    let config = match config(&journal.0) {
        Ok(config) => config,
        Err(response) => return *response,
    };
    let sent = send_brain_refresh_request(&journal.0);
    json_response(solstone_core_thinking::brain::check_response(
        &journal.0, &config, sent,
    ))
}

async fn confidential_enable(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
    Extension(runtime): Extension<ConfidentialRuntime>,
    override_runtime: Option<Extension<ConfidentialRuntimeOverride>>,
) -> Response {
    let journal = journal.as_ref();
    let config = match solstone_core_thinking::read_config(&journal.0) {
        Ok(config) => config,
        Err(_) => return thinking_failure(),
    };
    if confidential_configured(&config) {
        return invalid_state("confidential processing is already set up.");
    }
    let (portal_base_url, poll) = match override_runtime {
        Some(Extension(value)) => (
            value.portal_base_url.trim_end_matches('/').to_owned(),
            value.poll,
        ),
        None => (runtime.portal_base_url, runtime.poll),
    };
    let instance_id = match confidential_instance_id(&journal.0) {
        Some(instance_id) => instance_id,
        None => return thinking_failure(),
    };
    let nonce = match mint_nonce() {
        Ok(nonce) => nonce,
        Err(_) => return thinking_failure(),
    };
    let portal_url = format!(
        "{portal_base_url}/enable/{SERVICE_SPP}?nonce={nonce}&instance={}",
        utf8_percent_encode(&instance_id, PYTHON_QUOTE_COMPONENT),
    );
    let (handle, operation) =
        match operations.start_operation(SERVICE_SPP, "enable", Some(portal_url)) {
            Ok(value) => value,
            Err(_) => return service_busy(),
        };
    spawn_confidential_handoff(
        journal.0.clone(),
        operations,
        handle,
        portal_base_url,
        nonce,
        poll,
    );
    json_response_with_status(
        StatusCode::ACCEPTED,
        json!({"success":true,"service":SERVICE_SPP,"operation":remap_operation(operation)}),
    )
}

async fn confidential_disable(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    match disable_confidential(&journal.0) {
        Ok(outcome) => json_response(json!({
            "success": true,
            "service": SERVICE_SPP,
            "result": {
                "was_enabled": outcome.was_enabled,
                "credential_preserved": outcome.credential_preserved,
            },
        })),
        Err(error) => mutation_error(error),
    }
}

async fn confidential_recheck(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    let config = match config(&journal.0) {
        Ok(config) => config,
        Err(response) => return *response,
    };
    if !solstone_core_thinking::confidential::confidential_enabled(&config) {
        return invalid_state("confidential processing is not active.");
    }
    let sent = send_brain_refresh_request(&journal.0);
    json_response(solstone_core_thinking::brain::check_response(
        &journal.0, &config, sent,
    ))
}

fn confidential_configured(config: &serde_json::Map<String, Value>) -> bool {
    config
        .get("services")
        .and_then(Value::as_object)
        .is_some_and(|services| services.get("confidential").is_some_and(Value::is_object))
}

fn confidential_instance_id(journal: &Path) -> Option<String> {
    let ca_dir = journal.join("link").join("ca");
    let derived = std::fs::read_to_string(ca_dir.join("cert.pem"))
        .ok()
        .zip(std::fs::read_to_string(ca_dir.join("private.pem")).ok())
        .and_then(|(certificate, private_key)| load_ca(&certificate, &private_key).ok())
        .and_then(|ca| jid_from_spki(ca.spki_der()).ok());
    derived.or_else(|| {
        std::fs::read_to_string(journal.join("link").join("state.json"))
            .ok()
            .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
            .and_then(|state| {
                state
                    .get("instance_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
    })
}

fn spawn_confidential_handoff(
    journal: std::path::PathBuf,
    operations: Arc<OperationRegistry>,
    handle: OperationHandle,
    portal_base_url: String,
    nonce: String,
    poll: Arc<dyn ConfidentialPoll>,
) {
    let worker_operations = operations.clone();
    let worker = tokio::spawn(async move {
        if !worker_operations.mark_waiting(SERVICE_SPP, handle) {
            return true;
        }
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
        let result = loop {
            let poll = poll.clone();
            let base_url = portal_base_url.clone();
            let nonce = nonce.clone();
            let poll_result =
                tokio::task::spawn_blocking(move || poll.poll(&base_url, &nonce)).await;
            match poll_result {
                Ok(PollOutcome::Continue) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Ok(PollOutcome::Continue) => break handoff_error("consent_timeout", None),
                Ok(PollOutcome::Failed { token, detail }) => break handoff_error(&token, detail),
                Ok(PollOutcome::EarlyAccess) => {
                    break HandoffResult {
                        phase: Phase::EarlyAccess,
                        guidance: None,
                        retryable: false,
                        subscribe_url: None,
                    };
                }
                Ok(PollOutcome::Success(payload)) => {
                    break match provision_confidential_handoff(&journal, &payload) {
                        Ok(()) => HandoffResult {
                            phase: Phase::Enabled,
                            guidance: Some(NOT_VERIFIED_GUIDANCE.to_owned()),
                            retryable: false,
                            subscribe_url: None,
                        },
                        Err(ProvisionError::Invalid) => handoff_error("unexpected_payload", None),
                        Err(ProvisionError::Mutation(_)) => handoff_error("write_failed", None),
                    };
                }
                Err(_) => return false,
            }
        };
        worker_operations.finish(SERVICE_SPP, handle, result)
    });
    tokio::spawn(async move {
        if !worker.await.unwrap_or(false) {
            let _ = operations.finish(
                SERVICE_SPP,
                handle,
                HandoffResult {
                    phase: Phase::Error,
                    guidance: None,
                    retryable: true,
                    subscribe_url: None,
                },
            );
        }
    });
}

fn handoff_error(token: &str, detail: Option<String>) -> HandoffResult {
    match outcome_from_token(token, detail) {
        Ok((code, _)) => handoff_result(code),
        Err(TokenError::OutOfDomain) => handoff_result(HandoffCode::LocalError),
    }
}

fn remap_operation(mut operation: Value) -> Value {
    let Some(phase) = operation
        .get("phase")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return operation;
    };
    let product = [
        ("starting", "starting"),
        ("waiting", "waiting"),
        ("enabled", "not_verified"),
        ("early_access", "early_access"),
        ("error", "repair_needed"),
    ]
    .into_iter()
    .find_map(|(raw, product)| (phase == raw).then_some(product))
    .unwrap_or(&phase);
    operation["phase"] = Value::String(product.to_owned());
    operation
}

fn service_busy() -> Response {
    envelope(
        "service_busy",
        "The service operation is already running. Try again in a moment.",
        "operation already running",
        StatusCode::SERVICE_UNAVAILABLE,
    )
}

fn brain_refresh_argv() -> Option<Vec<String>> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    brain_refresh_argv_in(&dir)
}

fn brain_refresh_argv_in(dir: &Path) -> Option<Vec<String>> {
    let path =
        solstone_core_journal_cli::sibling_native_in_dir(dir, "solstone-core-journal").ok()?;
    let path = path.to_str()?.to_owned();
    Some(vec![path, "brain".to_owned(), "refresh".to_owned()])
}

fn send_brain_refresh_request(journal_root: &Path) -> bool {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let Some(cmd) = brain_refresh_argv() else {
        return false;
    };
    let mut extra = Map::new();
    extra.insert("cmd".to_owned(), json!(cmd));
    extra.insert(
        "ref".to_owned(),
        json!(format!(
            "brain-refresh:thinking:{}-{counter}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )),
    );
    let envelope = CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "request".to_owned(),
        ts: None,
        extra,
    };
    let Ok(mut line) = serde_json::to_string(&envelope) else {
        return false;
    };
    line.push('\n');
    let sender = CallosumOneShotSender::new(
        journal_root.join("health/callosum.sock"),
        Duration::from_secs(1),
    );
    sender.send_line(&line).is_ok()
}

async fn generators(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        Ok(config) => match solstone_core_thinking::generators::generators(&config) {
            Ok(value) => json_response(value),
            Err(detail) => server_error(detail),
        },
        Err(response) => *response,
    }
}
async fn validate_keys(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let journal = journal.as_ref();
    match config(&journal.0) {
        // This GET deliberately does not probe providers, so configured keys
        // report `validation_unavailable`. The reference GET probes live
        // providers; that is a knowing divergence, not an unfinished stub.
        // POST is the deliberate real-probe path.
        Ok(config) => json_response(solstone_core_thinking::providers::validate_keys_with(
            &config,
            &UnavailableValidator,
        )),
        Err(response) => *response,
    }
}

async fn save_key(Extension(journal): Extension<Arc<JournalRoot>>, body: Bytes) -> Response {
    let Some(request) = request_object(&body) else {
        return missing_request_body();
    };
    let env_var = request
        .get("env_var")
        .or_else(|| request.get("key"))
        .and_then(Value::as_str);
    let Some(env_var) = env_var.filter(|value| key_provider(value).is_some()) else {
        return invalid_config(format!(
            "Invalid env var: {}. Must be one of: GOOGLE_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY",
            request
                .get("env_var")
                .or_else(|| request.get("key"))
                .map(json_display)
                .unwrap_or_else(|| "None".to_owned())
        ));
    };
    let value = match request.get("value") {
        None => "",
        Some(Value::String(value)) => value.as_str(),
        Some(_) => return invalid_request("value must be a string"),
    };
    let value = value.trim();
    let provider = key_provider(env_var).expect("checked provider");
    let validator = match one_shot_validator() {
        Ok(validator) => validator,
        Err(response) => return *response,
    };
    let validation = (!value.is_empty()).then(|| {
        let result = validator.validate(provider, value).unwrap_or_else(
            |error| json!({"valid":false,"reason_code":"validation_unavailable","error":error}),
        );
        let mut result = result.as_object().cloned().unwrap_or_default();
        result.insert(
            "timestamp".to_owned(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
        Value::Object(result)
    });
    match solstone_core_thinking::providers::save_key(
        &journal.0, env_var, provider, value, validation,
    ) {
        Ok(value) => json_response(value),
        Err(error) => mutation_error(error),
    }
}

async fn persist_key_validations(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let validator = match one_shot_validator() {
        Ok(validator) => validator,
        Err(response) => return *response,
    };
    match solstone_core_thinking::providers::persist_key_validations(&journal.0, &validator) {
        Ok(value) => json_response(value),
        Err(error) => mutation_error(error),
    }
}

async fn check_key(body: Bytes) -> Response {
    let Some(request) = request_object(&body) else {
        return missing_request_body();
    };
    let Some(env_var) = request
        .get("env_var")
        .and_then(Value::as_str)
        .filter(|value| key_provider(value).is_some())
    else {
        return invalid_config(format!(
            "Invalid env var: {}. Must be one of: GOOGLE_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY",
            request
                .get("env_var")
                .map(json_display)
                .unwrap_or_else(|| "None".to_owned())
        ));
    };
    let Some(value) = request.get("value").unwrap_or(&Value::Null).as_str() else {
        return invalid_request("value must be a string");
    };
    let value = value.trim();
    if value.is_empty() {
        return invalid_request("value must not be empty");
    }
    let validator = match one_shot_validator() {
        Ok(value) => value,
        Err(response) => return *response,
    };
    browser_validation(
        validator
            .validate(key_provider(env_var).expect("checked"), value)
            .unwrap_or_else(
                |error| json!({"valid":false,"reason_code":"validation_unavailable","error":error}),
            ),
        json!({"provider":key_provider(env_var).expect("checked")}),
    )
}

async fn validate_model(Extension(journal): Extension<Arc<JournalRoot>>, body: Bytes) -> Response {
    let Some(request) = request_object(&body).filter(|value| !value.is_empty()) else {
        return missing_request_body();
    };
    let Some(provider) = request
        .get("provider")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "anthropic" | "google" | "openai"))
    else {
        return invalid_request(format!(
            "Invalid provider: {}. Must be one of: anthropic, google, openai",
            request
                .get("provider")
                .map(json_display)
                .unwrap_or_else(|| "None".to_owned())
        ));
    };
    let Some(model) = request
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return invalid_request("model must be a non-empty string.");
    };
    let config = match solstone_core_thinking::read_config(&journal.0) {
        Ok(value) => value,
        Err(_error) => return thinking_failure(),
    };
    let env_var = match provider {
        "google" => "GOOGLE_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        _ => unreachable!(),
    };
    let key = config
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get(env_var))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let identity = json!({"provider":provider,"model":model});
    let Some(key) = key else {
        return browser_validation(
            json!({"valid":false,"reason_code":"key_missing","error":"No stored API key for provider."}),
            identity,
        );
    };
    let validator = match one_shot_validator() {
        Ok(value) => value,
        Err(response) => return *response,
    };
    browser_validation(
        validator
            .validate_model(provider, model, key)
            .unwrap_or_else(
                |error| json!({"valid":false,"reason_code":"validation_unavailable","error":error}),
            ),
        identity,
    )
}

async fn update_providers(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
    body: Bytes,
) -> Response {
    let Some(request) = request_object(&body).filter(|request| !request.is_empty()) else {
        return missing_request_body();
    };
    let unknown: Vec<_> = request
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "lane" | "provider" | "model" | "google_model_resolution_targets"
            )
        })
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return invalid_config(format!("Unknown provider fields: {}", unknown.join(", ")));
    }
    let Some(lane_value) = request.get("lane") else {
        return missing_field("lane");
    };
    let Some(lane) = lane_value.as_str() else {
        return invalid_config(format!(
            "Invalid lane: {}. Must be one of: byo, confidential, local",
            json_display(lane_value)
        ));
    };
    let update = match solstone_core_thinking::providers::resolve_provider_update(
        &journal.0, lane, &request,
    ) {
        Ok(update) => update,
        Err(solstone_core_thinking::providers::ProviderRequestError::InvalidInput(detail)) => {
            return invalid_config(detail);
        }
        Err(solstone_core_thinking::providers::ProviderRequestError::InvalidState(detail)) => {
            return invalid_state(detail);
        }
        Err(solstone_core_thinking::providers::ProviderRequestError::ConfigUnreadable(_)) => {
            return thinking_failure();
        }
    };
    match solstone_core_thinking::providers::update_providers(
        &journal.0,
        update,
        operations.operation(SERVICE_SPP),
    ) {
        Ok(value) => json_response(value),
        Err(solstone_core_thinking::providers::ProviderUpdateError::Mutation(error)) => {
            mutation_error(error)
        }
        Err(solstone_core_thinking::providers::ProviderUpdateError::Confidential(detail)) => {
            invalid_state(detail)
        }
    }
}

async fn update_endpoint(Extension(journal): Extension<Arc<JournalRoot>>, body: Bytes) -> Response {
    let Some(request) = request_object(&body) else {
        return missing_request_body();
    };
    let Some(endpoint_url) = request
        .get("endpoint_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return missing_field("endpoint_url");
    };
    let Some(normalized) = normalize_endpoint(endpoint_url) else {
        return invalid_config("endpoint_url must be an http or https URL with a host");
    };
    let Some(served_model_id) = request
        .get("served_model_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return missing_field("served_model_id");
    };
    let credential = match request.get("credential") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(_) => return invalid_request("credential"),
    };
    endpoint_result(solstone_core_thinking::local::update_endpoint(
        &journal.0,
        normalized,
        served_model_id.to_owned(),
        credential,
    ))
}

async fn clear_endpoint(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    endpoint_result(solstone_core_thinking::local::clear_endpoint(&journal.0))
}

fn endpoint_result(
    result: Result<Value, solstone_core_thinking::local::EndpointMutationError>,
) -> Response {
    match result {
        Ok(value) => json_response(value),
        Err(solstone_core_thinking::local::EndpointMutationError::Mutation(error)) => {
            mutation_error(error)
        }
        Err(solstone_core_thinking::local::EndpointMutationError::Confidential(detail)) => {
            invalid_state(detail)
        }
    }
}

async fn endpoint_options() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

async fn endpoint_get_not_allowed() -> Response {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(header::ALLOW, "POST, DELETE, OPTIONS")
        .body(Body::empty())
        .expect("method refusal builds")
}

async fn update_generators(
    Extension(journal): Extension<Arc<JournalRoot>>,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return missing_request_body();
    }
    let Some(request) = request_object(&body).filter(|request| !request.is_empty()) else {
        return missing_request_body();
    };
    for (key, update) in &request {
        let Some(update) = update.as_object() else {
            continue;
        };
        for field in ["disabled", "extract"] {
            if update.contains_key(field) && !update[field].is_boolean() {
                return invalid_config(format!("{field} must be boolean for {key}"));
            }
        }
    }
    match solstone_core_thinking::generators::update_overrides(&journal.0, &request) {
        Ok(()) => match solstone_core_thinking::read_config(&journal.0) {
            Ok(config) => match solstone_core_thinking::generators::generators(&config) {
                Ok(value) => json_response(value),
                Err(_error) => thinking_failure(),
            },
            Err(_error) => thinking_failure(),
        },
        Err(error) => mutation_error(error),
    }
}

fn model_response(
    _journal: &Path,
    requested: Option<&str>,
    render: impl FnOnce(&str) -> Value,
) -> Response {
    match solstone_core_thinking::local::accepted_model(requested) {
        Some(model) => json_response(render(model)),
        None => json_error(solstone_core_thinking::local::invalid_model(
            requested.unwrap_or(solstone_core_thinking::local::default_model()),
        )),
    }
}
fn config(journal: &Path) -> Result<serde_json::Map<String, Value>, Box<Response>> {
    solstone_core_thinking::read_config(journal)
        .map_err(|error| Box::new(server_error(error.to_string())))
}
fn json_error(value: Value) -> Response {
    json_response_with_status(StatusCode::BAD_REQUEST, value)
}
fn json_response(value: Value) -> Response {
    json_response_with_status(StatusCode::OK, value)
}
fn json_response_with_status(status: StatusCode, value: Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!("{}\n", flask_json(&value))))
        .expect("JSON response builds")
}
fn flask_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => flask_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(flask_json).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => {
            let mut fields: Vec<_> = values.iter().collect();
            fields.sort_unstable_by(|left, right| left.0.cmp(right.0));
            format!(
                "{{{}}}",
                fields
                    .into_iter()
                    .map(|(key, value)| format!("{}:{}", flask_string(key), flask_json(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}
fn flask_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' || !character.is_ascii() => {
                let code = character as u32;
                if code <= 0xffff {
                    output.push_str(&format!("\\u{code:04x}"));
                } else {
                    let code = code - 0x1_0000;
                    output.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (code >> 10),
                        0xdc00 + (code & 0x3ff)
                    ));
                }
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}
fn server_error(detail: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        detail,
    )
        .into_response()
}

fn request_object(body: &Bytes) -> Option<Map<String, Value>> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .as_object()
        .cloned()
}

fn key_provider(env_var: &str) -> Option<&'static str> {
    match env_var {
        "GOOGLE_API_KEY" => Some("google"),
        "ANTHROPIC_API_KEY" => Some("anthropic"),
        "OPENAI_API_KEY" => Some("openai"),
        _ => None,
    }
}

fn one_shot_validator()
-> Result<solstone_core_thinking::providers::OneShotKeyValidator, Box<Response>> {
    solstone_core_thinking::providers::OneShotKeyValidator::sibling()
        .map_err(|_error| Box::new(thinking_failure()))
}

fn browser_validation(result: Value, identity: Value) -> Response {
    let valid = result.get("valid").and_then(Value::as_bool) == Some(true);
    let mut response = serde_json::Map::from_iter([(String::from("valid"), Value::Bool(valid))]);
    if let Some(identity) = identity.as_object() {
        response.extend(identity.clone());
    }
    if !valid {
        response.insert(
            "reason_code".to_owned(),
            result.get("reason_code").cloned().unwrap_or(Value::Null),
        );
        response.insert(
            "message".to_owned(),
            result
                .get("error")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())),
        );
    } else if let Some(reason) = result.get("probe_reason_code") {
        response.insert("probe_reason_code".to_owned(), reason.clone());
    }
    json_response(Value::Object(response))
}

fn json_display(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "None".to_owned(),
        value => value.to_string(),
    }
}

fn normalize_endpoint(value: &str) -> Option<String> {
    let value = value.trim();
    let valid = ["http://", "https://"].iter().any(|prefix| {
        value
            .strip_prefix(prefix)
            .is_some_and(|rest| !rest.split('/').next().unwrap_or("").is_empty())
    });
    if !valid {
        return None;
    }
    let value = value.trim_end_matches('/');
    Some(
        value
            .strip_suffix("/v1")
            .unwrap_or(value)
            .trim_end_matches('/')
            .to_owned(),
    )
}

fn envelope(
    reason: &str,
    message: &str,
    detail: impl Into<String>,
    status: StatusCode,
) -> Response {
    error_envelope(reason, message, detail, status).into_response()
}

fn missing_request_body() -> Response {
    envelope(
        "missing_request_body",
        "that request had no data in it.",
        "No data provided",
        StatusCode::BAD_REQUEST,
    )
}
fn missing_field(detail: impl Into<String>) -> Response {
    envelope(
        "missing_required_field",
        "a required field is missing.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn invalid_config(detail: impl Into<String>) -> Response {
    envelope(
        "invalid_config_value",
        "that setting couldn't be saved because one value was invalid.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn invalid_request(detail: impl Into<String>) -> Response {
    envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn invalid_state(detail: impl Into<String>) -> Response {
    envelope(
        "invalid_operation_for_state",
        "that action couldn't be taken in the current state.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn thinking_failure() -> Response {
    thinking_failure_with_detail(GENERIC_THINKING_ERROR)
}
fn thinking_failure_with_detail(detail: impl Into<String>) -> Response {
    envelope(
        "settings_operation_failed",
        "those settings couldn't be saved.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}
fn mutation_error(error: solstone_core_thinking::MutationError) -> Response {
    match error {
        solstone_core_thinking::MutationError::ConfigLock(_) => thinking_config_busy_response(),
        solstone_core_thinking::MutationError::ConfigLoad(_error) => thinking_failure(),
        solstone_core_thinking::MutationError::ConfigWrite(_error) => thinking_failure(),
        solstone_core_thinking::MutationError::Read(_error) => thinking_failure(),
        solstone_core_thinking::MutationError::ActionLog(_error) => thinking_failure(),
    }
}

#[allow(dead_code)] // Wired by Thinking write routes in Thinking write-route chunks.
fn thinking_config_busy_response() -> Response {
    error_envelope(
        "config_busy",
        "those settings couldn't be saved right now because they were busy. try again in a moment.",
        "settings are busy; try again",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use chrono::{Duration, Utc};
    use serde_json::{Value, json};
    use solstone_core_brain::{begin_refresh, finish_refresh};
    use tower::ServiceExt;

    use super::{
        PollOutcome, brain_refresh_argv_in, classify_portal_call_error, poll_success_body,
    };

    #[test]
    fn portal_body_timeout_keeps_polling() {
        assert!(matches!(
            poll_success_body(Err(ureq::Error::Timeout(ureq::Timeout::RecvBody))),
            PollOutcome::Continue
        ));
        assert!(matches!(
            poll_success_body(Err(ureq::Error::HostNotFound)),
            PollOutcome::Failed {
                token,
                ..
            } if token == "unexpected_payload"
        ));
        assert!(matches!(
            classify_portal_call_error(ureq::Error::Timeout(ureq::Timeout::Connect)),
            PollOutcome::Continue
        ));
        assert!(matches!(
            classify_portal_call_error(ureq::Error::Timeout(ureq::Timeout::RecvResponse)),
            PollOutcome::Continue
        ));
    }

    #[test]
    fn thinking_embedded_assets_are_generated_from_crate_copies() {
        let generated = include_str!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
        for path in [
            "/app/thinking/workspace",
            "/app/thinking/static/thinking.js",
        ] {
            let entry = generated
                .lines()
                .find(|line| line.contains(&format!("path: \"{path}\"")))
                .expect("thinking asset is embedded");
            assert!(entry.contains(env!("CARGO_MANIFEST_DIR")));
            assert!(entry.contains("assets/thinking/"));
            assert!(!entry.contains("solstone/apps/"));
        }
    }

    #[tokio::test]
    async fn write_routes_register_the_reference_methods() {
        let root = temporary_journal("write-methods");
        let router = crate::router(root.clone());
        let response = router
            .clone()
            .oneshot(
                Request::post("/app/thinking/api/providers")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = router
            .oneshot(
                Request::get("/app/thinking/api/local/endpoint")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()["allow"], "POST, DELETE, OPTIONS");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn copy_payload_round_trips_from_api_state() {
        let root = temporary_journal("copy");
        let response = crate::router(root.clone())
            .oneshot(
                Request::get("/app/thinking/api/state")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body reads"),
        )
        .expect("state is JSON");
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../fixtures/convey_thinking_corpus.json"
        ))
        .expect("corpus parses");
        let expected = corpus["phases"]["none"]
            .as_array()
            .expect("none cases")
            .iter()
            .find(|case| case["path"] == "/app/thinking/api/state")
            .expect("state case");
        // Dollar estimates were retired after this capture; all other copy stays pinned.
        let mut expected_copy = expected["json"]["copy"].clone();
        expected_copy["byo_setup"]
            .as_object_mut()
            .unwrap()
            .remove("custom_cost_note");
        expected_copy["byo_setup"]["tier_blurb_top"] =
            json!("the most capable, for the heaviest thinking.");
        expected_copy["byo_setup"]["tier_blurb_lite"] =
            json!("light and quick. tuned for small models, so this one does the job well.");
        assert_eq!(body["copy"], expected_copy);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn invalid_brain_record_degrades_the_brain_read_projections() {
        let root = temporary_journal("invalid-brain");
        fs::write(root.join("config/journal.json"), br#"{"env":{"OPENAI_API_KEY":"key"},"providers":{"active":{"model":"gpt-5","provider":"openai"}},"setup":{"completed_at":1767225600}}"#).expect("config writes");
        let now = Utc::now();
        let evidence = json!({"status":"ok","observed_at":now.to_rfc3339(),"expires_at":(now + Duration::days(1)).to_rfc3339()});
        let permit = begin_refresh(&root, now, None, None, false, None)
            .expect("refresh starts")
            .expect("permit");
        finish_refresh(&root, permit, json!({"configuration":evidence,"lane_prerequisites":evidence,"generate":evidence,"cogitate":evidence}), now, None).expect("refresh finishes");
        let brain = root.join("health/brain.json");
        let mut invalid: Value =
            serde_json::from_slice(&fs::read(&brain).expect("record reads")).expect("record JSON");
        invalid["fingerprint_sha256"] = Value::String("x".repeat(64));
        fs::write(
            &brain,
            serde_json::to_vec(&invalid).expect("record serializes"),
        )
        .expect("record writes");
        for path in ["/app/thinking/api/state", "/app/thinking/api/providers"] {
            let response = crate::router(root.clone())
                .oneshot(
                    Request::get(path)
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("router responds");
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body reads"),
            )
            .expect("projection is JSON");
            let brain = if path.ends_with("state") {
                &body["providers"]["brain"]
            } else {
                &body["brain"]
            };
            assert_eq!(brain["reason_code"], "brain_record_invalid");
        }
        let response = crate::router(root.clone())
            .oneshot(
                Request::get("/app/thinking/api/providers/local/status")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body reads"),
        )
        .expect("local status is JSON");
        assert_eq!(body["generate_ready"], false);
        assert_eq!(body["cogitate_ready"], false);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn brain_refresh_argv_resolves_the_sibling_journal_binary() {
        let root = tempfile::tempdir().expect("dir");
        let binary = root.path().join("solstone-core-journal");
        fs::write(&binary, "#!/bin/sh\nexit 0\n").expect("write sibling");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod");
        let argv = brain_refresh_argv_in(root.path()).expect("resolved");
        assert_eq!(
            argv,
            vec![
                binary.to_str().expect("utf-8").to_owned(),
                "brain".to_owned(),
                "refresh".to_owned(),
            ]
        );
    }

    #[test]
    fn brain_refresh_argv_is_none_when_the_sibling_is_missing() {
        let root = tempfile::tempdir().expect("dir");
        assert_eq!(brain_refresh_argv_in(root.path()), None);
    }

    fn temporary_journal(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("solstone-thinking-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("config")).expect("config directory creates");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1767225600}}"#,
        )
        .expect("config writes");
        root
    }
}
