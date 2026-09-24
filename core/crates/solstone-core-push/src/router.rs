// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::body::to_bytes;
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::identity::{AccessBasis, LinkedDeviceCid};

use crate::envelope::PushKey;
use crate::model::{
    PushEnvironment, PushPlatform, ReasonCode, StatusResponse, TestResponse, device_token_is_valid,
};
use crate::store::{PushRegistry, PushStoreError};

const REGISTER_BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
struct PushState {
    registry: PushRegistry,
}

/// Build the public push API routes for one journal root.
pub fn api_router(journal_root: impl AsRef<Path>) -> Router {
    Router::new()
        .route(
            "/api/push/register",
            post(register_push_device).delete(deregister_push_device),
        )
        .route("/api/push/status", get(push_status))
        .route("/api/push/test", post(push_test))
        .with_state(PushState {
            registry: PushRegistry::new(journal_root),
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
    match state.registry.device_count() {
        Err(error) => push_registry_unavailable(error),
        Ok(0) => error_envelope(
            ReasonCode::FeatureUnavailable.as_str(),
            "Push test unavailable",
            "no devices to reach",
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response(),
        Ok(device_count) => Json(TestResponse { device_count }).into_response(),
    }
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

    use super::api_router;
    use crate::envelope::PushKey;
    use crate::store::set_test_clock;
    use crate::test_log;

    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TOKEN_A1: &str = "0123456789abcdef";
    const TOKEN_A2: &str = "fedcba9876543210";
    const VALID_KEY_B64: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio"; // 42 repeat 32 url-safe unpadded

    fn basis(cid: &str) -> AccessBasis {
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(cid).expect("fixture CID"),
        }
    }

    fn root() -> TempDir {
        TempDir::new_in("/var/tmp").expect("journal root")
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
        let app = api_router(root.path());

        let t1 = OffsetDateTime::from_unix_timestamp(1_758_672_000).unwrap(); // 2025-09-24T00:00:00Z
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

        let t2 = OffsetDateTime::from_unix_timestamp(1_758_672_100).unwrap(); // 2025-09-24T00:01:40Z
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
        let app = api_router(root.path());
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
        let app = api_router(root.path());
        set_test_clock(None);

        let key_bytes_a = [0x55u8; 32];
        let key_bytes_b = [0x77u8; 32];
        let key_a_b64url = URL_SAFE_NO_PAD.encode(key_bytes_a);
        let key_a_std = STANDARD.encode(key_bytes_a);
        let key_a_hex = (0..32).map(|_| "55").collect::<String>();
        let key_a_rust_dbg = format!("{key_bytes_a:?}");

        // 1. Successful register
        let (status_reg, body_reg) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_register_body(TOKEN_A1, "org.example", &key_a_b64url),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status_reg, StatusCode::CREATED);

        // 2. Register of same key with '=' padding -> 400
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

        // 3. Positive-control 503 from corrupt registry
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

        // Check positive-control log presence
        let logs = test_log::records_for_current_thread();
        assert!(
            logs.iter()
                .any(|(lvl, msg)| *lvl == Level::Warn && msg.contains("push registry unavailable")),
            "expected warn log for unavailable registry"
        );

        // 4. Twin file without '=' returns 200
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

        // Assert key encodings are absent from all responses and logs
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

        // Check PushKey Debug redaction
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
        let app = api_router(root.path());
        set_test_clock(None);

        // Unmutated body is 201
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
                    "device_token": "0123456789ABCDEF", // uppercase
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "0123456789abcde", // odd length (15)
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "0123456789abcd", // 14-char
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "00".repeat(101), // 202-char
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": VALID_KEY_B64
                }),
                "device_token",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": "", // empty
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
                    "bundle_id": "   ", // blank bundle_id
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
                    "environment": "sandbox", // invalid environment
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
                    "push_key": URL_SAFE_NO_PAD.encode([0u8; 31]) // 31-byte key
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": URL_SAFE_NO_PAD.encode([0u8; 33]) // 33-byte key
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": format!("{VALID_KEY_B64}=") // = padding
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development",
                    "push_key": STANDARD.encode([255u8; 32]) // + or /
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": 5, // non-string
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
                    "extra_key": "val" // unknown key
                }),
                "extra_key",
            ),
            (
                json!({
                    "platform": "ios",
                    "device_token": TOKEN_A1,
                    "bundle_id": "org.example",
                    "environment": "development"
                    // missing push_key
                }),
                "push_key",
            ),
            (
                json!({
                    "platform": "android", // invalid platform
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

        // DELETE body with extra push_key
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
        let app = api_router(root.path());
        set_test_clock(None);

        // (i) Two tokens under A both stay, second register is 201
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

        // (ii) B registering A's t1 is 201, t1 moves to B, A's t2 stays
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

        // (iii) B DELETE of A's t2 is 204 and removes nothing
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

        // (iv) A DELETE of A's own t1 is 204 (already stolen, removes nothing) and DELETE of t2 removes it
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
        let app = api_router(root_legacy.path());
        set_test_clock(None);

        let v1_bytes = br#"{"devices":{"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":{"device_token":"0a1b2c","bundle_id":"app.solstone.swift","environment":"development","platform":"ios","registered_at":"2026-08-27T12:00:00Z"},"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb":{"device_token":" Token AbCd ","bundle_id":"app.solstone.swift","environment":"production","platform":"ios","registered_at":"2026-08-28T12:00:00Z"}}}"#;
        let reg_path = root_legacy.path().join("config/push-registry.json");
        fs::create_dir_all(root_legacy.path().join("config")).unwrap();
        fs::write(&reg_path, v1_bytes).unwrap();

        // 1. Status is {"items":[],"total":0,"cursor":null}
        let (status, body) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"items":[],"total":0,"cursor":null}));

        // 2. Test returns 503
        let (status_test, body_test) =
            call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status_test, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body_test["reason_code"], "feature_unavailable");

        // 3. No-match DELETE returns 204
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

        // File unchanged and no info log yet
        assert_eq!(fs::read(&reg_path).unwrap(), v1_bytes);
        assert!(
            !test_log::records_for_current_thread()
                .iter()
                .any(|(lvl, _)| *lvl == Level::Info)
        );

        // 4. Register writes v2 and logs exactly one info record with count 2
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

        // {"devices":{}} reads as empty and its write logs nothing
        test_log::clear_current_thread();
        let root_empty_v1 = root();
        let app_empty_v1 = api_router(root_empty_v1.path());
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

        // Corrupted v1 with environment: 7 returns 503
        let root_corrupt_v1 = root();
        let app_corrupt_v1 = api_router(root_corrupt_v1.path());
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
            let app = api_router(root.path());
            let reg_path = root.path().join("config/push-registry.json");
            fs::create_dir_all(root.path().join("config")).unwrap();
            let raw_bytes = serde_json::to_vec(&mutation).unwrap();
            fs::write(&reg_path, &raw_bytes).unwrap();

            // Status -> 503
            let (s_stat, r_stat) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
            assert_eq!(s_stat, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(r_stat["reason_code"], "push_registry_unavailable");

            // Test -> 503
            let (s_test, r_test) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
            assert_eq!(s_test, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(r_test["reason_code"], "push_registry_unavailable");

            // Register -> 503
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

            // DELETE -> 503
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

            // Bytes unmodified
            assert_eq!(fs::read(&reg_path).unwrap(), raw_bytes);
        }
    }

    #[tokio::test]
    async fn directory_as_file_returns_503_on_all_routes() {
        let root = root();
        fs::create_dir_all(root.path().join("config/push-registry.json")).unwrap();
        let app = api_router(root.path());

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
}
