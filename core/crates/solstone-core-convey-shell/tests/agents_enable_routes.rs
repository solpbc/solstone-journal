// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Map, Value, json};
use solstone_core_convey_shell::{
    SmeOperationsOverride, SmePoll, SmePollOutcome, SmeRuntimeOverride, router,
};
use solstone_core_thinking::confidential::OperationRegistry;
use tower::ServiceExt;

fn journal() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "solstone-agents-enable-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).expect("journal creates");
    fs::create_dir_all(path.join("config")).expect("config directory creates");
    fs::write(
        path.join("config/journal.json"),
        b"{\"setup\":{\"completed_at\":1}}\n",
    )
    .expect("config writes");
    path
}

async fn request(app: axum::Router, method: Method, path: &str, body: Body) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("request builds"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let parsed = serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "JSON body status={status} body={:?}: {error}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, parsed)
}

struct StaticPoll {
    outcome: SmePollOutcome,
    calls: AtomicUsize,
    polled_urls: Mutex<Vec<String>>,
    polled_nonces: Mutex<Vec<String>>,
}

impl StaticPoll {
    fn new(outcome: SmePollOutcome) -> Self {
        Self {
            outcome,
            calls: AtomicUsize::new(0),
            polled_urls: Mutex::new(Vec::new()),
            polled_nonces: Mutex::new(Vec::new()),
        }
    }
}

impl SmePoll for StaticPoll {
    fn poll(&self, base_url: &str, nonce: &str) -> SmePollOutcome {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.polled_urls.lock().unwrap().push(base_url.to_string());
        self.polled_nonces.lock().unwrap().push(nonce.to_string());
        self.outcome.clone()
    }
}

fn overridden(
    root: &Path,
    poll: Arc<StaticPoll>,
    registry: Arc<OperationRegistry>,
) -> axum::Router {
    router(root.to_path_buf())
        .layer(Extension(SmeOperationsOverride(registry)))
        .layer(Extension(SmeRuntimeOverride {
            portal_base_url: "https://services.solstone.app".to_string(),
            poll,
        }))
}

