// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::identity::{AccessBasis, LinkedDeviceCid};
use solstone_core_sol_link::ledger::{AuthorizedClientsRead, read_authorized_clients};
use time::OffsetDateTime;

use crate::envelope::{Notification, PushKey, SealedEnvelope, seal};
use crate::model::{
    PushEnvironment, PushPlatform, ReasonCode, StatusResponse, TestItem, TestResponse,
    device_token_is_valid, mask_target, sanitize_reason,
};
use crate::relay::{RelayFault, RelayTransport, UreqRelay};
use crate::store::{PushRegistry, PushStoreError, StoredDevice, parse_registered_at};

const REGISTER_BODY_LIMIT: usize = 1024 * 1024;
const DISPATCH_BATCH_SIZE: usize = 16;
const RECIPIENT_EXPIRY_DAYS: i64 = 30;

#[derive(Clone)]
struct PushState {
    registry: PushRegistry,
    journal_root: PathBuf,
    portal_base: String,
    transport: Arc<dyn RelayTransport>,
}

/// Build the public push API routes for one journal root using the default Ureq transport.
pub fn api_router(journal_root: impl AsRef<Path>, portal_base: impl Into<String>) -> Router {
    api_router_with_transport(journal_root, portal_base, Arc::new(UreqRelay::default()))
}

/// Build the public push API routes with a custom relay transport.
pub(crate) fn api_router_with_transport(
    journal_root: impl AsRef<Path>,
    portal_base: impl Into<String>,
    transport: Arc<dyn RelayTransport>,
) -> Router {
    let journal_root = journal_root.as_ref().to_path_buf();
    let portal_base = portal_base.into().trim_end_matches('/').to_string();
    Router::new()
        .route(
            "/api/push/register",
            post(register_push_device).delete(deregister_push_device),
        )
        .route("/api/push/status", get(push_status))
        .route("/api/push/test", post(push_test))
        .with_state(PushState {
            registry: PushRegistry::new(&journal_root),
            journal_root,
            portal_base,
            transport,
        })
}

async fn register_push_device(
    State(state): State<PushState>,
    basis: Option<Extension<AccessBasis>>,
    request: Request,
) -> Response {
    let Some(cid) = linked_device_cid(basis) else {
        return linked_device_required();
    };
    let body = match to_bytes(request.into_body(), REGISTER_BODY_LIMIT).await {
        Ok(body) => body,
        Err(_) => return push_request_invalid("request body must be valid JSON"),
    };
    let registration = match parse_registration(&body) {
        Ok(registration) => registration,
        Err(detail) => return push_request_invalid(detail),
    };
    match state.registry.register(
        &cid,
        registration.device_token,
        registration.bundle_id,
        registration.environment,
        registration.platform,
        registration.push_key,
    ) {
        Ok((is_created, item)) => {
            let status = if is_created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(item)).into_response()
        }
        Err(error) => push_registry_unavailable(error),
    }
}

async fn deregister_push_device(
    State(state): State<PushState>,
    basis: Option<Extension<AccessBasis>>,
    request: Request,
) -> Response {
    let Some(cid) = linked_device_cid(basis) else {
        return linked_device_required();
    };
    let body = match to_bytes(request.into_body(), REGISTER_BODY_LIMIT).await {
        Ok(body) => body,
        Err(_) => return push_request_invalid("request body must be valid JSON"),
    };
    let deregistration = match parse_deregistration(&body) {
        Ok(deregistration) => deregistration,
        Err(detail) => return push_request_invalid(detail),
    };
    match state
        .registry
        .deregister(&cid, deregistration.platform, &deregistration.device_token)
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => push_registry_unavailable(error),
    }
}

async fn push_status(State(state): State<PushState>) -> Response {
    match state.registry.status() {
        Ok((items, total)) => Json(StatusResponse {
            items,
            total,
            cursor: None,
        })
        .into_response(),
        Err(error) => push_registry_unavailable(error),
    }
}

async fn push_test(State(state): State<PushState>) -> Response {
    #[cfg(test)]
    let now =
        crate::store::TEST_CLOCK.with(|cell| cell.borrow().unwrap_or_else(OffsetDateTime::now_utc));
    #[cfg(not(test))]
    let now = OffsetDateTime::now_utc();

    let handle = tokio::task::spawn_blocking(move || execute_push_test(state, now));

    match handle.await {
        Ok(response) => response,
        Err(_) => push_registry_unavailable(PushStoreError::Clock),
    }
}

fn execute_push_test(state: PushState, now: OffsetDateTime) -> Response {
    let loaded_devices = match state.registry.read_devices_locked() {
        Ok(devices) => devices,
        Err(error) => return push_registry_unavailable(error),
    };

    if loaded_devices.is_empty() {
        return feature_unavailable_no_devices();
    }

    let read_state =
        read_authorized_clients(&state.journal_root.join("link/authorized_clients.json"));
    match read_state {
        AuthorizedClientsRead::Unreadable
        | AuthorizedClientsRead::Malformed
        | AuthorizedClientsRead::DuplicateCid => {
            return error_envelope(
                ReasonCode::PushLedgerUnavailable.as_str(),
                "Push ledger unavailable",
                "the authorization ledger is unreadable",
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .into_response();
        }
        _ => {}
    }

    let cutoff = now - time::Duration::days(RECIPIENT_EXPIRY_DAYS);
    let mut recent_devices = Vec::new();
    for device in loaded_devices {
        match &device {
            StoredDevice::Ios { registered_at, .. } => {
                if let Some(reg_time) = parse_registered_at(registered_at)
                    && reg_time >= cutoff
                {
                    recent_devices.push(device);
                }
            }
        }
    }

    if recent_devices.is_empty() {
        return feature_unavailable_no_devices();
    }

    let authorized_cids: HashSet<String> = match read_state {
        AuthorizedClientsRead::Present(clients) => {
            clients.into_iter().map(|c| c.fingerprint).collect()
        }
        _ => HashSet::new(),
    };

    let mut to_dispatch = Vec::new();

    for device in recent_devices {
        match device {
            StoredDevice::Ios {
                cid,
                device_token,
                bundle_id,
                environment,
                push_key,
                ..
            } => {
                if authorized_cids.contains(&cid) {
                    let target = mask_target(&device_token);
                    to_dispatch.push((cid, device_token, bundle_id, environment, push_key, target));
                }
            }
        }
    }

    if to_dispatch.is_empty() {
        return feature_unavailable_no_devices();
    }

    let items = dispatch_push_tests(&state, &to_dispatch, now);

    let sent = items.iter().filter(|i| i.outcome == "sent").count();
    let revoked = items.iter().filter(|i| i.outcome == "revoked").count();
    let failed = items.iter().filter(|i| i.outcome == "failed").count();

    if revoked > 0 || failed > 0 {
        let mut reasons = Vec::new();
        for item in &items {
            if let Some(r) = item.reason.as_deref()
                && !reasons.contains(&r)
            {
                reasons.push(r);
            }
        }
        let reasons_str = if reasons.is_empty() {
            "-".to_owned()
        } else {
            reasons.join(",")
        };
        log::warn!(
            "push test delivery sent={sent} revoked={revoked} failed={failed} reasons={reasons_str}"
        );
    }

    (StatusCode::OK, Json(TestResponse { items })).into_response()
}

#[derive(Serialize)]
struct RelayEnrollmentRequest<'a> {
    assertion: &'a str,
    ca_pubkey: &'a str,
    instance_id: &'a str,
}

#[derive(Deserialize)]
struct RelayEnrollmentResponse {
    token: String,
    token_type: String,
    instance_id: String,
}

fn is_valid_relay_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

