// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_convey_shell::{
    NetworkOperationsOverride, SplDisableFailureOverride, SplEnrollment, SplPoll, SplPollOutcome,
    SplRuntimeOverride, router,
};
use solstone_core_sol_link::service_identity::ServiceIdentity;
use solstone_core_spl::EnrollError;
use solstone_core_thinking::confidential::OperationRegistry;
use tower::ServiceExt;

fn journal() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "solstone-network-writes-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).expect("journal creates");
    fs::create_dir_all(path.join("config")).expect("config directory creates");
    fs::write(
        path.join("config/journal.json"),
        b"{\"setup\":{\"completed_at\":1},\"link\":{\"posture\":\"direct\"}}\n",
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

async fn post(root: &Path, path: &str, body: impl Into<Body>) -> (StatusCode, Value) {
    request(router(root.to_path_buf()), Method::POST, path, body.into()).await
}

fn write_token(root: &Path) {
    let path = root.join("link/tokens/account.json");
    fs::create_dir_all(path.parent().expect("token parent")).expect("token parent creates");
    fs::write(path, br#"{"service_token":"token"}"#).expect("token writes");
}

fn operation_keys(value: &Value) {
    let object = value.as_object().expect("operation object");
    assert_eq!(object.len(), 7);
    for key in [
        "kind",
        "phase",
        "guidance",
        "retryable",
        "portal_url",
        "subscribe_url",
        "elapsed_ms",
    ] {
        assert!(object.contains_key(key), "operation has {key}");
    }
    assert!(object["elapsed_ms"].is_u64());
}

struct StaticPoll {
    outcome: SplPollOutcome,
    calls: AtomicUsize,
}
impl SplPoll for StaticPoll {
    fn poll(&self, _base_url: &str, _nonce: &str) -> SplPollOutcome {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.outcome.clone()
    }
}

enum Enrollment {
    Token,
    Error(u16, Option<&'static str>),
    Unreachable,
}
struct FakeEnrollment(Enrollment);
impl SplEnrollment for FakeEnrollment {
    fn enroll(
        &self,
        _journal: &std::path::Path,
        _identity: &ServiceIdentity,
        _ca_pubkey: &str,
    ) -> Result<String, EnrollError> {
        match &self.0 {
            Enrollment::Token => Ok("service-token".to_owned()),
            Enrollment::Error(status, reason) => Err(EnrollError::Rejected {
                status: *status,
                reason: reason.map(|value| (*value).to_owned()),
            }),
            Enrollment::Unreachable => Err(EnrollError::Unreachable("offline".to_owned())),
        }
    }
}

fn overridden(
    root: &Path,
    outcome: SplPollOutcome,
    enrollment: Enrollment,
) -> (axum::Router, Arc<StaticPoll>) {
    let poll = Arc::new(StaticPoll {
        outcome,
        calls: AtomicUsize::new(0),
    });
    let app = router(root.to_path_buf())
        .layer(Extension(NetworkOperationsOverride(Arc::new(
            OperationRegistry::default(),
        ))))
        .layer(Extension(SplRuntimeOverride {
            portal_base_url: "https://portal.test".to_owned(),
            poll: poll.clone(),
            enrollment: Arc::new(FakeEnrollment(enrollment)),
        }));
    (app, poll)
}

async fn wait_operation(app: axum::Router, expected_phase: &str) -> Value {
    for _ in 0..100 {
        let (_, body) = request(
            app.clone(),
            Method::GET,
            "/app/network/api/private-link",
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
async fn host_address_normalizes_status_and_clears_lenient_twins() {
    let root = journal();
    let (status, body) = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"10.0.0.2:07657"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok":true,"home_address":"10.0.0.2:7657"}));
    let (_, status_body) = request(
        router(root.clone()),
        Method::GET,
        "/app/network/api/status",
        Body::empty(),
    )
    .await;
    assert_eq!(status_body["home_address"], "10.0.0.2:7657");
    for body in [
        Body::empty(),
        Body::from("not-json"),
        Body::from("[]"),
        Body::from(r#"{"home_address":"  "}"#),
    ] {
        let _ = post(
            &root,
            "/app/network/host-address",
            Body::from(r#"{"home_address":"10.0.0.2:7657"}"#),
        )
        .await;
        let (status, body) = post(&root, "/app/network/host-address", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"ok":true,"home_address":null}));
    }
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn host_address_admits_the_journal_selected_direct_port() {
    let root = journal();
    let path = root.join("config/journal.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&path).expect("config reads")).unwrap();
    config
        .as_object_mut()
        .expect("object")
        .insert("pairing".to_owned(), json!({"direct_port": 9000}));
    fs::write(
        &path,
        serde_json::to_vec(&config).expect("config serializes"),
    )
    .expect("config writes");
    let (status, body) = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"10.0.0.2:9000"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok":true,"home_address":"10.0.0.2:9000"}));
    let (status, rejected) = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"10.0.0.2:7657"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["reason_code"], "invalid_config_value");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn host_address_refusals_and_identical_write_are_exact() {
    let root = journal();
    let (status, hostname) = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"home.example:7657"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, invalid) = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"10.0.0.2:7658"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(hostname["reason_code"], "invalid_config_value");
    assert_eq!(invalid["reason_code"], "invalid_config_value");
    assert_ne!(hostname["detail"], invalid["detail"]);
    let _ = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"10.0.0.2:7657"}"#),
    )
    .await;
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(root.join("config/journal.json"))
            .unwrap()
            .ino()
    };
    let _ = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"10.0.0.2:7657"}"#),
    )
    .await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(root.join("config/journal.json"))
                .unwrap()
                .ino(),
            inode
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn enable_refuses_only_fully_enabled_and_has_exact_acceptance_shape() {
    let root = journal();
    fs::write(
        root.join("config/journal.json"),
        b"{\"setup\":{\"completed_at\":1},\"link\":{\"posture\":\"spl\"}}\n",
    )
    .unwrap();
    let (app, _) = overridden(
        &root,
        SplPollOutcome::Success(
            json!({"service":"spl","state":"pending"})
                .as_object()
                .unwrap()
                .clone(),
        ),
        Enrollment::Token,
    );
    let (status, body) = request(
        app,
        Method::POST,
        "/app/network/private-link/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "inconsistent state can retry: {body:?}"
    );
    write_token(&root);
    let (status, body) = post(&root, "/app/network/private-link/enable", Body::empty()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["reason_code"], "invalid_operation_for_state");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn enable_busy_and_consent_preparation_failures_are_refusals() {
    let root = journal();
    let registry = Arc::new(OperationRegistry::default());
    registry.start_operation("spl", "spl_enable", None).unwrap();
    let (status, busy) = request(
        router(root.clone()).layer(Extension(NetworkOperationsOverride(registry))),
        Method::POST,
        "/app/network/private-link/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        busy,
        json!({
            "reason_code": "service_busy",
            "reason": "service_busy",
            "error": "The service operation is already running. Try again in a moment.",
            "detail": "operation already running",
        })
    );
    let isolated = journal();
    let pending = json!({"service":"spl","state":"pending"})
        .as_object()
        .expect("pending payload")
        .clone();
    let (isolated_app, _) = overridden(
        &isolated,
        SplPollOutcome::Success(pending),
        Enrollment::Token,
    );
    let (_, status_body) = request(
        isolated_app.clone(),
        Method::GET,
        "/app/network/api/private-link",
        Body::empty(),
    )
    .await;
    assert!(status_body["operation"].is_null());
    let (status, _) = request(
        isolated_app,
        Method::POST,
        "/app/network/private-link/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "router-scoped registry does not leak busy state"
    );
    let broken = journal();
    fs::write(broken.join("link"), b"not-a-directory").unwrap();
    let (status, failed) = post(&broken, "/app/network/private-link/enable", Body::empty()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failed["reason_code"], "service_operation_failed");
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(isolated);
    let _ = fs::remove_dir_all(broken);
}

#[tokio::test]
async fn enable_approved_writes_identity_posture_and_token() {
    let root = journal();
    let approved = json!({"service":"spl","state":"approved","approved_at":1})
        .as_object()
        .unwrap()
        .clone();
    let (app, poll) = overridden(&root, SplPollOutcome::Success(approved), Enrollment::Token);
    let (status, body) = request(
        app.clone(),
        Method::POST,
        "/app/network/private-link/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body.as_object().unwrap().len(), 3);
    for key in ["success", "service", "operation"] {
        assert!(body.get(key).is_some());
    }
    operation_keys(&body["operation"]);
    assert_eq!(body["operation"]["kind"], "spl_enable");
    let operation = wait_operation(app, "enabled").await;
    operation_keys(&operation);
    assert!(operation["portal_url"].is_null());
    assert!(root.join("link/state.json").is_file());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(root.join("config/journal.json")).unwrap())
            .unwrap()["link"]["posture"],
        "spl"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(root.join("link/tokens/account.json")).unwrap())
            .unwrap()["service_token"],
        "service-token"
    );
    assert!(poll.calls.load(Ordering::Relaxed) > 0);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn enable_consent_and_relay_outcomes_are_mapped_without_egress() {
    let cases = [
        (SplPollOutcome::Success(json!({"service":"spl","state":"revoked"}).as_object().unwrap().clone()), Enrollment::Token, "revoked", Some("Consent was not granted")),
        (SplPollOutcome::Success(json!({"service":"spl","state":"needs_subscription","subscribe_url":"https://subscribe.test"}).as_object().unwrap().clone()), Enrollment::Token, "needs_subscription", Some("private network needs")),
        (SplPollOutcome::Success(json!({"service":"spl","state":"approved","approved_at":1}).as_object().unwrap().clone()), Enrollment::Error(409, Some("ca_pubkey already registered to another instance")), "error", Some("different identity")),
        (SplPollOutcome::Success(json!({"service":"spl","state":"approved","approved_at":1}).as_object().unwrap().clone()), Enrollment::Error(409, Some("ca_pubkey mismatch — rotation not supported in v1")), "error", Some("security key changed")),
        (SplPollOutcome::Success(json!({"service":"spl","state":"approved","approved_at":1}).as_object().unwrap().clone()), Enrollment::Error(503, None), "error", Some("isn't available")),
        (SplPollOutcome::Success(json!({"service":"spl","state":"approved","approved_at":1}).as_object().unwrap().clone()), Enrollment::Unreachable, "error", Some("could not be reached")),
        (SplPollOutcome::Success(json!({"service":"spl","state":"approved","approved_at":1}).as_object().unwrap().clone()), Enrollment::Error(502, None), "error", Some("error 502")),
    ];
    for (poll_result, enrollment, phase, guidance) in cases {
        let root = journal();
        let (app, poll) = overridden(&root, poll_result, enrollment);
        let (status, _) = request(
            app.clone(),
            Method::POST,
            "/app/network/private-link/enable",
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let operation = wait_operation(app, phase).await;
        assert!(
            operation["guidance"]
                .as_str()
                .unwrap_or_default()
                .contains(guidance.unwrap())
        );
        if phase == "needs_subscription" {
            assert_eq!(operation["subscribe_url"], "https://subscribe.test");
        }
        assert!(poll.calls.load(Ordering::Relaxed) > 0);
        let _ = fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn pending_stays_open_and_terminal_grace_allows_a_fresh_enable() {
    let root = journal();
    let pending = json!({"service":"spl","state":"pending"})
        .as_object()
        .unwrap()
        .clone();
    let (app, _) = overridden(&root, SplPollOutcome::Success(pending), Enrollment::Token);
    let (_, started) = request(
        app.clone(),
        Method::POST,
        "/app/network/private-link/enable",
        Body::empty(),
    )
    .await;
    let pending = wait_operation(app, "waiting").await;
    operation_keys(&pending);
    let terminal_root = journal();
    let revoked = json!({"service":"spl","state":"revoked"})
        .as_object()
        .unwrap()
        .clone();
    let (terminal, _) = overridden(
        &terminal_root,
        SplPollOutcome::Success(revoked),
        Enrollment::Token,
    );
    let _ = request(
        terminal.clone(),
        Method::POST,
        "/app/network/private-link/enable",
        Body::empty(),
    )
    .await;
    let terminal_operation = wait_operation(terminal.clone(), "revoked").await;
    assert!(terminal_operation["portal_url"].is_null());
    let (status, next) = request(
        terminal,
        Method::POST,
        "/app/network/private-link/enable",
        Body::empty(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "terminal grace permits replacement: {next:?}"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(terminal_root);
    drop(started);
}

#[tokio::test]
async fn disable_shape_failure_and_get_operation_are_exact() {
    let root = journal();
    let (status, _) = post(
        &root,
        "/app/network/host-address",
        Body::from(r#"{"home_address":"10.0.0.2:7657"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let registry = Arc::new(OperationRegistry::default());
    registry
        .start_operation("spl", "spl_enable", Some("https://portal.test".to_owned()))
        .unwrap();
    let app = router(root.clone()).layer(Extension(NetworkOperationsOverride(registry.clone())));
    let (_, get) = request(
        app.clone(),
        Method::GET,
        "/app/network/api/private-link",
        Body::empty(),
    )
    .await;
    operation_keys(&get["operation"]);
    let (status, disabled) = request(
        app.clone(),
        Method::POST,
        "/app/network/private-link/disable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(disabled.as_object().unwrap().len(), 4);
    assert_eq!(disabled["status"].as_object().unwrap().len(), 7);
    assert!(disabled["status"].get("success").is_none());
    registry.clear_operation("spl");
    let (_, cleared) = request(
        app,
        Method::GET,
        "/app/network/api/private-link",
        Body::empty(),
    )
    .await;
    assert!(cleared["operation"].is_null());
    let failure_root = journal();
    let (status, failure) = request(
        router(failure_root.clone()).layer(Extension(SplDisableFailureOverride)),
        Method::POST,
        "/app/network/private-link/disable",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failure["detail"], "");
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(failure_root);
}

#[tokio::test]
async fn link_prefix_write_routes_are_registered() {
    let root = journal();
    for path in [
        "/app/link/host-address",
        "/app/link/private-link/enable",
        "/app/link/private-link/disable",
    ] {
        let (status, _) = post(&root, path, Body::empty()).await;
        assert_ne!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let _ = fs::remove_dir_all(root);
}

// ── forget a never-delivered device (G3-208) ─────────────────────────────────

fn write_forget_fixture(root: &Path) {
    fs::create_dir_all(root.join("link")).expect("link directory creates");
    fs::write(
        root.join("link/authorized_clients.json"),
        json!([
            {
                "fingerprint": "sha256:never",
                "device_label": "",
                "paired_at": "2026-07-02T18:23:01Z",
                "instance_id": "instance-never",
                "role": "",
                "network": "anywhere",
                "client_label": "SOL-WINBUILD",
                "kind": "cert",
            },
            {
                "fingerprint": "sha256:delivered",
                "device_label": "",
                "paired_at": "2026-06-28T23:17:06Z",
                "instance_id": "instance-delivered",
                "role": "",
                "network": "anywhere",
                "client_label": "iPhone's iPhone",
                "kind": "cert",
            },
            {
                "fingerprint": "sha256:thishost",
                "device_label": "",
                "paired_at": "2026-06-15T00:33:19Z",
                "instance_id": "instance-host",
                "role": "",
                "network": "network",
                "client_label": "Suze.local",
                "kind": "cert",
            },
            {
                "fingerprint": "sha256:sourceonly",
                "device_label": "",
                "paired_at": "2026-06-30T16:27:01Z",
                "instance_id": "instance-source",
                "role": "",
                "network": "anywhere",
                "client_label": "solstone glasses",
                "kind": "cert",
            }
        ])
        .to_string(),
    )
    .expect("authorization ledger writes");
    fs::write(
        root.join("link/devices.json"),
        json!({
            "sha256:never": {"last_seen_at": "2026-09-05T20:35:27Z"},
            "sha256:delivered": {
                "last_seen_at": "2026-09-06T15:17:10Z",
                "last_accepted_ingest_at": "2026-09-06T15:16:00Z",
                "last_accepted_segment": {"day": "20260906", "name": "151600_1"}
            },
            "sha256:thishost": {"last_seen_at": "2026-09-06T21:35:12Z"},
            "sha256:sourceonly": {
                "last_seen_at": "2026-09-06T10:00:00Z",
                "sources": {
                    "screen": {"last_accepted_ingest_at": "2026-09-06T09:00:00Z"}
                }
            }
        })
        .to_string(),
    )
    .expect("activity metadata writes");
}

fn forget_app(root: &Path) -> axum::Router {
    router(root.to_path_buf()).layer(Extension(solstone_core_convey_shell::HostLabelOverride(
        "suze".to_owned(),
    )))
}

async fn forget(root: &Path, fingerprint: &str) -> (StatusCode, Value) {
    request(
        forget_app(root),
        Method::POST,
        &format!("/app/network/api/devices/{}/forget", urlencode(fingerprint)),
        Body::empty(),
    )
    .await
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn ledger_fingerprints(root: &Path) -> Vec<String> {
    serde_json::from_slice::<Value>(
        &fs::read(root.join("link/authorized_clients.json")).expect("ledger reads"),
    )
    .expect("ledger parses")
    .as_array()
    .expect("ledger array")
    .iter()
    .map(|entry| entry["fingerprint"].as_str().unwrap_or_default().to_owned())
    .collect()
}

#[tokio::test]
async fn forget_removes_a_never_delivered_device_and_names_what_it_removed() {
    let root = journal();
    write_forget_fixture(&root);

    let (status, body) = forget(&root, "sha256:never").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["forgotten"]["fingerprint"], "sha256:never");
    assert_eq!(body["forgotten"]["display_label"], "SOL-WINBUILD");
    assert_eq!(
        ledger_fingerprints(&root),
        vec![
            "sha256:delivered".to_owned(),
            "sha256:thishost".to_owned(),
            "sha256:sourceonly".to_owned()
        ],
        "only the forgotten row leaves the ledger"
    );
}

#[tokio::test]
async fn forget_refuses_a_device_that_has_ever_delivered_material() {
    let root = journal();
    write_forget_fixture(&root);

    let (status, body) = forget(&root, "sha256:delivered").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason_code"], "device_has_delivered");
    assert!(
        ledger_fingerprints(&root).contains(&"sha256:delivered".to_owned()),
        "a refused forget leaves the ledger alone"
    );
}

#[tokio::test]
async fn forget_refuses_a_device_whose_only_delivery_is_recorded_per_source() {
    let root = journal();
    write_forget_fixture(&root);

    let (status, body) = forget(&root, "sha256:sourceonly").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason_code"], "device_has_delivered");
    assert!(ledger_fingerprints(&root).contains(&"sha256:sourceonly".to_owned()));
}

#[tokio::test]
async fn forget_refuses_the_computer_this_journal_runs_on() {
    let root = journal();
    write_forget_fixture(&root);

    let (status, body) = forget(&root, "sha256:thishost").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason_code"], "device_is_this_host");
    assert!(
        ledger_fingerprints(&root).contains(&"sha256:thishost".to_owned()),
        "the journal's own host is never removed by this route"
    );
}

#[tokio::test]
async fn forget_refuses_a_fingerprint_that_is_not_paired() {
    let root = journal();
    write_forget_fixture(&root);

    let (status, body) = forget(&root, "sha256:absent").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["reason_code"], "paired_device_not_found");
    assert_eq!(ledger_fingerprints(&root).len(), 4);
}

#[tokio::test]
async fn forget_refuses_when_the_delivery_record_cannot_be_read() {
    let root = journal();
    write_forget_fixture(&root);
    fs::write(root.join("link/devices.json"), b"{not json").expect("devices file rewrites");

    let (status, body) = forget(&root, "sha256:never").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["reason_code"], "device_activity_unavailable");
    assert_eq!(ledger_fingerprints(&root).len(), 4);
}
