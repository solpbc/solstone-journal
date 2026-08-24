// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native private-link and home-address write routes.
//!
//! These three POST routes have no local-owner check. That is intentional:
//! pairing is itself an owner act, so a paired device may rewrite the home
//! address and enable or disable the private link. `pair-start` and
//! `nonce-status` in `network.rs` do require a local owner, because those mint
//! and inspect enrollment windows. Do not add a local-owner gate here.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_handoff_nonce::mint_nonce;
use solstone_core_journal_config::read_direct_door_port;
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_sol_link::pairing::addresses::is_usable_ipv4;
use solstone_core_sol_link::service_identity::{ServiceIdentity, load_or_create_service_identity};
use solstone_core_spl::{EnrollError, disable_spl, enable_spl_with, enroll_home};
use solstone_core_thinking::confidential::{
    HandoffResult, OperationHandle, OperationRegistry, Phase,
};

use crate::JournalRoot;
use crate::assets;
use crate::network::refusal;
use crate::network_status::private_link_body;
use crate::pair_window_manager::{PairWindowManager, unix_seconds};

const SERVICE: &str = "spl";
const DEFAULT_PORTAL_URL: &str = "https://services.solstone.app";
const BUSY_ERROR: &str = "The service operation is already running. Try again in a moment.";
const BUSY_DETAIL: &str = "operation already running";

#[derive(Clone, Debug)]
pub enum SplPollOutcome {
    Continue,
    Failed {
        token: String,
        detail: Option<String>,
    },
    Success(Map<String, Value>),
}

pub trait SplPoll: Send + Sync {
    fn poll(&self, base_url: &str, nonce: &str) -> SplPollOutcome;
}
pub trait SplEnrollment: Send + Sync {
    fn enroll(
        &self,
        journal: &std::path::Path,
        identity: &ServiceIdentity,
        ca_pubkey: &str,
    ) -> Result<String, EnrollError>;
}

#[derive(Clone)]
pub struct SplRuntimeOverride {
    pub portal_base_url: String,
    pub poll: Arc<dyn SplPoll>,
    pub enrollment: Arc<dyn SplEnrollment>,
}
#[derive(Clone)]
pub struct NetworkOperationsOverride(pub Arc<OperationRegistry>);
#[derive(Clone)]
pub struct SplDisableFailureOverride;
#[derive(Clone)]
struct SplRuntime {
    portal_base_url: String,
    poll: Arc<dyn SplPoll>,
    enrollment: Arc<dyn SplEnrollment>,
}
struct PortalPoll;
struct RelayEnrollment;

impl SplPoll for PortalPoll {
    fn poll(&self, base_url: &str, nonce: &str) -> SplPollOutcome {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(35)))
            .timeout_recv_response(Some(Duration::from_secs(35)))
            .timeout_recv_body(Some(Duration::from_secs(35)))
            .timeout_global(Some(Duration::from_secs(35)))
            .build()
            .new_agent();
        let response = match agent
            .get(&format!("{base_url}/handoff/spl?nonce={nonce}"))
            .header("Connection", "close")
            .call()
        {
            Ok(value) => value,
            Err(ureq::Error::Timeout(_)) => return SplPollOutcome::Continue,
            Err(error) => {
                return SplPollOutcome::Failed {
                    token: "portal_unreachable".to_owned(),
                    detail: Some(error.to_string()),
                };
            }
        };
        match response.status().as_u16() {
            204 => SplPollOutcome::Continue,
            400 => SplPollOutcome::Failed {
                token: "nonce_invalid".to_owned(),
                detail: None,
            },
            410 => SplPollOutcome::Failed {
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
                Some(value) => SplPollOutcome::Success(value),
                None => SplPollOutcome::Failed {
                    token: "unexpected_payload".to_owned(),
                    detail: None,
                },
            },
            _ => SplPollOutcome::Failed {
                token: "unexpected_payload".to_owned(),
                detail: None,
            },
        }
    }
}
impl SplEnrollment for RelayEnrollment {
    fn enroll(
        &self,
        journal: &std::path::Path,
        identity: &ServiceIdentity,
        ca_pubkey: &str,
    ) -> Result<String, EnrollError> {
        enroll_home(
            &solstone_core_spl::relay_url(journal),
            &identity.instance_id,
            ca_pubkey,
            &identity.home_label,
        )
    }
}

pub fn router(prefix: &str) -> axum::Router {
    let runtime = SplRuntime {
        portal_base_url: std::env::var("SERVICES_PORTAL_URL")
            .unwrap_or_else(|_| DEFAULT_PORTAL_URL.to_owned())
            .trim_end_matches('/')
            .to_owned(),
        poll: Arc::new(PortalPoll),
        enrollment: Arc::new(RelayEnrollment),
    };
    axum::Router::new()
        .route(
            &format!("{prefix}/host-address"),
            axum::routing::post(set_home_address),
        )
        .route(
            &format!("{prefix}/private-link/enable"),
            axum::routing::post(private_link_enable),
        )
        .route(
            &format!("{prefix}/private-link/disable"),
            axum::routing::post(private_link_disable),
        )
        .layer(Extension(runtime))
}

