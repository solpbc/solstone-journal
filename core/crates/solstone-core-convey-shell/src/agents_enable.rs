// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native solstone.me consent handoff and enable write routes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_handoff_nonce::mint_nonce;
use solstone_core_journal_config::{
    McpEndpointCapability, mcp_endpoint_capability, read_journal_config,
};
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_sol_link::service_identity::load_or_create_service_identity;
use solstone_core_thinking::confidential::{
    HandoffResult, OperationHandle, OperationRegistry, Phase,
};

use crate::JournalRoot;
use crate::assets;

pub const SERVICE: &str = "sme";
const DEFAULT_PORTAL_URL: &str = "https://services.solstone.app";
const BUSY_ERROR: &str = "The service operation is already running. Try again in a moment.";
const BUSY_DETAIL: &str = "operation already running";

#[derive(Clone, Debug)]
pub enum SmePollOutcome {
    Continue,
    Failed {
        token: String,
        detail: Option<String>,
    },
    Success(Map<String, Value>),
}

pub trait SmePoll: Send + Sync {
    fn poll(&self, base_url: &str, nonce: &str) -> SmePollOutcome;
}

#[derive(Clone)]
pub struct SmeRuntimeOverride {
    pub portal_base_url: String,
    pub poll: Arc<dyn SmePoll>,
}

#[derive(Clone)]
pub struct SmeOperationsOverride(pub Arc<OperationRegistry>);

#[derive(Clone)]
struct SmeRuntime {
    portal_base_url: String,
    poll: Arc<dyn SmePoll>,
}

struct PortalPoll;

impl SmePoll for PortalPoll {
    fn poll(&self, base_url: &str, nonce: &str) -> SmePollOutcome {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(35)))
            .timeout_recv_response(Some(Duration::from_secs(35)))
            .timeout_recv_body(Some(Duration::from_secs(35)))
            .timeout_global(Some(Duration::from_secs(35)))
            .build()
            .new_agent();
        let response = match agent
            .get(&format!("{base_url}/handoff/solstone-me?nonce={nonce}"))
            .header("Connection", "close")
            .call()
        {
            Ok(value) => value,
            Err(ureq::Error::Timeout(_)) => return SmePollOutcome::Continue,
            Err(error) => {
                return SmePollOutcome::Failed {
                    token: "portal_unreachable".to_owned(),
                    detail: Some(error.to_string()),
                };
            }
        };
        match response.status().as_u16() {
            204 => SmePollOutcome::Continue,
            400 => SmePollOutcome::Failed {
                token: "nonce_invalid".to_owned(),
                detail: None,
            },
            410 => SmePollOutcome::Failed {
                token: "consent_link_expired".to_owned(),
                detail: None,
            },
            200 => match response
                .into_body()
                .read_to_string()
                .ok()
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .and_then(|value| value.as_object().cloned())
            {
                Some(value) => SmePollOutcome::Success(value),
                None => SmePollOutcome::Failed {
                    token: "unexpected_payload".to_owned(),
                    detail: None,
                },
            },
            _ => SmePollOutcome::Failed {
                token: "unexpected_payload".to_owned(),
                detail: None,
            },
        }
    }
}

pub fn router(prefix: &str, operations: Arc<OperationRegistry>) -> axum::Router {
    let runtime = SmeRuntime {
        portal_base_url: std::env::var("SERVICES_PORTAL_URL")
            .unwrap_or_else(|_| DEFAULT_PORTAL_URL.to_owned())
            .trim_end_matches('/')
            .to_owned(),
        poll: Arc::new(PortalPoll),
    };
    axum::Router::new()
        .route(
            &format!("{prefix}/api/enable"),
            axum::routing::post(agents_enable).get(agents_operation),
        )
        .layer(Extension(operations))
        .layer(Extension(runtime))
}

async fn agents_operation(
    Extension(operations): Extension<Arc<OperationRegistry>>,
    override_operations: Option<Extension<SmeOperationsOverride>>,
) -> Response {
    let operations = override_operations
        .map(|Extension(value)| value.0)
        .unwrap_or(operations);
    Json(json!({
        "service": SERVICE,
        "operation": operations.operation_raw(SERVICE)
    }))
    .into_response()
}