fn obtain_relay_token(state: &PushState, wall_unix_seconds: i64) -> Result<String, String> {
    let committed =
        match solstone_core_sol_link::committed::load_committed_identity(&state.journal_root) {
            Ok(c) => c,
            Err(_) => {
                return Err("identity_unavailable".to_owned());
            }
        };

    let assertion = match solstone_core_sol_link::home_reach::sign_home_reach_assertion(
        "push.relay.enroll",
        &committed,
        wall_unix_seconds,
    ) {
        Ok(a) => a,
        Err(_) => {
            return Err("identity_unavailable".to_owned());
        }
    };

    let enroll_url = format!("{}/reach/push/relay-token", state.portal_base);
    let enroll_body = match serde_json::to_vec(&RelayEnrollmentRequest {
        assertion: &assertion.compact,
        ca_pubkey: &assertion.ca_pubkey_pem,
        instance_id: committed.instance_id(),
    }) {
        Ok(b) => b,
        Err(_) => return Err("identity_unavailable".to_owned()),
    };

    let reply = match state.transport.post_json(&enroll_url, &enroll_body, None) {
        Ok(r) => r,
        Err(_) => return Err("relay_unreachable".to_owned()),
    };

    if reply.status == 401 || reply.status == 403 {
        return Err("relay_enrollment_rejected".to_owned());
    }
    if reply.status != 200 {
        return Err("relay_refused".to_owned());
    }

    let parsed: RelayEnrollmentResponse = match serde_json::from_slice(&reply.body) {
        Ok(p) => p,
        Err(_) => return Err("relay_refused".to_owned()),
    };

    if parsed.token_type != "Bearer"
        || parsed.instance_id != committed.instance_id()
        || !is_valid_relay_token(&parsed.token)
    {
        return Err("relay_refused".to_owned());
    }

    Ok(parsed.token)
}

#[derive(Serialize)]
struct DispatchRequest<'a> {
    devices: Vec<DispatchDeviceItem<'a>>,
}

#[derive(Serialize)]
struct DispatchDeviceItem<'a> {
    envelope: &'a SealedEnvelope,
    environment: &'static str,
    token: &'a str,
}

#[derive(Deserialize)]
struct DispatchResponse {
    results: Vec<DispatchResultItem>,
}

#[derive(Deserialize)]
struct DispatchResultItem {
    token: String,
    outcome: String,
    #[serde(default)]
    reason: Option<String>,
}

fn dispatch_push_tests(
    state: &PushState,
    devices: &[(String, String, String, PushEnvironment, PushKey, String)],
    now: OffsetDateTime,
) -> Vec<TestItem> {
    let token = match obtain_relay_token(state, now.unix_timestamp()) {
        Ok(token) => token,
        Err(reason) => {
            return devices
                .iter()
                .map(|(_, _, _, _, _, target)| TestItem {
                    target: target.clone(),
                    outcome: "failed".to_owned(),
                    reason: Some(reason.clone()),
                })
                .collect();
        }
    };

    let notif = Notification {
        at: now,
        kind: "test".to_owned(),
        title: "solstone".to_owned(),
        body: "test notification from your journal.".to_owned(),
        open: None,
    };

    let mut results = Vec::new();

    for chunk in devices.chunks(DISPATCH_BATCH_SIZE) {
        let mut sealed_envelopes = Vec::new();
        let mut chunk_to_send = Vec::new();
        let mut batch_results = Vec::new();

        for (_cid, token_str, _bundle, env, key, target) in chunk {
            match seal(key, &notif) {
                Ok(sealed) => {
                    sealed_envelopes.push(sealed);
                    let env_str = match env {
                        PushEnvironment::Development => "sandbox",
                        PushEnvironment::Production => "production",
                    };
                    chunk_to_send.push((target.clone(), token_str.as_str(), env_str));
                }
                Err(err) => {
                    log::warn!("failed to seal push test envelope: {err}");
                    batch_results.push(TestItem {
                        target: target.clone(),
                        outcome: "failed".to_owned(),
                        reason: Some("unspecified".to_owned()),
                    });
                }
            }
        }

        if !chunk_to_send.is_empty() {
            let dispatch_items: Vec<DispatchDeviceItem<'_>> = chunk_to_send
                .iter()
                .zip(sealed_envelopes.iter())
                .map(|((_, token_str, env_str), sealed)| DispatchDeviceItem {
                    envelope: sealed,
                    environment: env_str,
                    token: token_str,
                })
                .collect();

            let dispatch_url = format!("{}/push/dispatch", state.portal_base);
            let dispatch_body = serde_json::to_vec(&DispatchRequest {
                devices: dispatch_items,
            })
            .unwrap_or_default();

            let batch_tokens: Vec<&str> = chunk_to_send
                .iter()
                .map(|(_, token_str, _)| *token_str)
                .collect();

            match state
                .transport
                .post_json(&dispatch_url, &dispatch_body, Some(&token))
            {
                Ok(reply) if reply.status == 200 => {
                    let parsed: Result<DispatchResponse, _> = serde_json::from_slice(&reply.body);
                    match parsed {
                        Ok(resp) if resp.results.len() == chunk_to_send.len() => {
                            let valid = resp.results.iter().zip(chunk_to_send.iter()).all(
                                |(res_item, (_, dev_token, _))| {
                                    res_item.token == *dev_token
                                        && matches!(
                                            res_item.outcome.as_str(),
                                            "sent" | "revoked" | "failed"
                                        )
                                },
                            );
                            if valid {
                                for (res_item, (target, _, _)) in
                                    resp.results.into_iter().zip(chunk_to_send)
                                {
                                    let reason = res_item
                                        .reason
                                        .as_deref()
                                        .map(|r| sanitize_reason(r, &batch_tokens));
                                    batch_results.push(TestItem {
                                        target,
                                        outcome: res_item.outcome,
                                        reason,
                                    });
                                }
                            } else {
                                for (target, _, _) in chunk_to_send {
                                    batch_results.push(TestItem {
                                        target,
                                        outcome: "failed".to_owned(),
                                        reason: Some("relay_response_unverifiable".to_owned()),
                                    });
                                }
                            }
                        }
                        _ => {
                            for (target, _, _) in chunk_to_send {
                                batch_results.push(TestItem {
                                    target,
                                    outcome: "failed".to_owned(),
                                    reason: Some("relay_response_unverifiable".to_owned()),
                                });
                            }
                        }
                    }
                }
                Ok(reply) => {
                    let reason = format!("relay_rejected_{}", reply.status);
                    for (target, _, _) in chunk_to_send {
                        batch_results.push(TestItem {
                            target,
                            outcome: "failed".to_owned(),
                            reason: Some(reason.clone()),
                        });
                    }
                }
                Err(RelayFault::Timeout) => {
                    for (target, _, _) in chunk_to_send {
                        batch_results.push(TestItem {
                            target,
                            outcome: "failed".to_owned(),
                            reason: Some("relay_timeout".to_owned()),
                        });
                    }
                }
                Err(_) => {
                    for (target, _, _) in chunk_to_send {
                        batch_results.push(TestItem {
                            target,
                            outcome: "failed".to_owned(),
                            reason: Some("relay_unreachable".to_owned()),
                        });
                    }
                }
            }
        }

        results.extend(batch_results);
    }

    results
}

