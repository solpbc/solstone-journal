// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_journal_config::materialized_defaults;
use tower::ServiceExt;

use crate::establish;
use crate::http::router;
use crate::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
use crate::mark::mark_from_jid;

const VALID_CID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const INIT_HTML: &str = include_str!("../assets/init.html");

#[tokio::test]
async fn init_routes_reject_linked_devices_and_serve_localhost() {
    let temporary = TempDir::new();
    let routes = [
        (Method::GET, "/init/api/state"),
        (Method::GET, "/init/api/local-capability"),
        (Method::GET, "/init"),
        (Method::GET, "/init/mark"),
        (Method::POST, "/init/mark/regenerate"),
        (Method::POST, "/init/mark/lock"),
        (Method::POST, "/init/finalize"),
    ];

    for (method, path) in routes {
        let linked = request(
            temporary.path(),
            AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
            },
            method.clone(),
            path,
            None,
        )
        .await;
        assert_eq!(linked.status(), StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(
            response_json(linked).await["reason_code"],
            "init_local_only"
        );

        let localhost = request(temporary.path(), AccessBasis::Localhost, method, path, None).await;
        assert_ne!(localhost.status(), StatusCode::FORBIDDEN, "{path}");
        assert_ne!(localhost.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn init_routes_are_registered_and_observers_remain_deferred() {
    let temporary = TempDir::new();
    let routes = [
        (Method::GET, "/init/api/state"),
        (Method::GET, "/init/api/local-capability"),
        (Method::GET, "/init"),
        (Method::GET, "/init/mark"),
        (Method::POST, "/init/mark/regenerate"),
        (Method::POST, "/init/mark/lock"),
        (Method::POST, "/init/finalize"),
    ];

    for (method, path) in routes {
        let response = request(temporary.path(), AccessBasis::Localhost, method, path, None).await;
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }

    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/observers",
        None,
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "/init/observers is deliberately not ported — it reads in-process SSE subscriber state \
         (convey/bridge.py _SSE_SUBSCRIBERS_BY_KEY) that a Rust convey with no SSE substrate has no \
         way to answer honestly; delete this assertion (not route around it) once Rust convey owns \
         the SSE subscriber set."
    );
}

#[tokio::test]
async fn devices_emit_exactly_the_protocol_fields_and_role() {
    let temporary = TempDir::new();
    let mut ledger = AuthorizationLedger::new(temporary.path());
    ledger
        .add(ClientEntry::new(
            "sha256:client",
            "phone",
            "2026-08-04T00:00:00Z",
            "journal-id",
            ClientRole::Peer,
        ))
        .unwrap();

    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/app/network/api/devices",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"devices":[{
            "fingerprint":"sha256:client",
            "device_label":"phone",
            "paired_at":"2026-08-04T00:00:00Z",
            "instance_id":"journal-id",
            "role":"peer"
        }]})
    );
}

#[tokio::test]
async fn devices_distinguish_missing_unreadable_and_malformed_ledgers() {
    let missing = TempDir::new();
    let response = request(
        missing.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/app/network/api/devices",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, json!({"devices":[]}));

    let unreadable = TempDir::new();
    fs::create_dir_all(
        unreadable
            .path()
            .join("link")
            .join("authorized_clients.json"),
    )
    .unwrap();
    let response = request(
        unreadable.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/app/network/api/devices",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(response).await["reason_code"],
        "authorization_ledger_unreadable"
    );

    let malformed = TempDir::new();
    fs::create_dir_all(malformed.path().join("link")).unwrap();
    fs::write(
        malformed
            .path()
            .join("link")
            .join("authorized_clients.json"),
        "{not json",
    )
    .unwrap();
    let response = request(
        malformed.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/app/network/api/devices",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(response).await["reason_code"],
        "authorization_ledger_malformed"
    );
}

#[tokio::test]
async fn identity_is_neutral_until_committed_then_returns_a_render_spec() {
    let temporary = TempDir::new();
    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/app/network/api/identity",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"committed":false,"instance_id":null,"mark":null})
    );

    establish::current_candidate(temporary.path()).unwrap();
    let committed = establish::lock_in(temporary.path(), None).unwrap();
    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/app/network/api/identity",
        None,
    )
    .await;
    let body = response_json(response).await;
    assert_eq!(body["committed"], true);
    assert_eq!(body["instance_id"], committed.instance_id);
    assert_mark_render_spec(&body["mark"]);
}