async fn agents_enable(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
    Extension(runtime): Extension<SmeRuntime>,
    override_operations: Option<Extension<SmeOperationsOverride>>,
    override_runtime: Option<Extension<SmeRuntimeOverride>>,
) -> Response {
    let operations = override_operations
        .map(|Extension(value)| value.0)
        .unwrap_or(operations);
    let config = match read_journal_config(&journal.0) {
        Ok(c) => c,
        Err(_) => {
            return refusal(
                "service_operation_failed",
                &copy("SME_CONSENT_LINK_PREPARE_FAILED_DETAIL"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    if matches!(
        mcp_endpoint_capability(&config),
        Ok(McpEndpointCapability::Enabled)
    ) {
        return refusal(
            "invalid_operation_for_state",
            &copy("SME_ALREADY_ENABLED_DETAIL"),
            StatusCode::BAD_REQUEST,
        );
    }
    let runtime = override_runtime
        .map(|Extension(value)| SmeRuntime {
            portal_base_url: value.portal_base_url.trim_end_matches('/').to_owned(),
            poll: value.poll,
        })
        .unwrap_or(runtime);
    let identity = match load_or_create_service_identity(&journal.0, "solstone") {
        Ok(value) => value,
        Err(_) => {
            return refusal(
                "service_operation_failed",
                &copy("SME_CONSENT_LINK_PREPARE_FAILED_DETAIL"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let nonce = match mint_nonce() {
        Ok(value) => value,
        Err(_) => {
            return refusal(
                "service_operation_failed",
                &copy("SME_CONSENT_LINK_PREPARE_FAILED_DETAIL"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let portal_url = format!(
        "{}/enable/solstone-me?nonce={nonce}&instance={}",
        runtime.portal_base_url, identity.instance_id
    );
    let (handle, operation) =
        match operations.start_operation(SERVICE, "sme_enable", Some(portal_url)) {
            Ok(value) => value,
            Err(_) => return busy_refusal(),
        };
    spawn_handoff(journal.0.clone(), operations, handle, runtime, nonce);
    (
        StatusCode::ACCEPTED,
        Json(json!({"success":true,"service":SERVICE,"operation":operation})),
    )
        .into_response()
}

fn busy_refusal() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "reason_code": "service_busy",
            "reason": "service_busy",
            "error": BUSY_ERROR,
            "detail": BUSY_DETAIL,
        })),
    )
        .into_response()
}

fn refusal(code: &'static str, detail: &str, status: StatusCode) -> Response {
    (
        status,
        Json(json!({
            "reason_code": code,
            "reason": code,
            "error": detail,
            "detail": detail,
        })),
    )
        .into_response()
}

fn spawn_handoff(
    journal: std::path::PathBuf,
    operations: Arc<OperationRegistry>,
    handle: OperationHandle,
    runtime: SmeRuntime,
    nonce: String,
) {
    tokio::spawn(async move {
        if !operations.mark_waiting(SERVICE, handle) {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(900);
        let result = loop {
            let poll = runtime.poll.clone();
            let base = runtime.portal_base_url.clone();
            let nonce = nonce.clone();
            match tokio::task::spawn_blocking(move || poll.poll(&base, &nonce)).await {
                Ok(SmePollOutcome::Continue) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Ok(SmePollOutcome::Continue) => break outcome(Phase::Error, "expired", None),
                Ok(SmePollOutcome::Failed { token, detail }) => {
                    break outcome_for_token(&token, detail);
                }
                Ok(SmePollOutcome::Success(payload)) => match classify_payload(&payload) {
                    Ok(("pending", _)) if Instant::now() < deadline => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    Ok(("revoked", _)) => break outcome(Phase::Revoked, "revoked", None),
                    Ok(("needs_subscription", url)) => {
                        break outcome(Phase::NeedsSubscription, "needs_subscription", url);
                    }
                    Ok(("approved", _)) => {
                        let result = enable_sme(&journal);
                        break match result {
                            Ok(()) => outcome(Phase::Enabled, "approved", None),
                            Err(_) => outcome(Phase::Error, "local_error", None),
                        };
                    }
                    _ => break outcome(Phase::Error, "malformed", None),
                },
                Err(_) => break outcome(Phase::Error, "local_error", None),
            }
        };
        operations.finish(SERVICE, handle, result);
    });
}

fn enable_sme(journal: &std::path::Path) -> Result<(), String> {
    mutate_journal_config(journal, Default::default(), |config| {
        let endpoint = config
            .entry("mcp_endpoint".to_owned())
            .or_insert_with(|| json!({}));
        let Some(endpoint) = endpoint.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err("mcp_endpoint setting is not an object".to_owned()),
            };
        };
        let changed = endpoint.get("enabled") != Some(&Value::Bool(true));
        endpoint.insert("enabled".to_owned(), Value::Bool(true));
        JournalConfigMutation {
            changed,
            value: Ok(()),
        }
    })
    .map_err(|e| e.to_string())
    .and_then(|mutation| mutation.value)
}

fn classify_payload(payload: &Map<String, Value>) -> Result<(&str, Option<String>), ()> {
    if payload.get("service").and_then(Value::as_str) != Some(SERVICE) {
        return Err(());
    }
    let state = payload.get("state").and_then(Value::as_str).ok_or(())?;
    match state {
        "approved"
            if payload.len() == 3
                && payload
                    .get("approved_at")
                    .is_some_and(|value| value.is_string() || value.is_number()) =>
        {
            Ok((state, None))
        }
        "pending" | "revoked" if payload.len() == 2 => Ok((state, None)),
        "needs_subscription" if payload.len() == 3 => payload
            .get("subscribe_url")
            .and_then(Value::as_str)
            .filter(|url| url.starts_with("https://"))
            .map(|url| (state, Some(url.to_owned())))
            .ok_or(()),
        _ => Err(()),
    }
}

fn outcome_for_token(token: &str, _detail: Option<String>) -> HandoffResult {
    let code = match token {
        "consent_link_expired" | "consent_timeout" => "expired",
        "portal_unreachable" | "tls_verification_failed" => "network_error",
        _ => "malformed",
    };
    outcome(Phase::Error, code, None)
}

fn outcome(phase: Phase, code: &str, subscribe_url: Option<String>) -> HandoffResult {
    HandoffResult {
        phase,
        guidance: guidance(code),
        retryable: matches!(code, "expired" | "network_error" | "local_error"),
        subscribe_url,
    }
}

fn guidance(code: &str) -> Option<String> {
    serde_json::from_str::<Value>(assets::spl_outcome_strings_json())
        .ok()?
        .get("SME_OUTCOME_GUIDANCE")?
        .get(code)?
        .as_str()
        .map(str::to_owned)
}

fn copy(name: &str) -> String {
    serde_json::from_str::<Value>(assets::spl_outcome_strings_json())
        .ok()
        .and_then(|value| value.get(name).and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
}