fn feature_unavailable_no_devices() -> Response {
    error_envelope(
        ReasonCode::FeatureUnavailable.as_str(),
        "Push test unavailable",
        "no devices to reach",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

fn linked_device_cid(basis: Option<Extension<AccessBasis>>) -> Option<LinkedDeviceCid> {
    let Some(Extension(AccessBasis::LinkedDevice { cid, .. })) = basis else {
        return None;
    };
    Some(cid)
}

#[derive(Debug)]
pub(crate) struct Registration {
    pub device_token: String,
    pub bundle_id: String,
    pub environment: PushEnvironment,
    pub platform: PushPlatform,
    pub push_key: PushKey,
}

#[derive(Debug)]
pub(crate) struct Deregistration {
    pub platform: PushPlatform,
    pub device_token: String,
}

fn parse_registration(body: &[u8]) -> Result<Registration, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "request body must be valid JSON".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "request body must be a JSON object".to_owned())?;

    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "platform" | "device_token" | "bundle_id" | "environment" | "push_key"
        ) {
            return Err(format!("unknown field `{key}`"));
        }
    }

    for required in [
        "platform",
        "device_token",
        "bundle_id",
        "environment",
        "push_key",
    ] {
        if !object.contains_key(required) {
            return Err(format!("missing field `{required}`"));
        }
    }

    let platform_str = object
        .get("platform")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid `platform`".to_owned())?;
    let platform =
        PushPlatform::parse(platform_str).ok_or_else(|| "invalid `platform`".to_owned())?;

    let device_token = object
        .get("device_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid `device_token`".to_owned())?;
    if !device_token_is_valid(device_token) {
        return Err("invalid `device_token`".to_owned());
    }

    let bundle_id = object
        .get("bundle_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid `bundle_id`".to_owned())?;
    if bundle_id.trim().is_empty() {
        return Err("invalid `bundle_id`".to_owned());
    }

    let environment_str = object
        .get("environment")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid `environment`".to_owned())?;
    let environment = PushEnvironment::parse(environment_str)
        .ok_or_else(|| "invalid `environment`".to_owned())?;

    let push_key_str = object
        .get("push_key")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid `push_key`".to_owned())?;
    let push_key =
        PushKey::from_base64url(push_key_str).map_err(|_| "invalid `push_key`".to_owned())?;

    Ok(Registration {
        device_token: device_token.to_owned(),
        bundle_id: bundle_id.to_owned(),
        environment,
        platform,
        push_key,
    })
}

fn parse_deregistration(body: &[u8]) -> Result<Deregistration, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "request body must be valid JSON".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "request body must be a JSON object".to_owned())?;

    for key in object.keys() {
        if !matches!(key.as_str(), "platform" | "device_token") {
            return Err(format!("unknown field `{key}`"));
        }
    }

    for required in ["platform", "device_token"] {
        if !object.contains_key(required) {
            return Err(format!("missing field `{required}`"));
        }
    }

    let platform_str = object
        .get("platform")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid `platform`".to_owned())?;
    let platform =
        PushPlatform::parse(platform_str).ok_or_else(|| "invalid `platform`".to_owned())?;

    let device_token = object
        .get("device_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid `device_token`".to_owned())?;
    if !device_token_is_valid(device_token) {
        return Err("invalid `device_token`".to_owned());
    }

    Ok(Deregistration {
        platform,
        device_token: device_token.to_owned(),
    })
}

fn linked_device_required() -> Response {
    error_envelope(
        ReasonCode::LinkedDeviceRequired.as_str(),
        "Linked device required",
        "a linked device identity is required",
        StatusCode::FORBIDDEN,
    )
    .into_response()
}