async fn wait_operation(app: axum::Router, expected_phase: &str) -> Value {
    for _ in 0..100 {
        let (_, body) = request(
            app.clone(),
            Method::GET,
            "/app/agents/api/enable",
            Body::empty(),
        )
        .await;
        let operation = &body["operation"];
        if operation["phase"] == expected_phase {
            return operation.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("operation did not reach {expected_phase}");
}

#[tokio::test]
async fn enable_flow_ac1_consent_url_and_crockford_nonce() {
    let root = journal();
    let registry = Arc::new(OperationRegistry::default());
    let poll = Arc::new(StaticPoll::new(SmePollOutcome::Continue));
    let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

    let (status, body) = request(
        app.clone(),
        Method::POST,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["success"], true);
    assert_eq!(body["service"], "sme");
    let portal_url = body["operation"]["portal_url"]
        .as_str()
        .expect("portal_url");
    assert!(portal_url.starts_with("https://services.solstone.app/enable/solstone-me?nonce="));
    assert!(portal_url.contains("&instance="));

    // Nonce must be a 52-char Crockford base32 string
    let query = portal_url.split('?').nth(1).expect("query params");
    let nonce_pair = query
        .split('&')
        .find(|p| p.starts_with("nonce="))
        .expect("nonce param");
    let nonce = &nonce_pair["nonce=".len()..];
    assert_eq!(nonce.len(), 52, "nonce must be 52 chars");
    for c in nonce.chars() {
        assert!(
            solstone_core_handoff_nonce::NONCE_ALPHABET.contains(&(c as u8)),
            "invalid Crockford base32 char: {c}"
        );
    }

    // Wait a tick and verify poll was called with base url and matching nonce
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(poll.calls.load(Ordering::Relaxed) >= 1);
    let polled_urls = poll.polled_urls.lock().unwrap();
    assert_eq!(polled_urls[0], "https://services.solstone.app");
    let polled_nonces = poll.polled_nonces.lock().unwrap();
    assert_eq!(polled_nonces[0], nonce);
}

#[tokio::test]
async fn enable_flow_ac2_needs_subscription_terminal_outcome() {
    let root = journal();
    let registry = Arc::new(OperationRegistry::default());
    let mut payload = Map::new();
    payload.insert("service".to_string(), json!("sme"));
    payload.insert("state".to_string(), json!("needs_subscription"));
    payload.insert(
        "subscribe_url".to_string(),
        json!("https://services.solstone.app/services/solstone-me"),
    );
    let poll = Arc::new(StaticPoll::new(SmePollOutcome::Success(payload)));
    let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

    let (status, body) = request(
        app.clone(),
        Method::POST,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["service"], "sme");

    let op = wait_operation(app, "needs_subscription").await;
    assert_eq!(op["phase"], "needs_subscription");
    let subscribe_url = op["subscribe_url"].as_str().expect("subscribe_url");
    assert!(subscribe_url.starts_with("https://"));

    // Verify capability was NOT enabled in journal.json
    let config: Value = serde_json::from_str(
        &fs::read_to_string(root.join("config/journal.json")).expect("journal.json"),
    )
    .expect("json");
    assert_ne!(
        config.get("mcp_endpoint").and_then(|m| m.get("enabled")),
        Some(&json!(true))
    );
}

#[tokio::test]
async fn enable_flow_ac3_approved_terminal_outcome() {
    let root = journal();
    let registry = Arc::new(OperationRegistry::default());
    let mut payload = Map::new();
    payload.insert("service".to_string(), json!("sme"));
    payload.insert("state".to_string(), json!("approved"));
    payload.insert("approved_at".to_string(), json!("2026-09-20T12:00:00Z"));
    let poll = Arc::new(StaticPoll::new(SmePollOutcome::Success(payload)));
    let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

    let (status, body) = request(
        app.clone(),
        Method::POST,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["service"], "sme");

    let op = wait_operation(app, "enabled").await;
    assert_eq!(op["phase"], "enabled");

    // Verify journal.json updated with mcp_endpoint.enabled = true
    let config: Value = serde_json::from_str(
        &fs::read_to_string(root.join("config/journal.json")).expect("journal.json"),
    )
    .expect("json");
    assert_eq!(config["mcp_endpoint"]["enabled"], true);
}

#[tokio::test]
async fn enable_flow_ac4_consent_link_expired_and_malformed_failures() {
    // Sub-case 1: consent_link_expired
    {
        let root = journal();
        let registry = Arc::new(OperationRegistry::default());
        let poll = Arc::new(StaticPoll::new(SmePollOutcome::Failed {
            token: "consent_link_expired".to_string(),
            detail: None,
        }));
        let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

        let (status, _) = request(
            app.clone(),
            Method::POST,
            "/app/agents/api/enable",
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);

        let op = wait_operation(app, "error").await;
        assert_eq!(op["phase"], "error");
        assert_eq!(op["retryable"], true);
        assert!(op["guidance"].is_string());
    }

    // Sub-case 2: malformed / unrecognized payload
    {
        let root = journal();
        let registry = Arc::new(OperationRegistry::default());
        let mut malformed = Map::new();
        malformed.insert("service".to_string(), json!("unknown_service"));
        malformed.insert("state".to_string(), json!("approved"));
        let poll = Arc::new(StaticPoll::new(SmePollOutcome::Success(malformed)));
        let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

        let (status, _) = request(
            app.clone(),
            Method::POST,
            "/app/agents/api/enable",
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);

        let op = wait_operation(app, "error").await;
        assert_eq!(op["phase"], "error");
        assert!(op["guidance"].is_string());
    }
}

#[tokio::test]
async fn enable_flow_revoked_terminal_outcome() {
    let root = journal();
    let registry = Arc::new(OperationRegistry::default());
    let mut payload = Map::new();
    payload.insert("service".to_string(), json!("sme"));
    payload.insert("state".to_string(), json!("revoked"));
    let poll = Arc::new(StaticPoll::new(SmePollOutcome::Success(payload)));
    let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

    let (status, _) = request(
        app.clone(),
        Method::POST,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let op = wait_operation(app, "revoked").await;
    assert_eq!(op["phase"], "revoked");
}

#[tokio::test]
async fn enable_flow_already_enabled_refusal() {
    let root = journal();
    fs::write(
        root.join("config/journal.json"),
        b"{\"setup\":{\"completed_at\":1},\"mcp_endpoint\":{\"enabled\":true}}\n",
    )
    .expect("config writes");
    let registry = Arc::new(OperationRegistry::default());
    let poll = Arc::new(StaticPoll::new(SmePollOutcome::Continue));
    let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

    let (status, body) = request(app, Method::POST, "/app/agents/api/enable", Body::empty()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["reason_code"], "invalid_operation_for_state");
}