/// The mark an owner approves is the mark they get, across a regenerate.
///
/// `committed_mark_returns_the_locked_identity_mark` cannot catch a violation of
/// this: it derives its expected value from whatever `lock_in` committed, so it
/// passes for an implementation that mints a fresh CA at commit time. This test
/// takes its expected value from the last preview the owner saw instead, which is
/// the only oracle that fails when lock-in stops promoting the candidate.
///
/// Why it earns a permanent assertion: a wrong mark is not a cosmetic defect. The
/// mark exists so an owner can recognise their own journal, so a mark that changes
/// between approval and commit reads as evidence of compromise.
#[tokio::test]
async fn the_committed_mark_is_the_last_previewed_mark() {
    let temporary = TempDir::new();

    let first = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::GET,
            "/init/mark",
            None,
        )
        .await,
    )
    .await;

    let regenerated = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::POST,
            "/init/mark/regenerate",
            None,
        )
        .await,
    )
    .await;

    // Without this the equality at the end could be comparing one mark to itself
    // on an implementation that ignores regenerate entirely.
    assert_ne!(
        first["mark"], regenerated["mark"],
        "regenerate must replace the candidate, so the preview must change"
    );

    let previewed_again = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::GET,
            "/init/mark",
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        regenerated["mark"], previewed_again["mark"],
        "the candidate must survive between requests, not be minted per request"
    );

    let locked = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/lock",
        None,
    )
    .await;
    assert_eq!(locked.status(), StatusCode::OK);

    let committed = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::GET,
            "/init/mark",
            None,
        )
        .await,
    )
    .await;
    assert_eq!(committed["locked"], true);
    assert_eq!(
        committed["mark"], previewed_again["mark"],
        "the committed CA must be the previewed candidate, not a freshly minted one"
    );
}

#[tokio::test]
async fn mark_preview_returns_only_a_render_spec_and_lock_requires_a_candidate() {
    let temporary = TempDir::new();
    let no_candidate = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/lock",
        None,
    )
    .await;
    assert_eq!(no_candidate.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(no_candidate).await["reason_code"],
        "invalid_operation_for_state"
    );

    let preview = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/mark",
        None,
    )
    .await;
    let body = response_json(preview).await;
    assert_eq!(body["locked"], false);
    assert_mark_render_spec(&body["mark"]);
}

#[tokio::test]
async fn regenerate_is_blocked_when_identity_is_locked() {
    let temporary = committed_journal();
    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/regenerate",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        json!({
            "error":"Bad Request",
            "reason_code":"invalid_operation_for_state",
            "detail":"journal id already locked"
        })
    );
}

#[tokio::test]
async fn committed_mark_returns_the_locked_identity_mark() {
    let temporary = TempDir::new();
    establish::current_candidate(temporary.path()).unwrap();
    let committed = establish::lock_in(temporary.path(), None).unwrap();
    let expected_mark = serde_json::to_value(
        mark_from_jid(&committed.instance_id)
            .unwrap()
            .to_render_spec(),
    )
    .unwrap();

    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/mark",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"locked":true,"mark":expected_mark})
    );
}

#[tokio::test]
async fn init_state_reads_materialized_defaults_without_writing() {
    let temporary = TempDir::new();
    let state = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    let state = response_json(state).await;
    let defaults = materialized_defaults();
    assert_eq!(state["identity_name"], defaults["identity"]["name"]);
    assert_eq!(
        state["identity_preferred"],
        defaults["identity"]["preferred"]
    );
    assert_eq!(state["retention_mode"], "keep");
    assert_eq!(state["lanes"][0]["id"], "local");
    assert_eq!(
        state["confidential"]["lane_detail"]["heading"],
        "confidential processing"
    );
    assert!(
        !temporary
            .path()
            .join("config")
            .join("journal.json")
            .exists()
    );
}

#[tokio::test]
async fn init_state_lanes_and_confidential_match_pre_move_fixture() {
    let temporary = TempDir::new();
    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    let actual = response_json(response).await;
    let expected: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/init_state_lanes_pre_move.json"
    ))
    .expect("pre-move init-state fixture parses");
    assert_eq!(
        json!({
            "lanes": actual["lanes"].clone(),
            "confidential": actual["confidential"].clone(),
        }),
        expected
    );
}

#[tokio::test]
async fn init_reads_defaults_without_writing() {
    let temporary = TempDir::new();
    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), INIT_HTML.as_bytes());
    assert!(
        !temporary
            .path()
            .join("config")
            .join("journal.json")
            .exists()
    );
}

#[tokio::test]
async fn init_redirects_after_finalize() {
    let temporary = TempDir::new();

    establish::current_candidate(temporary.path()).unwrap();
    establish::lock_in(temporary.path(), None).unwrap();
    let finalize = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({"lane":"local"})),
    )
    .await;
    assert_eq!(finalize.status(), StatusCode::OK);
    let init = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init",
        None,
    )
    .await;
    assert_eq!(init.status(), StatusCode::FOUND);
    assert_eq!(init.headers()["location"], "/");
}

