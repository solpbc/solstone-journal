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

fn plant_committed_identity(root: &Path) -> String {
    let ca = solstone_core_sol_link::ca::generate_ca().expect("ca");
    let cert_pem = ca.certificate_pem();
    let key_pem = ca.private_key_pem();
    let instance_id = solstone_core_sol_link::ca::jid_from_spki(ca.spki_der()).expect("jid");

    let ca_dir = root.join("link/ca");
    fs::create_dir_all(&ca_dir).expect("ca dir");
    fs::write(ca_dir.join("cert.pem"), cert_pem).expect("cert");
    fs::write(ca_dir.join("private.pem"), key_pem).expect("key");
    let state_path = root.join("link/state.json");
    fs::write(
        state_path,
        format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
    )
    .expect("state");
    instance_id
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
    outcome: Mutex<SmePollOutcome>,
    calls: AtomicUsize,
    polled_urls: Mutex<Vec<String>>,
    polled_nonces: Mutex<Vec<String>>,
}

impl StaticPoll {
    fn new(outcome: SmePollOutcome) -> Self {
        Self {
            outcome: Mutex::new(outcome),
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
        self.outcome.lock().unwrap().clone()
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
async fn enable_flow_opens_the_solstone_me_consent_link_and_polls_its_nonce() {
    let root = journal();
    let instance_id = plant_committed_identity(&root);
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
    let (path, query) = portal_url
        .strip_prefix("https://services.solstone.app")
        .expect("prefix")
        .split_once('?')
        .expect("query params");
    assert_eq!(path, "/enable/solstone-me");

    let params: std::collections::BTreeMap<_, _> = query
        .split('&')
        .map(|p| p.split_once('=').expect("pair"))
        .collect();
    assert_eq!(params.len(), 4, "A URL with only nonce and instance fails");
    assert_eq!(params["instance"], instance_id);
    let nonce = params["nonce"];
    assert_eq!(nonce.len(), 52, "nonce must be 52 chars");
    for c in nonce.chars() {
        assert!(
            solstone_core_handoff_nonce::NONCE_ALPHABET.contains(&(c as u8)),
            "invalid Crockford base32 char: {c}"
        );
    }

    let compact = solstone_core_sol_link::home_reach::percent_decode(params["assertion"]).unwrap();
    let _ca_pubkey =
        solstone_core_sol_link::home_reach::percent_decode(params["ca_pubkey"]).unwrap();
    #[cfg(feature = "full-tests")]
    assert!(
        solstone_core_sol_link::home_reach::verify_service_enable_compact(&_ca_pubkey, &compact)
    );

    let claims =
        solstone_core_sol_link::home_reach::decode_service_enable_claims(&compact).unwrap();
    assert_eq!(claims["iss"], format!("home:{instance_id}"));
    assert_eq!(claims["aud"], "solstone-reach");
    assert_eq!(claims["scope"], "services.enable");
    assert_eq!(claims["instance_id"], instance_id);
    assert_eq!(claims["nonce"], nonce);
    assert_eq!(claims["service"], "sme");
    let iat = claims["iat"].as_i64().expect("iat integer");
    let exp = claims["exp"].as_i64().expect("exp integer");
    assert_eq!(exp - iat, 1800);

    // Wait a tick and verify poll was called with base url and matching nonce
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(poll.calls.load(Ordering::Relaxed) >= 1);
    let polled_urls = poll.polled_urls.lock().unwrap();
    assert_eq!(polled_urls[0], "https://services.solstone.app");
    let polled_nonces = poll.polled_nonces.lock().unwrap();
    assert_eq!(polled_nonces[0], nonce);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn enable_flow_needs_subscription_is_terminal_and_leaves_the_capability_off() {
    let root = journal();
    plant_committed_identity(&root);
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
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn enable_flow_approved_turns_the_capability_on() {
    let root = journal();
    plant_committed_identity(&root);
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
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn enable_flow_expired_and_malformed_handoffs_end_with_a_named_error() {
    // Sub-case 1: consent_link_expired
    {
        let root = journal();
        plant_committed_identity(&root);
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
        let _ = fs::remove_dir_all(root);
    }

    // Sub-case 2: malformed / unrecognized payload
    {
        let root = journal();
        plant_committed_identity(&root);
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
        let _ = fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn enable_flow_revoked_terminal_outcome() {
    let root = journal();
    plant_committed_identity(&root);
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
    let _ = fs::remove_dir_all(root);
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
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn enable_refuses_when_identity_missing_or_corrupt_or_fault() {
    async fn assert_refusal(root: &Path) {
        let registry = Arc::new(OperationRegistry::default());
        let poll = Arc::new(StaticPoll::new(SmePollOutcome::Continue));
        let app = overridden(root, Arc::clone(&poll), Arc::clone(&registry));
        let (status, body) = request(
            app.clone(),
            Method::POST,
            "/app/agents/api/enable",
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "service_operation_failed");

        let (_, status_body) =
            request(app, Method::GET, "/app/agents/api/enable", Body::empty()).await;
        assert!(
            status_body["operation"].is_null() || status_body["operation"]["portal_url"].is_null()
        );
        assert_eq!(poll.calls.load(Ordering::Relaxed), 0);
    }

    // Missing CA
    let missing_ca = journal();
    assert_refusal(&missing_ca).await;
    let _ = fs::remove_dir_all(missing_ca);

    // Corrupt CA
    let corrupt_ca = journal();
    plant_committed_identity(&corrupt_ca);
    fs::write(corrupt_ca.join("link/ca/cert.pem"), b"not a cert").unwrap();
    assert_refusal(&corrupt_ca).await;
    let _ = fs::remove_dir_all(corrupt_ca);

    // Missing state
    let missing_state = journal();
    plant_committed_identity(&missing_state);
    fs::remove_file(missing_state.join("link/state.json")).unwrap();
    assert_refusal(&missing_state).await;
    let _ = fs::remove_dir_all(missing_state);

    // Mismatched state
    let mismatched = journal();
    plant_committed_identity(&mismatched);
    fs::write(
        mismatched.join("link/state.json"),
        b"{\"instance_id\":\"wrong-instance-id\",\"home_label\":\"Test\"}",
    )
    .unwrap();
    assert_refusal(&mismatched).await;
    let _ = fs::remove_dir_all(mismatched);

    #[cfg(feature = "full-tests")]
    {
        use solstone_core_sol_link::home_reach::{HomeReachFaultGuard, HomeReachFaultPrimitive};
        for primitive in [
            HomeReachFaultPrimitive::HeaderJsonSerialization,
            HomeReachFaultPrimitive::ClaimsJsonSerialization,
            HomeReachFaultPrimitive::SigningKeyLoad,
            HomeReachFaultPrimitive::EcdsaSign,
        ] {
            let fault_root = journal();
            plant_committed_identity(&fault_root);
            let registry = Arc::new(OperationRegistry::default());
            let poll = Arc::new(StaticPoll::new(SmePollOutcome::Continue));
            let app = overridden(&fault_root, Arc::clone(&poll), Arc::clone(&registry));
            let guard = HomeReachFaultGuard::install(primitive);
            let (status, body) = request(
                app.clone(),
                Method::POST,
                "/app/agents/api/enable",
                Body::empty(),
            )
            .await;
            assert!(guard.was_consumed());
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body["reason_code"], "service_operation_failed");

            let (_, status_body) =
                request(app, Method::GET, "/app/agents/api/enable", Body::empty()).await;
            assert!(
                status_body["operation"].is_null()
                    || status_body["operation"]["portal_url"].is_null()
            );
            assert_eq!(poll.calls.load(Ordering::Relaxed), 0);
            let _ = fs::remove_dir_all(fault_root);
        }
    }
}

#[tokio::test]
async fn enable_busy_keeps_original_signed_portal_url_until_terminal() {
    let root = journal();
    let instance_id = plant_committed_identity(&root);
    let registry = Arc::new(OperationRegistry::default());
    let poll = Arc::new(StaticPoll::new(SmePollOutcome::Continue));
    let app = overridden(&root, Arc::clone(&poll), Arc::clone(&registry));

    // 1. First accepted enable stores a signed URL
    let (status, body) = request(
        app.clone(),
        Method::POST,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let first_url = body["operation"]["portal_url"].as_str().unwrap().to_owned();

    // 2. Second start while that operation is open returns service_busy refusal
    let (busy_status, busy_body) = request(
        app.clone(),
        Method::POST,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(busy_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy_body["reason_code"], "service_busy");

    // Read-back returns that same URL string
    let (_, status_body) = request(
        app.clone(),
        Method::GET,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(
        status_body["operation"]["portal_url"].as_str().unwrap(),
        first_url
    );

    // 3. After operation ends, next assertion and nonce are new
    let mut revoked_payload = Map::new();
    revoked_payload.insert("service".to_string(), json!("sme"));
    revoked_payload.insert("state".to_string(), json!("revoked"));
    *poll.outcome.lock().unwrap() = SmePollOutcome::Success(revoked_payload);
    let op = wait_operation(app.clone(), "revoked").await;
    assert!(op["portal_url"].is_null());

    let (next_status, next_body) = request(
        app.clone(),
        Method::POST,
        "/app/agents/api/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(next_status, StatusCode::ACCEPTED);
    let next_url = next_body["operation"]["portal_url"].as_str().unwrap();
    assert_ne!(next_url, first_url);

    let first_query = first_url.split_once('?').unwrap().1;
    let first_params: std::collections::BTreeMap<_, _> = first_query
        .split('&')
        .map(|p| p.split_once('=').unwrap())
        .collect();
    let next_query = next_url.split_once('?').unwrap().1;
    let next_params: std::collections::BTreeMap<_, _> = next_query
        .split('&')
        .map(|p| p.split_once('=').unwrap())
        .collect();
    assert_ne!(first_params["nonce"], next_params["nonce"]);
    assert_ne!(first_params["assertion"], next_params["assertion"]);
    assert_eq!(first_params["instance"], instance_id);
    assert_eq!(next_params["instance"], instance_id);

    let _ = fs::remove_dir_all(root);
}