async fn set_home_address(
    Extension(journal): Extension<Arc<JournalRoot>>,
    body: Bytes,
) -> Response {
    let object = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let requested = object
        .get("home_address")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let expected_port = match read_direct_door_port(&journal.0) {
        Ok(port) => port,
        Err(_) => {
            return refusal(
                "service_operation_failed",
                "couldn't save your home address",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let address = match requested {
        Some(value) => match validate_home_address(value, expected_port) {
            Ok(value) => Some(value),
            Err(detail) => {
                return refusal("invalid_config_value", &detail, StatusCode::BAD_REQUEST);
            }
        },
        None => None,
    };
    let result = mutate_journal_config(&journal.0, Default::default(), |config| {
        let pairing = object_at(config, "pairing");
        let next = address.as_ref().map(|value| Value::String(value.clone()));
        let changed = pairing.get("home_address") != next.as_ref();
        if let Some(value) = &address {
            pairing.insert("home_address".to_owned(), Value::String(value.clone()));
        } else {
            pairing.remove("home_address");
        }
        JournalConfigMutation { changed, value: () }
    });
    if result.is_err() {
        return refusal(
            "service_operation_failed",
            "couldn't save your home address",
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    let value = std::fs::read(journal.0.join("config/journal.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("pairing")
                .and_then(|value| value.get("home_address"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    Json(json!({"ok": true, "home_address": value})).into_response()
}

async fn private_link_enable(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
    Extension(runtime): Extension<SplRuntime>,
    override_runtime: Option<Extension<SplRuntimeOverride>>,
    override_operations: Option<Extension<NetworkOperationsOverride>>,
) -> Response {
    let operations = override_operations
        .map(|Extension(value)| value.0)
        .unwrap_or(operations);
    if private_link_body(&journal.0, Some(operations.operation_raw(SERVICE))).state == "enabled" {
        return refusal(
            "invalid_operation_for_state",
            &copy("SPL_PRIVATE_LINK_ALREADY_ENABLED_DETAIL"),
            StatusCode::BAD_REQUEST,
        );
    }
    let runtime = override_runtime
        .map(|Extension(value)| SplRuntime {
            portal_base_url: value.portal_base_url.trim_end_matches('/').to_owned(),
            poll: value.poll,
            enrollment: value.enrollment,
        })
        .unwrap_or(runtime);
    let identity = match load_or_create_service_identity(&journal.0, "solstone") {
        Ok(value) => value,
        Err(_) => {
            return refusal(
                "service_operation_failed",
                &copy("SPL_PRIVATE_LINK_CONSENT_LINK_PREPARE_FAILED_DETAIL"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let nonce = match mint_nonce() {
        Ok(value) => value,
        Err(_) => {
            return refusal(
                "service_operation_failed",
                &copy("SPL_PRIVATE_LINK_CONSENT_LINK_PREPARE_FAILED_DETAIL"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let portal_url = format!(
        "{}/enable/spl?nonce={nonce}&instance={}",
        runtime.portal_base_url, identity.instance_id
    );
    let (handle, operation) =
        match operations.start_operation(SERVICE, "spl_enable", Some(portal_url)) {
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

async fn private_link_disable(
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(operations): Extension<Arc<OperationRegistry>>,
    pair_windows: Option<Extension<Arc<PairWindowManager>>>,
    override_operations: Option<Extension<NetworkOperationsOverride>>,
    forced_failure: Option<Extension<SplDisableFailureOverride>>,
) -> Response {
    let operations = override_operations
        .map(|Extension(value)| value.0)
        .unwrap_or(operations);
    if forced_failure.is_some() {
        return refusal(
            "service_operation_failed",
            "",
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    match disable_spl(&journal.0) {
        Ok(result) => {
            if let Some(Extension(pair_windows)) = pair_windows
                && pair_windows
                    .retire_all(&journal.0, unix_seconds())
                    .await
                    .is_err()
            {
                return refusal(
                    "service_operation_failed",
                    "",
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
            let mut status = serde_json::to_value(private_link_body(
                &journal.0,
                Some(operations.operation_raw(SERVICE)),
            ))
            .expect("status serializes");
            status
                .as_object_mut()
                .expect("status object")
                .remove("success");
            Json(json!({"success":true,"service":SERVICE,"result":{"was_enabled":result.was_enabled},"status":status})).into_response()
        }
        Err(error) => {
            log::warn!("private-link disable failed: {error}");
            refusal(
                "service_operation_failed",
                "",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    }
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

fn spawn_handoff(
    journal: std::path::PathBuf,
    operations: Arc<OperationRegistry>,
    handle: OperationHandle,
    runtime: SplRuntime,
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
                Ok(SplPollOutcome::Continue) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Ok(SplPollOutcome::Continue) => break outcome(Phase::Error, "expired", None),
                Ok(SplPollOutcome::Failed { token, detail }) => {
                    break outcome_for_token(&token, detail);
                }
                Ok(SplPollOutcome::Success(payload)) => match classify_payload(&payload) {
                    Ok(("pending", _)) if Instant::now() < deadline => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    Ok(("revoked", _)) => break outcome(Phase::Revoked, "revoked", None),
                    Ok(("needs_subscription", url)) => {
                        break outcome(Phase::NeedsSubscription, "needs_subscription", url);
                    }
                    Ok(("approved", _)) => {
                        let enroll = runtime.enrollment.clone();
                        let result = enable_spl_with(&journal, |identity, ca| {
                            enroll.enroll(&journal, identity, ca)
                        });
                        break match result {
                            Ok(()) => outcome(Phase::Enabled, "approved", None),
                            Err(solstone_core_spl::EnableSplError::Enroll(error)) => {
                                outcome_from_enroll(error)
                            }
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
        "portal_unreachable" | "tls_verification_failed" | "relay_unreachable" => "network_error",
        _ => "malformed",
    };
    outcome(Phase::Error, code, None)
}
fn outcome_from_enroll(error: EnrollError) -> HandoffResult {
    match error {
        EnrollError::Rejected {
            status: 409,
            reason: Some(reason),
        } if reason == "ca_pubkey already registered to another instance" => {
            outcome(Phase::Error, "relay_identity_conflict", None)
        }
        EnrollError::Rejected {
            status: 409,
            reason: Some(reason),
        } if reason == "ca_pubkey mismatch — rotation not supported in v1" => {
            outcome(Phase::Error, "relay_rotation_unsupported", None)
        }
        EnrollError::Rejected { status: 503, .. } => {
            outcome(Phase::Error, "relay_unavailable", None)
        }
        EnrollError::Rejected { status, .. } => {
            outcome_with_status(Phase::Error, "relay_rejected", None, Some(status))
        }
        EnrollError::Unreachable(_) => outcome(Phase::Error, "network_error", None),
        EnrollError::Response(_) => outcome(Phase::Error, "local_error", None),
    }
}
fn outcome(phase: Phase, code: &str, subscribe_url: Option<String>) -> HandoffResult {
    outcome_with_status(phase, code, subscribe_url, None)
}
fn outcome_with_status(
    phase: Phase,
    code: &str,
    subscribe_url: Option<String>,
    relay_status: Option<u16>,
) -> HandoffResult {
    HandoffResult {
        phase,
        guidance: guidance(code, relay_status),
        retryable: matches!(
            code,
            "expired" | "network_error" | "local_error" | "relay_unavailable" | "relay_rejected"
        ),
        subscribe_url,
    }
}
fn guidance(code: &str, relay_status: Option<u16>) -> Option<String> {
    serde_json::from_str::<Value>(assets::spl_outcome_strings_json())
        .ok()?
        .get("SPL_OUTCOME_GUIDANCE")?
        .get(code)?
        .as_str()
        .map(|value| {
            if code == "relay_rejected" {
                value.replace("{code}", &relay_status.unwrap_or_default().to_string())
            } else {
                value.to_owned()
            }
        })
}
fn copy(name: &str) -> String {
    let source = if name.starts_with("HOME_ADDRESS") {
        assets::home_address_strings_json()
    } else {
        assets::spl_outcome_strings_json()
    };
    serde_json::from_str::<Value>(source)
        .ok()
        .and_then(|value| value.get(name).and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
}
fn validate_home_address(value: &str, expected_port: u16) -> Result<String, String> {
    let cleaned = value.trim();
    if cleaned.is_empty() || cleaned.contains("://") || cleaned.contains('/') {
        return Err(copy("HOME_ADDRESS_INVALID"));
    }
    let Some((host, port)) = cleaned.rsplit_once(':') else {
        return Err(if looks_hostname(cleaned) {
            copy("HOME_ADDRESS_HOSTNAME_UNSUPPORTED")
        } else {
            copy("HOME_ADDRESS_INVALID")
        });
    };
    if host.is_empty() || port.is_empty() {
        return Err(if looks_hostname(cleaned) {
            copy("HOME_ADDRESS_HOSTNAME_UNSUPPORTED")
        } else {
            copy("HOME_ADDRESS_INVALID")
        });
    }
    let ipv4 = host.parse::<Ipv4Addr>().map_err(|_| {
        if looks_hostname(host) {
            copy("HOME_ADDRESS_HOSTNAME_UNSUPPORTED")
        } else {
            copy("HOME_ADDRESS_INVALID")
        }
    })?;
    let port = port
        .parse::<u16>()
        .map_err(|_| copy("HOME_ADDRESS_INVALID"))?;
    if port != expected_port || !is_usable_ipv4(ipv4) {
        return Err(copy("HOME_ADDRESS_INVALID"));
    }
    Ok(format!("{ipv4}:{port}"))
}
fn looks_hostname(value: &str) -> bool {
    value.contains('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        && value.bytes().any(|byte| byte.is_ascii_alphabetic())
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
