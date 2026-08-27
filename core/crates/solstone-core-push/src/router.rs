// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::body::to_bytes;
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Map, Value};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::identity::{AccessBasis, LinkedDeviceCid};

use crate::model::{
    DeregisterResponse, PushEnvironment, PushPlatform, ReasonCode, RegisterResponse,
    StatusResponse, TestResponse,
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
    if let Err(error) = state.registry.register(
        &cid,
        registration.device_token,
        registration.bundle_id,
        registration.environment,
        registration.platform,
    ) {
        return push_registry_unavailable(error);
    }
    Json(RegisterResponse { registered: true }).into_response()
}

async fn deregister_push_device(
    State(state): State<PushState>,
    basis: Option<Extension<AccessBasis>>,
) -> Response {
    let Some(cid) = linked_device_cid(basis) else {
        return linked_device_required();
    };
    match state.registry.deregister(&cid) {
        Ok(removed) => Json(DeregisterResponse { removed }).into_response(),
        Err(error) => push_registry_unavailable(error),
    }
}

async fn push_status(State(state): State<PushState>) -> Response {
    match state.registry.status() {
        Ok(devices) => Json(StatusResponse {
            count: devices.len(),
            devices,
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

fn parse_registration(body: &[u8]) -> Result<Registration, &'static str> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "request body must be valid JSON")?;
    let object = value
        .as_object()
        .ok_or("request body must be a JSON object")?;
    let device_token = required_string(object, "device_token")?;
    let bundle_id = required_string(object, "bundle_id")?;
    let environment = object
        .get("environment")
        .and_then(Value::as_str)
        .and_then(PushEnvironment::parse)
        .ok_or("environment must be development or production")?;
    let platform = object
        .get("platform")
        .and_then(Value::as_str)
        .and_then(PushPlatform::parse)
        .ok_or("platform must be ios")?;

    Ok(Registration {
        device_token,
        bundle_id,
        environment,
        platform,
    })
}

fn required_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, &'static str> {
    let Some(value) = object.get(field).and_then(Value::as_str) else {
        return Err(match field {
            "device_token" => "device_token is required",
            "bundle_id" => "bundle_id is required",
            _ => "required field is missing",
        });
    };
    if value.trim().is_empty() {
        return Err(match field {
            "device_token" => "device_token is required",
            "bundle_id" => "bundle_id is required",
            _ => "required field is missing",
        });
    }
    Ok(value.to_owned())
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

struct Registration {
    device_token: String,
    bundle_id: String,
    environment: PushEnvironment,
    platform: PushPlatform,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::api_router;

    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn basis(cid: &str) -> AccessBasis {
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(cid).expect("fixture CID"),
        }
    }

    fn root() -> TempDir {
        TempDir::new_in("/var/tmp").expect("journal root")
    }

    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: impl Into<Body>,
        basis: Option<AccessBasis>,
    ) -> (StatusCode, Value) {
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
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }

    fn valid_body(token: &str, bundle_id: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "device_token": token,
            "bundle_id": bundle_id,
            "environment": "development",
            "platform": "ios"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn register_persists_exact_values_and_replaces_by_identity_or_token() {
        let root = root();
        let app = api_router(root.path());
        let (status, body) = call(
            &app,
            "POST",
            "/api/push/register",
            valid_body(" Token AbCd ", " org.example.push "),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"registered": true}));
        let registry: Value = serde_json::from_slice(
            &fs::read(root.path().join("config/push-registry.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(registry["devices"][CID_A]["device_token"], " Token AbCd ");
        assert_eq!(
            registry["devices"][CID_A]["bundle_id"],
            " org.example.push "
        );
        assert!(!root.path().join("config/push_devices.json").exists());

        call(
            &app,
            "POST",
            "/api/push/register",
            valid_body("replacement", "org.example.replacement"),
            Some(basis(CID_A)),
        )
        .await;
        call(
            &app,
            "POST",
            "/api/push/register",
            valid_body("replacement", "org.example.stolen"),
            Some(basis(CID_B)),
        )
        .await;
        let registry: Value = serde_json::from_slice(
            &fs::read(root.path().join("config/push-registry.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(registry["devices"].as_object().unwrap().len(), 1);
        assert_eq!(registry["devices"][CID_A], Value::Null);
        assert_eq!(
            registry["devices"][CID_B]["bundle_id"],
            "org.example.stolen"
        );
    }

    #[tokio::test]
    async fn registration_validation_refuses_each_field_without_writing() {
        let root = root();
        let app = api_router(root.path());
        let invalid = [
            json!({"bundle_id":"org.example","environment":"development","platform":"ios"}),
            json!({"device_token":"  ","bundle_id":"org.example","environment":"development","platform":"ios"}),
            json!({"device_token":"token","environment":"development","platform":"ios"}),
            json!({"device_token":"token","bundle_id":"\t","environment":"development","platform":"ios"}),
            json!({"device_token":"token","bundle_id":"org.example","platform":"ios"}),
            json!({"device_token":"token","bundle_id":"org.example","environment":"staging","platform":"ios"}),
            json!({"device_token":"token","bundle_id":"org.example","environment":"development"}),
            json!({"device_token":"token","bundle_id":"org.example","environment":"development","platform":"android"}),
            json!([]),
        ];
        for body in invalid {
            let (status, response) = call(
                &app,
                "POST",
                "/api/push/register",
                serde_json::to_vec(&body).unwrap(),
                Some(basis(CID_A)),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(response["reason_code"], "push_request_invalid");
        }
        let (status, response) = call(
            &app,
            "POST",
            "/api/push/register",
            b"{".to_vec(),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["reason_code"], "push_request_invalid");
        assert!(!root.path().join("config/push-registry.json").exists());
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
    async fn deregister_status_and_test_have_their_documented_shapes() {
        let root = root();
        let app = api_router(root.path());
        let (status, body) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"count": 0, "devices": []}));
        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "feature_unavailable");

        call(
            &app,
            "POST",
            "/api/push/register",
            valid_body(" Token AbCd ", "org.example.push"),
            Some(basis(CID_A)),
        )
        .await;
        let (status, body) = call(&app, "GET", "/api/push/status", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["count"], 1);
        assert_eq!(body["devices"][0]["device_token"], "...bCd ");
        assert_eq!(
            body["devices"][0],
            json!({
                "bundle_id": "org.example.push",
                "environment": "development",
                "platform": "ios",
                "registered_at": body["devices"][0]["registered_at"],
                "device_token": "...bCd "
            })
        );
        let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"device_count": 1}));

        let (status, body) = call(
            &app,
            "DELETE",
            "/api/push/register",
            Body::empty(),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"removed": true}));
        let (status, body) = call(
            &app,
            "DELETE",
            "/api/push/register",
            Body::empty(),
            Some(basis(CID_A)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"removed": false}));
    }

    #[tokio::test]
    async fn registry_failures_are_never_reported_as_empty() {
        let root = root();
        fs::create_dir_all(root.path().join("config/push-registry.json")).unwrap();
        let app = api_router(root.path());
        for (method, uri, body, basis) in [
            (
                "POST",
                "/api/push/register",
                valid_body("token", "org.example"),
                Some(basis(CID_A)),
            ),
            (
                "DELETE",
                "/api/push/register",
                Vec::new(),
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