fn push_request_invalid(detail: impl Into<String>) -> Response {
    error_envelope(
        ReasonCode::PushRequestInvalid.as_str(),
        "Push request refused",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn push_registry_unavailable(error: PushStoreError) -> Response {
    log::warn!("push registry unavailable: {error}");
    error_envelope(
        ReasonCode::PushRegistryUnavailable.as_str(),
        "Push registry temporarily unavailable",
        "the push device registry is unavailable; try again shortly",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

#[cfg(test)]
pub(crate) fn load_registration_for_test(body: &[u8]) -> Result<Registration, String> {
    parse_registration(body)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use log::Level;
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
    use tempfile::TempDir;
    use time::OffsetDateTime;
    use tower::ServiceExt;

    use super::{api_router, api_router_with_transport};
    use crate::envelope::{PushKey, open_envelope};
    use crate::relay::{RelayFault, RelayReply, RelayTransport};
    use crate::store::set_test_clock;
    use crate::test_log;

    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TOKEN_A1: &str = "0123456789abcdef";
    const TOKEN_A2: &str = "fedcba9876543210";
    const VALID_KEY_B64: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio"; // 42 repeat 32 url-safe unpadded
    const PORTAL_URL: &str = "https://portal.solstone.test";

    fn basis(cid: &str) -> AccessBasis {
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(cid).expect("fixture CID"),
        }
    }

    fn root() -> TempDir {
        TempDir::new_in("/var/tmp").expect("journal root")
    }

    fn setup_authorized_client(root: &Path, cid: &str) {
        setup_authorized_client_with_role(
            root,
            cid,
            solstone_core_sol_link::ledger::ClientRole::Roleless,
        );
    }

    fn setup_authorized_client_with_role(
        root: &Path,
        cid: &str,
        role: solstone_core_sol_link::ledger::ClientRole,
    ) {
        let ca_dir = root.join("link/ca");
        if !ca_dir.exists() {
            let ca = solstone_core_sol_link::ca::generate_ca().expect("ca");
            let spki_der = ca.spki_der().to_vec();
            let instance_id = solstone_core_sol_link::ca::jid_from_spki(&spki_der).expect("jid");
            fs::create_dir_all(&ca_dir).expect("ca dir");
            fs::write(ca_dir.join("cert.pem"), ca.certificate_pem()).expect("cert");
            fs::write(ca_dir.join("private.pem"), ca.private_key_pem()).expect("key");
            fs::write(
                root.join("link/state.json"),
                format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
            )
            .expect("state");
        }

        let mut ledger = solstone_core_sol_link::ledger::AuthorizationLedger::new(root);
        let entry = solstone_core_sol_link::ledger::ClientEntry::new(
            cid,
            "Test Device",
            "2026-09-24T00:00:00Z",
            "inst-1",
            role,
        );
        let _ = ledger.add(entry);
    }

    type RecordedCall = (String, Vec<u8>, Option<String>);

    #[derive(Default)]
    struct MockTransport {
        calls: Mutex<Vec<RecordedCall>>,
        enroll_response: Mutex<Option<Result<RelayReply, RelayFault>>>,
        dispatch_response: Mutex<Option<Result<RelayReply, RelayFault>>>,
        dispatch_call_count: AtomicUsize,
        dispatch_responses: Mutex<Vec<Result<RelayReply, RelayFault>>>,
        sleep_duration: Mutex<Option<Duration>>,
    }

    impl RelayTransport for MockTransport {
        fn post_json(
            &self,
            url: &str,
            body: &[u8],
            bearer_token: Option<&str>,
        ) -> Result<RelayReply, RelayFault> {
            if let Some(dur) = *self.sleep_duration.lock().unwrap() {
                std::thread::sleep(dur);
            }
            self.calls.lock().unwrap().push((
                url.to_owned(),
                body.to_vec(),
                bearer_token.map(String::from),
            ));
            if url.ends_with("/reach/push/relay-token") {
                if let Some(resp) = self.enroll_response.lock().unwrap().clone() {
                    return resp;
                }
                let body_val: Value = serde_json::from_slice(body).unwrap();
                let inst = body_val["instance_id"].as_str().unwrap_or("inst-default");
                return Ok(RelayReply {
                    status: 200,
                    body: serde_json::to_vec(&json!({
                        "token": "mock-relay-token-123",
                        "token_type": "Bearer",
                        "instance_id": inst,
                    }))
                    .unwrap(),
                });
            }
            if url.ends_with("/push/dispatch") {
                self.dispatch_call_count.fetch_add(1, Ordering::Relaxed);
                let mut queued = self.dispatch_responses.lock().unwrap();
                if !queued.is_empty() {
                    return queued.remove(0);
                }
                drop(queued);

                if let Some(resp) = self.dispatch_response.lock().unwrap().clone() {
                    return resp;
                }
                let req_val: Value = serde_json::from_slice(body).unwrap();
                let devices_arr = req_val["devices"].as_array().unwrap();
                let results: Vec<Value> = devices_arr
                    .iter()
                    .map(|d| {
                        json!({
                            "token": d["token"],
                            "outcome": "sent",
                        })
                    })
                    .collect();
                return Ok(RelayReply {
                    status: 200,
                    body: serde_json::to_vec(&json!({ "results": results })).unwrap(),
                });
            }
            Ok(RelayReply {
                status: 404,
                body: Vec::new(),
            })
        }
    }

    async fn call_raw(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: impl Into<Body>,
        basis: Option<AccessBasis>,
    ) -> (StatusCode, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.into())
            .expect("request");
        if let Some(basis) = basis {
            request.extensions_mut().insert(basis);
        }
        let response = app.clone().oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        (status, bytes.to_vec())
    }

    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: impl Into<Body>,
        basis: Option<AccessBasis>,
    ) -> (StatusCode, Value) {
        let (status, bytes) = call_raw(app, method, uri, body, basis).await;
        let val: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            panic!(
                "JSON response from bytes: {}",
                String::from_utf8_lossy(&bytes)
            )
        });
        (status, val)
    }

    fn valid_register_body(token: &str, bundle_id: &str, push_key: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "platform": "ios",
            "device_token": token,
            "bundle_id": bundle_id,
            "environment": "development",
            "push_key": push_key
        }))
        .unwrap()
    }

    fn valid_deregister_body(token: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "platform": "ios",
            "device_token": token
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn register_at_t1_is_created_and_same_pair_at_t2_updates_key() {
        let root = root();
        let app = api_router(root.path(), PORTAL_URL);

        let t1 = OffsetDateTime::from_unix_timestamp(1_758_672_000).unwrap();
        set_test_clock(Some(t1));

        let key1 = URL_SAFE_NO_PAD.encode([1u8; 32]);
        let (status1, body1) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example.app", &key1),
            Some(basis(CID_A)),
        )
        .await;

        assert_eq!(status1, StatusCode::CREATED);
        assert_eq!(body1["platform"], "ios");
        assert_eq!(body1["target"], "...cdef");
        assert_eq!(body1["environment"], "development");
        assert_eq!(body1["registered_at"], "2025-09-24T00:00:00Z");

        let t2 = OffsetDateTime::from_unix_timestamp(1_758_672_100).unwrap();
        set_test_clock(Some(t2));

        let key2 = URL_SAFE_NO_PAD.encode([2u8; 32]);
        let (status2, body2) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example.app", &key2),
            Some(basis(CID_A)),
        )
        .await;

        assert_eq!(status2, StatusCode::OK);
        assert_eq!(body2["registered_at"], "2025-09-24T00:01:40Z");

        let registry: Value = serde_json::from_slice(
            &fs::read(root.path().join("config/push-registry.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(registry["version"], 2);
        assert_eq!(registry["devices"].as_array().unwrap().len(), 1);
        assert_eq!(
            registry["devices"][0]["registered_at"],
            "2025-09-24T00:01:40Z"
        );
        assert_eq!(registry["devices"][0]["push_key"], key2);
        assert!(!root.path().join("config/push_devices.json").exists());
    }

    #[tokio::test]
    async fn identity_is_refused_before_bad_body_or_storage_access() {
        let root = root();
        let app = api_router(root.path(), PORTAL_URL);
        for basis in [None, Some(AccessBasis::Localhost)] {
            let (status, body) = call(
                &app,
                "POST",
                "/api/push/register",
                b"not JSON".to_vec(),
                basis,
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body["reason_code"], "linked_device_required");
        }
        let (status, body) = call(&app, "DELETE", "/api/push/register", Body::empty(), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "linked_device_required");
        assert!(!root.path().join("config/push-registry.json").exists());
    }

    #[tokio::test]
    async fn push_key_is_absent_from_responses_logs_and_debug() {
        test_log::clear_current_thread();
        let root = root();
        let app = api_router(root.path(), PORTAL_URL);
        set_test_clock(None);

        let key_bytes_a = [0x55u8; 32];
        let key_bytes_b = [0x77u8; 32];
        let key_a_b64url = URL_SAFE_NO_PAD.encode(key_bytes_a);
        let key_a_std = STANDARD.encode(key_bytes_a);
        let key_a_hex = (0..32).map(|_| "55").collect::<String>();
        let key_a_rust_dbg = format!("{key_bytes_a:?}");

        let (status_reg, body_reg) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", &key_a_b64url),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status_reg, StatusCode::CREATED);

        let padded_key = format!("{key_a_b64url}=");
        let (status_bad, body_bad) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", &padded_key),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status_bad, StatusCode::BAD_REQUEST);

        let reg_path = root.path().join("config/push-registry.json");
        let twin_corrupt = json!({
            "version": 2,
            "devices": [{
                "platform": "ios",
                "cid": CID_A,
                "device_token": TOKEN_A1,
                "bundle_id": "org.example",
                "environment": "development",
                "push_key": padded_key,
                "registered_at": "2026-09-24T00:00:00Z"
            }]
        });
        fs::write(&reg_path, serde_json::to_vec(&twin_corrupt).unwrap()).unwrap();
        let (status_503, body_503) =
            call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(status_503, StatusCode::SERVICE_UNAVAILABLE);

        let logs = test_log::records_for_current_thread();
        assert!(
            logs.iter()
                .any(|(lvl, msg)| *lvl == Level::Warn && msg.contains("push registry unavailable")),
            "expected warn log for unavailable registry"
        );

        let twin_valid = json!({
            "version": 2,
            "devices": [{
                "platform": "ios",
                "cid": CID_A,
                "device_token": TOKEN_A1,
                "bundle_id": "org.example",
                "environment": "development",
                "push_key": key_a_b64url,
                "registered_at": "2026-09-24T00:00:00Z"
            }]
        });
        fs::write(&reg_path, serde_json::to_vec(&twin_valid).unwrap()).unwrap();
        let (status_ok, body_ok) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(status_ok, StatusCode::OK);
        assert_eq!(body_ok["total"], 1);

        for json_val in [&body_reg, &body_bad, &body_503, &body_ok] {
            let s = json_val.to_string();
            assert!(
                !s.contains(&key_a_b64url),
                "found b64url key in response: {s}"
            );
            assert!(
                !s.contains(&key_a_std),
                "found std b64 key in response: {s}"
            );
            assert!(!s.contains(&key_a_hex), "found hex key in response: {s}");
            assert!(
                !s.contains(&key_a_rust_dbg),
                "found rust dbg key in response: {s}"
            );
        }

        for (_, msg) in &logs {
            assert!(
                !msg.contains(&key_a_b64url),
                "found b64url key in log: {msg}"
            );
            assert!(!msg.contains(&key_a_std), "found std b64 key in log: {msg}");
            assert!(!msg.contains(&key_a_hex), "found hex key in log: {msg}");
            assert!(
                !msg.contains(&key_a_rust_dbg),
                "found rust dbg key in log: {msg}"
            );
        }

        let key_a = PushKey::from_bytes(key_bytes_a);
        let key_b = PushKey::from_bytes(key_bytes_b);
        assert_eq!(format!("{key_a:?}"), format!("{key_b:?}"));
        assert_eq!(format!("{key_a:?}"), "PushKey([redacted])");

        let parsed_reg = super::load_registration_for_test(&valid_register_body(
            TOKEN_A1,
            "org.example",
            &key_a_b64url,
        ))
        .unwrap();
        let reg_dbg = format!("{parsed_reg:?}");
        assert!(!reg_dbg.contains(&key_a_b64url));
        assert!(!reg_dbg.contains(&key_a_hex));
    }

    #[tokio::test]
    async fn registration_validation_names_the_failing_field() {
        let root = root();
        let app = api_router(root.path(), PORTAL_URL);
        set_test_clock(None);

        let (status, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let cases = [
            (
                json!({
                    "platform": "ios",
                    "device_token": "0123456789ABCDEF",
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "0123456789abcde",
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "0123456789abcd",
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "00".repeat(101),
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "",
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "   ",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "bundle_id",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "sandbox",
                    "push_key": VALID_KEY_B64
                }),
                "environment",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": URL_SAFE_NO_PAD.encode([0u8; 31])
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": URL_SAFE_NO_PAD.encode([0u8; 33])
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": format!("{VALID_KEY_B64}=")
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": STANDARD.encode([255u8; 32])
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": 5,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64,
                    "extra_key": "val"
                }),
                "extra_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development"
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "android",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "platform",
            ),
        ];

        for (body, field_name) in cases {
            let (status, resp) = call(
                &app,
                "POST",
                "/api/push/register",
                serde_json::to_vec(&body).unwrap(),
                Some(basis(CID_A)),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "failed for field {field_name}"
            );
            assert_eq!(resp["reason_code"], "push_request_invalid");
            let detail = resp["detail"].as_str().unwrap();
            assert!(
                detail.contains(field_name),
                "expected detail '{detail}' to contain '{field_name}'"
            );
        }

        let bad_del = json!({
            "platform": "ios",
            "device_token": TOKEN_A1,
            "push_key": VALID_KEY_B64
        });
        let (status, resp) = call(
            &app,
            "DELETE",
            "/api/push/register",
            serde_json::to_vec(&bad_del).unwrap(),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp["reason_code"], "push_request_invalid");
        assert!(resp["detail"].as_str().unwrap().contains("push_key"));
    }

    #[tokio::test]
    async fn cid_and_token_rows_stay_unless_that_pair_is_replaced() {
        let root = root();
        let app = api_router(root.path(), PORTAL_URL);
        set_test_clock(None);

        let (s1, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example.a", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(s1, StatusCode::CREATED);

        let (s2, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A2, "org.example.a", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(s2, StatusCode::CREATED);

        let (s_stat, body_stat) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(s_stat, StatusCode::OK);
        assert_eq!(body_stat["total"], 2);

        let (s3, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example.b", VALID_KEY_B64),
            Some(basis(CID_B)),
        )
        .await;
        assert_eq!(s3, StatusCode::CREATED);

        let registry: Value = serde_json::from_slice(
            &fs::read(root.path().join("config/push-registry.json")).unwrap(),
        )
        .unwrap();
        let devices = registry["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        assert!(
            devices
                .iter()
                .any(|d| d["cid"] == CID_B && d["device_token"] == TOKEN_A1)
        );
        assert!(
            devices
                .iter()
                .any(|d| d["cid"] == CID_A && d["device_token"] == TOKEN_A2)
        );

        let (del_status, del_body) = call_raw(
            &app,
            "DELETE",
            "/api/push/register",
            valid_deregister_body(TOKEN_A2),
            Some(basis(CID_B)),
        )
        .await;
        assert_eq!(del_status, StatusCode::NO_CONTENT);
        assert!(del_body.is_empty());

        let (s_stat2, body_stat2) =
            call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(s_stat2, StatusCode::OK);
        assert_eq!(body_stat2["total"], 2);

        let (del_status2, _) = call_raw(
            &app,
            "DELETE",
            "/api/push/register",
            valid_deregister_body(TOKEN_A2),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(del_status2, StatusCode::NO_CONTENT);

        let (s_stat3, body_stat3) =
            call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(s_stat3, StatusCode::OK);
        assert_eq!(body_stat3["total"], 1);
        assert_eq!(body_stat3["items"][0]["target"], "...cdef");
    }

    #[tokio::test]
    async fn legacy_v1_is_empty_until_a_write_discards_it() {
        test_log::clear_current_thread();
        let root_legacy = root();
        let app = api_router(root_legacy.path(), PORTAL_URL);
        set_test_clock(None);

        let v1_bytes = br#"{"devices":{"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":{"device_token":"0a1b2c","bundle_id":"app.solstone.swift","environment":"development","platform":"ios","registered_at":"2026-08-27T12:00:00Z"},"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb":{"device_token":" Token AbCd ","bundle_id":"app.solstone.swift","environment":"production","platform":"ios","registered_at":"2026-08-28T12:00:00Z"}}}"#;
        let reg_path = root_legacy.path().join("config/push-registry.json");
        fs::create_dir_all(root_legacy.path().join("config")).unwrap();
        fs::write(&reg_path, v1_bytes).unwrap();

        let (status, body) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"items":[],"total":0,"cursor":null}));

        let (status_test, body_test) =
            call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status_test, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_test["reason_code"], "feature_unavailable");

        let (status_del, del_bytes) = call_raw(
            &app,
            "DELETE",
            "/api/push/register",
            valid_deregister_body(TOKEN_A1),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status_del, StatusCode::NO_CONTENT);
        assert!(del_bytes.is_empty());

        assert_eq!(fs::read(&reg_path).unwrap(), v1_bytes);
        assert!(
            !test_log::records_for_current_thread()
                .iter()
                .any(|(lvl, _)| *lvl == Level::Info)
        );

        let (status_reg, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status_reg, StatusCode::CREATED);

        let info_logs: Vec<_> = test_log::records_for_current_thread()
            .into_iter()
            .filter(|(lvl, _)| *lvl == Level::Info)
            .collect();
        assert_eq!(info_logs.len(), 1);
        assert!(info_logs[0].1.contains('2'));

        test_log::clear_current_thread();
        let root_empty_v1 = root();
        let app_empty_v1 = api_router(root_empty_v1.path(), PORTAL_URL);
        let reg_empty_path = root_empty_v1.path().join("config/push-registry.json");
        fs::create_dir_all(root_empty_v1.path().join("config")).unwrap();
        fs::write(&reg_empty_path, b"{\"devices\":{}}").unwrap();

        let (status_empty_reg, _) = call(
            &app_empty_v1,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status_empty_reg, StatusCode::CREATED);
        assert!(
            !test_log::records_for_current_thread()
                .iter()
                .any(|(lvl, _)| *lvl == Level::Info)
        );

        let root_corrupt_v1 = root();
        let app_corrupt_v1 = api_router(root_corrupt_v1.path(), PORTAL_URL);
        let reg_corrupt_path = root_corrupt_v1.path().join("config/push-registry.json");
        fs::create_dir_all(root_corrupt_v1.path().join("config")).unwrap();
        let bad_v1 = br#"{"devices":{"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":{"device_token":"0a1b2c","bundle_id":"app.solstone.swift","environment":7,"platform":"ios","registered_at":"2026-08-27T12:00:00Z"}}}"#;
        fs::write(&reg_corrupt_path, bad_v1).unwrap();

        let (status_bad_v1, _) = call(
            &app_corrupt_v1,
            "GET",
            "/api/push/status",
            Body::empty(),
            None,
        )
        .await;
        assert_eq!(status_bad_v1, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(fs::read(&reg_corrupt_path).unwrap(), bad_v1);
    }

    #[tokio::test]
    async fn invalid_v2_registry_is_unavailable_and_unmodified() {
        let base_valid = json!({
            "version": 2,
            "devices": [{
                "platform": "ios",
                "cid": CID_A,
                "device_token": TOKEN_A1,
                "bundle_id": "org.example",
                "environment": "development",
                "push_key": VALID_KEY_B64,
                "registered_at": "2026-09-24T00:00:00Z"
            }]
        });

        let mutations = [
            json!({"version": 3, "devices": base_valid["devices"]}),
            json!({"version": "2", "devices": base_valid["devices"]}),
            json!({
                "version": 2,
                "devices": [{
                    "platform": "ios",
                    "cid": CID_A,
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64,
                    "registered_at": "2026-09-24T00:00:00Z",
                    "extra": "value"
                }]
            }),
            json!({
                "version": 2,
                "devices": [{
                    "platform": "ios",
                    "cid": CID_A,
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64,
                    "registered_at": "2026-09-24T00:00:00Z",
                    "endpoint": "https://example.com"
                }]
            }),
            json!({
                "version": 2,
                "devices": [{
                    "platform": "ios",
                    "cid": CID_A,
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": URL_SAFE_NO_PAD.encode([0u8; 31]),
                    "registered_at": "2026-09-24T00:00:00Z"
                }]
            }),
            json!({
                "version": 2,
                "devices": [{
                    "platform": "ios",
                    "cid": CID_A,
                    "device_token": "odd_token_123",
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64,
                    "registered_at": "2026-09-24T00:00:00Z"
                }]
            }),
            json!({
                "version": 2,
                "devices": [{
                    "platform": "ios",
                    "cid": "invalid_cid",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64,
                    "registered_at": "2026-09-24T00:00:00Z"
                }]
            }),
            json!({
                "version": 2,
                "devices": [{
                    "platform": "ios",
                    "cid": CID_A,
                    "device_token": TOKEN_A1,
                    "bundle_id": "   ",
                    "environment": "development",
                    "push_key": VALID_KEY_B64,
                    "registered_at": "2026-09-24T00:00:00Z"
                }]
            }),
            json!({
                "version": 2,
                "devices": [{
                    "platform": "ios",
                    "cid": CID_A,
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64,
                    "registered_at": "2026-09-24T00:00:00+00:00"
                }]
            }),
            json!({
                "version": 2,
                "devices": [
                    {
                        "platform": "ios",
                        "cid": CID_A,
                        "device_token": TOKEN_A1,
                        "bundle_id": "org.example",
                        "environment": "development",
                        "push_key": VALID_KEY_B64,
                        "registered_at": "2026-09-24T00:00:00Z"
                    },
                    {
                        "platform": "ios",
                        "cid": CID_A,
                        "device_token": TOKEN_A1,
                        "bundle_id": "org.example",
                        "environment": "development",
                        "push_key": VALID_KEY_B64,
                        "registered_at": "2026-09-24T00:00:00Z"
                    }
                ]
            }),
            json!({
                "version": 2,
                "devices": [
                    {
                        "platform": "ios",
                        "cid": CID_A,
                        "device_token": TOKEN_A1,
                        "bundle_id": "org.example",
                        "environment": "development",
                        "push_key": VALID_KEY_B64,
                        "registered_at": "2026-09-24T00:00:00Z"
                    },
                    {
                        "platform": "ios",
                        "cid": CID_B,
                        "device_token": TOKEN_A1,
                        "bundle_id": "org.example",
                        "environment": "development",
                        "push_key": VALID_KEY_B64,
                        "registered_at": "2026-09-24T00:00:00Z"
                    }
                ]
            }),
        ];

        for mutation in mutations {
            let root = root();
            let app = api_router(root.path(), PORTAL_URL);
            let reg_path = root.path().join("config/push-registry.json");
            fs::create_dir_all(root.path().join("config")).unwrap();
            let raw_bytes = serde_json::to_vec(&mutation).unwrap();
            fs::write(&reg_path, &raw_bytes).unwrap();

            let (s_stat, r_stat) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
            assert_eq!(s_stat, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(r_stat["reason_code"], "push_registry_unavailable");

            let (s_test, r_test) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
            assert_eq!(s_test, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(r_test["reason_code"], "push_registry_unavailable");

            let (s_reg, r_reg) = call(
                &app,
                "POST",
                "/api/push/register",
                valid_register_body(TOKEN_A2, "org.example", VALID_KEY_B64),
                Some(basis(CID_A)),
            )
            .await;
            assert_eq!(s_reg, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(r_reg["reason_code"], "push_registry_unavailable");

            let (s_del, r_del) = call(
                &app,
                "DELETE",
                "/api/push/register",
                valid_deregister_body(TOKEN_A1),
                Some(basis(CID_A)),
            )
            .await;
            assert_eq!(s_del, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(r_del["reason_code"], "push_registry_unavailable");

            assert_eq!(fs::read(&reg_path).unwrap(), raw_bytes);
        }
    }

    #[tokio::test]
    async fn directory_as_file_returns_503_on_all_routes() {
        let root = root();
        fs::create_dir_all(root.path().join("config/push-registry.json")).unwrap();
        let app = api_router(root.path(), PORTAL_URL);

        for (method, uri, body, basis) in [
            (
                "POST",
                "/api/push/register",
                valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64),
                Some(basis(CID_A)),
            ),
            (
                "DELETE",
                "/api/push/register",
                valid_deregister_body(TOKEN_A1),
                Some(basis(CID_A)),
            ),
            ("GET", "/api/push/status", Vec::new(), None),
            ("POST", "/api/push/test", Vec::new(), None),
        ] {
            let (status, response) = call(&app, method, uri, body, basis).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{method} {uri}");
            assert_eq!(response["reason_code"], "push_registry_unavailable");
        }
    }

    #[tokio::test]
    async fn push_test_omits_unauthorized_rows_and_returns_503_if_none_remain() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);

        // Register 1 device on CID_B only (unauthorized)
        let (s, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64),
            Some(basis(CID_B)),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);

        // Test should omit CID_B and return 503 feature_unavailable with zero transport calls
        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "feature_unavailable");
        assert_eq!(body["detail"], "no devices to reach");
        assert_eq!(transport.calls.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn push_test_delivers_to_authorized_devices() {
        test_log::clear();
        let root = root();
        setup_authorized_client(root.path(), CID_A);

        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);

        let (s1, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(s1, StatusCode::CREATED);

        let (status_test, body_test) =
            call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status_test, StatusCode::OK);

        let items = body_test["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["outcome"], "sent");
        assert_eq!(items[0]["target"], "...cdef");
        assert!(items[0].get("reason").is_none());
        assert!(items[0].get("cid").is_none());
        assert!(items[0].get("platform").is_none());
        assert!(items[0].get("environment").is_none());
    }

    #[tokio::test]
    async fn push_test_empty_registry_is_503_even_with_malformed_ledger() {
        let root = root();
        fs::create_dir_all(root.path().join("link")).unwrap();
        fs::write(
            root.path().join("link/authorized_clients.json"),
            b"not JSON",
        )
        .unwrap();

        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "feature_unavailable");
        assert_eq!(body["detail"], "no devices to reach");
        assert_eq!(transport.calls.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn push_test_recipient_with_ledger_directory_empty_or_duplicate_returns_503_ledger() {
        for setup_bad_ledger in [
            |path: &Path| {
                fs::create_dir_all(path.join("link/authorized_clients.json")).unwrap();
            },
            |path: &Path| {
                fs::create_dir_all(path.join("link")).unwrap();
                fs::write(path.join("link/authorized_clients.json"), b"{}").unwrap();
            },
            |path: &Path| {
                fs::create_dir_all(path.join("link")).unwrap();
                fs::write(
                    path.join("link/authorized_clients.json"),
                    json!({
                        "version": 1,
                        "clients": [
                            {"fingerprint": CID_A, "device_label": "d1", "paired_at": "2026-09-24T00:00:00Z", "instance_id": "i1"},
                            {"fingerprint": CID_A, "device_label": "d2", "paired_at": "2026-09-24T00:00:00Z", "instance_id": "i2"}
                        ]
                    })
                    .to_string(),
                )
                .unwrap();
            },
        ] {
            let root = root();
            let reg_body = valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64);
            let app_reg = api_router(root.path(), PORTAL_URL);
            let (s, _) = call(
                &app_reg,
                "POST",
                "/api/push/register",
                reg_body,
                Some(basis(CID_A)),
            )
            .await;
            assert_eq!(s, StatusCode::CREATED);

            setup_bad_ledger(root.path());

            let transport = Arc::new(MockTransport::default());
            let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

            let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["reason_code"], "push_ledger_unavailable");
            assert_eq!(transport.calls.lock().unwrap().len(), 0);
        }
    }

    #[tokio::test]
    async fn push_test_filters_devices_older_than_30_days() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        let t0 = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        set_test_clock(Some(t0));
        let (s1, _) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(s1, StatusCode::CREATED);

        let t31 = t0 + time::Duration::days(31);
        set_test_clock(Some(t31));

        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "feature_unavailable");
        assert_eq!(body["detail"], "no devices to reach");
    }

    #[tokio::test]
    async fn push_test_roles_and_timestamps_filter_correctly() {
        let root = root();
        let t_now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        set_test_clock(Some(t_now));

        let cid_a = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let cid_o = "sha256:0101010101010101010101010101010101010101010101010101010101010101";
        let cid_p = "sha256:0202020202020202020202020202020202020202020202020202020202020202";
        let cid_z = "sha256:0303030303030303030303030303030303030303030303030303030303030303";

        setup_authorized_client_with_role(
            root.path(),
            cid_a,
            solstone_core_sol_link::ledger::ClientRole::Roleless,
        );
        setup_authorized_client_with_role(
            root.path(),
            cid_o,
            solstone_core_sol_link::ledger::ClientRole::Unknown("observer".into()),
        );
        setup_authorized_client_with_role(
            root.path(),
            cid_p,
            solstone_core_sol_link::ledger::ClientRole::Unknown("peer".into()),
        );

        let app_reg = api_router(root.path(), PORTAL_URL);

        // A now
        call(
            &app_reg,
            "POST",
            "/api/push/register",
            valid_register_body("0000000000000001", "app", VALID_KEY_B64),
            Some(basis(cid_a)),
        )
        .await;
        // A 29d ago
        set_test_clock(Some(t_now - time::Duration::days(29)));
        call(
            &app_reg,
            "POST",
            "/api/push/register",
            valid_register_body("0000000000000002", "app", VALID_KEY_B64),
            Some(basis(cid_a)),
        )
        .await;
        // A 31d ago
        set_test_clock(Some(t_now - time::Duration::days(31)));
        call(
            &app_reg,
            "POST",
            "/api/push/register",
            valid_register_body("0000000000000003", "app", VALID_KEY_B64),
            Some(basis(cid_a)),
        )
        .await;
        // A +1h in future
        set_test_clock(Some(t_now + time::Duration::hours(1)));
        call(
            &app_reg,
            "POST",
            "/api/push/register",
            valid_register_body("0000000000000004", "app", VALID_KEY_B64),
            Some(basis(cid_a)),
        )
        .await;
        // O now
        set_test_clock(Some(t_now));
        call(
            &app_reg,
            "POST",
            "/api/push/register",
            valid_register_body("0000000000000005", "app", VALID_KEY_B64),
            Some(basis(cid_o)),
        )
        .await;
        // P now
        call(
            &app_reg,
            "POST",
            "/api/push/register",
            valid_register_body("0000000000000006", "app", VALID_KEY_B64),
            Some(basis(cid_p)),
        )
        .await;
        // Z (unauthorized)
        call(
            &app_reg,
            "POST",
            "/api/push/register",
            valid_register_body("0000000000000007", "app", VALID_KEY_B64),
            Some(basis(cid_z)),
        )
        .await;

        set_test_clock(Some(t_now));
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        let items = body["items"].as_array().unwrap();
        // Sent set: A-now, A-29d, A-future, O, P -> 5 items
        assert_eq!(items.len(), 5);
        let targets: Vec<&str> = items
            .iter()
            .map(|i| i["target"].as_str().unwrap())
            .collect();
        assert!(targets.contains(&"...0001"));
        assert!(targets.contains(&"...0002"));
        assert!(targets.contains(&"...0004"));
        assert!(targets.contains(&"...0005"));
        assert!(targets.contains(&"...0006"));
    }

    #[tokio::test]
    async fn push_test_seals_with_distinct_keys_and_opens_only_with_matching_key() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);

        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let key1_b64 = URL_SAFE_NO_PAD.encode(key1);
        let key2_b64 = URL_SAFE_NO_PAD.encode(key2);

        call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "app", &key1_b64),
            Some(basis(CID_A)),
        )
        .await;
        call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A2, "app", &key2_b64),
            Some(basis(CID_A)),
        )
        .await;

        let (status, _) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);

        let calls = transport.calls.lock().unwrap();
        let dispatch_call = calls
            .iter()
            .find(|(url, _, _)| url.ends_with("/push/dispatch"))
            .unwrap();
        let body_val: Value = serde_json::from_slice(&dispatch_call.1).unwrap();
        let devices = body_val["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);

        let env1_str = devices[0]["envelope"].as_str().unwrap();
        let env2_str = devices[1]["envelope"].as_str().unwrap();
        assert_eq!(env1_str.len(), 1404);
        assert_eq!(env2_str.len(), 1404);
        assert_ne!(env1_str, env2_str);

        let pk1 = PushKey::from_bytes(key1);
        let pk2 = PushKey::from_bytes(key2);

        let raw1 = URL_SAFE_NO_PAD.decode(env1_str).unwrap();
        let raw2 = URL_SAFE_NO_PAD.decode(env2_str).unwrap();

        assert!(open_envelope(&pk1, &raw1).is_ok());
        assert!(open_envelope(&pk2, &raw1).is_err());
        assert!(open_envelope(&pk2, &raw2).is_ok());
        assert!(open_envelope(&pk1, &raw2).is_err());
    }

    #[tokio::test]
    async fn push_test_wire_keys_and_environment_mapping() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), "https://relay.test", transport.clone());

        set_test_clock(None);

        call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "app", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;

        let (status, _) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);

        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);

        // Call 1: Enrollment
        let (enroll_url, enroll_body, enroll_bearer) = &calls[0];
        assert_eq!(enroll_url, "https://relay.test/reach/push/relay-token");
        assert!(enroll_bearer.is_none());
        let enroll_str = std::str::from_utf8(enroll_body).unwrap();
        let assertion_pos = enroll_str.find("\"assertion\"").unwrap();
        let ca_pos = enroll_str.find("\"ca_pubkey\"").unwrap();
        let inst_pos = enroll_str.find("\"instance_id\"").unwrap();
        assert!(assertion_pos < ca_pos);
        assert!(ca_pos < inst_pos);

        // Call 2: Dispatch
        let (dispatch_url, dispatch_body, dispatch_bearer) = &calls[1];
        assert_eq!(dispatch_url, "https://relay.test/push/dispatch");
        assert_eq!(dispatch_bearer.as_deref(), Some("mock-relay-token-123"));
        let dispatch_val: Value = serde_json::from_slice(dispatch_body).unwrap();
        let dev = &dispatch_val["devices"][0];
        assert!(dev.get("envelope").is_some());
        assert_eq!(dev["environment"], "sandbox"); // development maps to sandbox
        assert_eq!(dev["token"], TOKEN_A1);
    }

    #[tokio::test]
    async fn push_test_batches_16_and_17_recipients_with_one_enrollment() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);

        for i in 0..17 {
            let token = format!("{:016x}", i + 1);
            call(
                &app,
                "POST",
                "/api/push/register",
                valid_register_body(&token, "app", VALID_KEY_B64),
                Some(basis(CID_A)),
            )
            .await;
        }

        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"].as_array().unwrap().len(), 17);

        let calls = transport.calls.lock().unwrap();
        // 1 enrollment + 2 dispatches = 3 calls total
        assert_eq!(calls.len(), 3);
        assert!(calls[0].0.ends_with("/reach/push/relay-token"));
        assert!(calls[1].0.ends_with("/push/dispatch"));
        assert!(calls[2].0.ends_with("/push/dispatch"));

        let batch1: Value = serde_json::from_slice(&calls[1].1).unwrap();
        let batch2: Value = serde_json::from_slice(&calls[2].1).unwrap();
        assert_eq!(batch1["devices"].as_array().unwrap().len(), 16);
        assert_eq!(batch2["devices"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn push_test_batch_failures_and_unverifiable_responses() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);
        call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "app", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;

        // 400
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 400,
            body: b"bad".to_vec(),
        }));
        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["reason"], "relay_rejected_400");

        // 500
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 500,
            body: b"internal error".to_vec(),
        }));
        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["reason"], "relay_rejected_500");

        // Non-JSON 200
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 200,
            body: b"not json".to_vec(),
        }));
        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["reason"], "relay_response_unverifiable");

        // Too few results
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 200,
            body: serde_json::to_vec(&json!({"results": []})).unwrap(),
        }));
        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["reason"], "relay_response_unverifiable");

        // Wrong token
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 200,
            body: serde_json::to_vec(&json!({"results": [{"token": "wrong", "outcome": "sent"}]}))
                .unwrap(),
        }));
        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["reason"], "relay_response_unverifiable");

        // Invalid outcome
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 200,
            body: serde_json::to_vec(
                &json!({"results": [{"token": TOKEN_A1, "outcome": "throttled"}]}),
            )
            .unwrap(),
        }));
        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["reason"], "relay_response_unverifiable");

        // Mix of sent/revoked/failed plus unknown field maps 1-to-1
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 200,
            body: serde_json::to_vec(&json!({
                "results": [{
                    "token": TOKEN_A1,
                    "outcome": "revoked",
                    "reason": "unregistered",
                    "extra_unknown": 42
                }]
            }))
            .unwrap(),
        }));
        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["outcome"], "revoked");
        assert_eq!(b["items"][0]["reason"], "unregistered");
    }

    #[tokio::test]
    async fn push_test_17_recipients_partial_batch_timeout() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);
        for i in 0..17 {
            let token = format!("{:016x}", i + 1);
            call(
                &app,
                "POST",
                "/api/push/register",
                valid_register_body(&token, "app", VALID_KEY_B64),
                Some(basis(CID_A)),
            )
            .await;
        }

        // Batch 1 success, Batch 2 timeout
        let batch1_resp = {
            let results: Vec<Value> = (0..16)
                .map(|i| json!({"token": format!("{:016x}", i + 1), "outcome": "sent"}))
                .collect();
            Ok(RelayReply {
                status: 200,
                body: serde_json::to_vec(&json!({"results": results})).unwrap(),
            })
        };
        let batch2_resp = Err(RelayFault::Timeout);

        *transport.dispatch_responses.lock().unwrap() = vec![batch1_resp, batch2_resp];

        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 17);
        for item in &items[..16] {
            assert_eq!(item["outcome"], "sent");
        }
        assert_eq!(items[16]["outcome"], "failed");
        assert_eq!(items[16]["reason"], "relay_timeout");
    }

    #[tokio::test]
    async fn push_test_enrollment_error_mappings() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);
        call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "app", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;

        for (enroll_reply, expected_reason) in [
            (Ok(RelayReply { status: 401, body: b"{}".to_vec() }), "relay_enrollment_rejected"),
            (Ok(RelayReply { status: 403, body: b"{}".to_vec() }), "relay_enrollment_rejected"),
            (Ok(RelayReply { status: 500, body: b"{}".to_vec() }), "relay_refused"),
            (Ok(RelayReply { status: 200, body: serde_json::to_vec(&json!({"token": "", "token_type": "Bearer", "instance_id": "inst-1"})).unwrap() }), "relay_refused"),
            (Ok(RelayReply { status: 200, body: serde_json::to_vec(&json!({"token": "tok\r\n", "token_type": "Bearer", "instance_id": "inst-1"})).unwrap() }), "relay_refused"),
            (Ok(RelayReply { status: 200, body: serde_json::to_vec(&json!({"token": "valid-token", "token_type": "Basic", "instance_id": "inst-1"})).unwrap() }), "relay_refused"),
            (Ok(RelayReply { status: 200, body: serde_json::to_vec(&json!({"token": "valid-token", "token_type": "Bearer", "instance_id": "wrong-inst"})).unwrap() }), "relay_refused"),
            (Err(RelayFault::Connect), "relay_unreachable"),
            (Err(RelayFault::Timeout), "relay_unreachable"),
        ] {
            *transport.enroll_response.lock().unwrap() = Some(enroll_reply);
            let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
            assert_eq!(b["items"][0]["outcome"], "failed");
            assert_eq!(b["items"][0]["reason"], expected_reason);
        }
    }

    #[tokio::test]
    async fn push_test_secret_leak_prevention() {
        test_log::clear();
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());

        set_test_clock(None);
        call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "app", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;

        let hex_leak = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        *transport.dispatch_response.lock().unwrap() = Some(Ok(RelayReply {
            status: 200,
            body: serde_json::to_vec(&json!({
                "results": [{
                    "token": TOKEN_A1,
                    "outcome": "failed",
                    "reason": hex_leak
                }]
            }))
            .unwrap(),
        }));

        let (_, b) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(b["items"][0]["reason"], "unspecified");

        let logs = test_log::records();
        for (_, msg) in &logs {
            assert!(!msg.contains(hex_leak));
            assert!(!msg.contains(TOKEN_A1));
            assert!(!msg.contains("mock-relay-token-123"));
        }
    }

    #[tokio::test]
    async fn push_test_runs_on_worker_thread_and_does_not_block_runtime() {
        let root = root();
        setup_authorized_client(root.path(), CID_A);
        let transport = Arc::new(MockTransport::default());
        *transport.sleep_duration.lock().unwrap() = Some(Duration::from_millis(200));

        let app = api_router_with_transport(root.path(), PORTAL_URL, transport.clone());
        set_test_clock(None);
        call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "app", VALID_KEY_B64),
            Some(basis(CID_A)),
        )
        .await;

        let app_clone = app.clone();
        let handle = tokio::spawn(async move {
            call(&app_clone, "POST", "/api/push/test", Body::empty(), None).await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        let (status_quick, _) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(status_quick, StatusCode::OK);

        let (status_slow, _) = handle.await.unwrap();
        assert_eq!(status_slow, StatusCode::OK);
    }
}