#[tokio::test]
async fn init_does_not_overwrite_corrupt_config() {
    let temporary = TempDir::new();
    let config_path = temporary.path().join("config").join("journal.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(&config_path, "{not json").unwrap();

    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response_json(response).await["reason_code"],
        "corrupt_config"
    );
    assert_eq!(fs::read_to_string(config_path).unwrap(), "{not json");
}

#[tokio::test]
async fn finalize_writes_journal_config_with_the_python_response_shape() {
    let temporary = TempDir::new();
    establish::current_candidate(temporary.path()).unwrap();
    establish::lock_in(temporary.path(), None).unwrap();
    let config_path = temporary.path().join("config").join("journal.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        json!({"convey":{"allow_network_access":true}}).to_string(),
    )
    .unwrap();

    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({
            "lane":"confidential",
            "retention_mode":"days",
            "retention_days":30,
            "name":"Ada",
            "preferred":"A",
            "timezone":"America/Denver"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"success":true,"redirect":"/app/thinking#confidential-setup","warnings":[]})
    );
    let config: Value = serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
    assert_eq!(config["identity"]["name"], "Ada");
    assert_eq!(config["identity"]["preferred"], "A");
    assert_eq!(config["identity"]["timezone"], "America/Denver");
    assert!(
        config["setup"]["completed_at"]
            .as_i64()
            .is_some_and(|value| value > 0)
    );
    assert_eq!(config["retention"]["raw_media"], "days");
    assert_eq!(config["retention"]["raw_media_days"], 30);
    assert!(config["convey"].get("allow_network_access").is_none());
}

#[tokio::test]
async fn finalize_does_not_write_convey_config() {
    let temporary = committed_journal();
    let convey_path = temporary.path().join("config").join("convey.json");
    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({})),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(!convey_path.exists());
}

#[tokio::test]
async fn finalize_preserves_non_object_sections_as_corrupt_config() {
    let temporary = committed_journal();
    let config_path = temporary.path().join("config").join("journal.json");
    let contents = br#"{"identity":"not-an-object"}"#;
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(&config_path, contents).unwrap();

    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({})),
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response_json(response).await["reason_code"],
        "corrupt_config"
    );
    assert_eq!(fs::read(config_path).unwrap(), contents);
}

#[tokio::test]
async fn finalize_treats_malformed_json_as_an_empty_request() {
    let temporary = committed_journal();
    let response = request_body(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Body::from("{not json"),
        Some("application/json"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"success":true,"redirect":"/app/thinking","warnings":[]})
    );
}

#[tokio::test]
async fn finalize_rejects_missing_identity_invalid_lane_and_invalid_retention() {
    let uncommitted = TempDir::new();
    let response = request(
        uncommitted.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["reason_code"],
        "identity_not_locked"
    );

    let committed = committed_journal();
    let invalid_lane = request(
        committed.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({"lane":"invalid"})),
    )
    .await;
    assert_eq!(invalid_lane.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(invalid_lane).await["detail"],
        "lane must be one of: byo, confidential, local"
    );

    let invalid_retention = request(
        committed.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({"retention_mode":"days","retention_days":0})),
    )
    .await;
    assert_eq!(invalid_retention.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(invalid_retention).await["detail"],
        "retention_days must be a positive integer"
    );
}

async fn request(
    journal: &Path,
    basis: AccessBasis,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    let (body, content_type) = if let Some(body) = body {
        (Body::from(body.to_string()), Some("application/json"))
    } else {
        (Body::empty(), None)
    };
    request_body(journal, basis, method, uri, body, content_type).await
}

async fn request_body(
    journal: &Path,
    basis: AccessBasis,
    method: Method,
    uri: &str,
    body: Body,
    content_type: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    router(journal)
        .layer(Extension(basis))
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn assert_mark_render_spec(mark: &Value) {
    assert_eq!(
        mark.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["icon1", "icon2", "words"]
    );
    for icon in ["icon1", "icon2"] {
        assert_eq!(
            mark[icon].as_object().unwrap().keys().collect::<Vec<_>>(),
            ["name", "svg", "color", "rot"]
        );
        assert_eq!(
            mark[icon]["color"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["name", "hex"]
        );
    }
    assert_eq!(mark["words"].as_array().unwrap().len(), 2);
}

fn committed_journal() -> TempDir {
    let temporary = TempDir::new();
    establish::current_candidate(temporary.path()).unwrap();
    establish::lock_in(temporary.path(), None).unwrap();
    temporary
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-sol-link-http-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
