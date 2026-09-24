// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "0123456789abcdef";
const PUSH_KEY: &str = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio";

fn established_journal() -> tempfile::TempDir {
    let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
    fs::create_dir_all(journal.path().join("config")).expect("config directory");
    fs::write(
        journal.path().join("config/journal.json"),
        br#"{"setup":{"completed_at":1767225600}}"#,
    )
    .expect("journal config");
    journal
}

fn basis() -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(CID_A).expect("fixture CID"),
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
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    (status, body.to_vec())
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: impl Into<Body>,
    basis: Option<AccessBasis>,
) -> (StatusCode, Value) {
    let (status, bytes) = call_raw(app, method, uri, body, basis).await;
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

fn registration() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "platform": "ios",
        "device_token": TOKEN,
        "bundle_id": "org.example.push",
        "environment": "development",
        "push_key": PUSH_KEY
    }))
    .expect("registration JSON")
}

fn deregistration() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "platform": "ios",
        "device_token": TOKEN
    }))
    .expect("deregistration JSON")
}

#[tokio::test]
async fn all_push_routes_resolve_through_the_composed_shell_router() {
    let journal = established_journal();
    let app = router(journal.path().to_path_buf());

    let (status, body) = call(
        &app,
        "POST",
        "/api/push/register",
        registration(),
        Some(basis()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["platform"], "ios");
    assert_eq!(body["target"], "...cdef");
    assert_eq!(body["environment"], "development");

    let (status, body) = call(
        &app,
        "GET",
        "/api/push/status",
        Body::empty(),
        Some(basis()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["target"], "...cdef");
    assert!(body["cursor"].is_null());

    let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), Some(basis())).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["reason_code"], "feature_unavailable");
    assert_eq!(body["detail"], "no devices to reach");

    let (status, body_bytes) = call_raw(
        &app,
        "DELETE",
        "/api/push/register",
        deregistration(),
        Some(basis()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body_bytes.is_empty());
}

#[tokio::test]
async fn malformed_ledger_with_recipient_returns_push_ledger_unavailable() {
    let journal = established_journal();
    let app = router(journal.path().to_path_buf());

    let (status, _) = call(
        &app,
        "POST",
        "/api/push/register",
        registration(),
        Some(basis()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    fs::create_dir_all(journal.path().join("link")).expect("link directory");
    fs::write(journal.path().join("link/authorized_clients.json"), b"{}")
        .expect("malformed ledger");

    let (status, body) = call(&app, "POST", "/api/push/test", Body::empty(), Some(basis())).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["reason_code"], "push_ledger_unavailable");
}

#[tokio::test]
async fn identityless_mutations_do_not_touch_registry_through_shell_router() {
    let journal = established_journal();
    let app = router(journal.path().to_path_buf());
    let registry_path = journal.path().join("config/push-registry.json");

    let (status, body) = call(
        &app,
        "POST",
        "/api/push/register",
        b"not JSON".to_vec(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["reason_code"], "linked_device_required");

    let (status, body) = call(&app, "DELETE", "/api/push/register", Body::empty(), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["reason_code"], "linked_device_required");
    assert!(!registry_path.exists());
}
