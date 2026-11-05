// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_journal_config::materialized_defaults;
use solstone_core_thinking_copy::SetupCatalogFacts;
use tower::ServiceExt;

use crate::establish;
use crate::http::{
    SetupDownloadTask, SetupDownloads, SetupHost, SetupProgressError, SetupRetryError,
    init_router_with_host, router,
};
use crate::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
use crate::mark::mark_from_jid;

const VALID_CID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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
        (Method::POST, "/init/api/action"),
        (Method::POST, "/init/finalize"),
    ];

    for (method, path) in routes {
        let linked = request(
            temporary.path(),
            AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
                leaf_spki: vec![0x30, 0x00],
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
        (Method::POST, "/init/api/action"),
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
            ClientRole::from_wire(Some("observer")),
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
            "role":"observer"
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

    let duplicate = TempDir::new();
    fs::create_dir_all(duplicate.path().join("link")).unwrap();
    fs::write(
        duplicate
            .path()
            .join("link")
            .join("authorized_clients.json"),
        br#"[{"fingerprint":"a","device_label":"one","paired_at":"1","instance_id":"i"},{"fingerprint":"a","device_label":"two","paired_at":"2","instance_id":"i"}]"#,
    )
    .unwrap();
    let response = request(
        duplicate.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/app/network/api/devices",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(response).await["reason_code"],
        "authorization_ledger_duplicate_cid"
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
    let committed = establish::lock_in(temporary.path()).unwrap();
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

    let proposal_id = regenerated["proposal_id"]
        .as_str()
        .expect("proposal_id")
        .to_string();
    let locked = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/lock",
        Some(json!({"proposal_id": proposal_id})),
    )
    .await;
    assert_eq!(locked.status(), StatusCode::OK);
    let locked_json = response_json(locked).await;
    assert_eq!(locked_json["proposal_id"], proposal_id);
    assert_eq!(locked_json["mark"], previewed_again["mark"]);

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
    let state_resp = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    assert_eq!(state_resp.status(), StatusCode::OK);
    assert!(!temporary.path().join("link").join("ca-staging").exists());

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
        "invalid_request"
    );

    let stale = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/lock",
        Some(json!({"proposal_id": "stale"})),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(stale).await["reason_code"],
        "proposal_conflict"
    );
    assert!(!temporary.path().join("link").join("ca").exists());

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
    assert_eq!(body["availability"], "candidate");
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
    let committed = establish::lock_in(temporary.path()).unwrap();
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
        json!({"availability":"ready","locked":true,"mark":expected_mark,"proposal_id":committed.instance_id})
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
async fn init_state_transports_shared_lanes_and_keeps_confidential_fixture() {
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
    let mut expected: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/init_state_lanes_pre_move.json"
    ))
    .expect("pre-move init-state fixture parses");
    // Shared lane words changed after this snapshot. The unchanged
    // confidential detail remains pinned to the historical fixture.
    expected["lanes"] =
        serde_json::to_value(solstone_core_thinking_copy::LANES).expect("shared lanes serialize");
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
    let expected = crate::http::injected_init_html();
    assert_eq!(body.as_ref(), expected.as_bytes());
    let html = std::str::from_utf8(&body).unwrap();
    let bootstrap = html
        .split_once("<script type=\"application/json\" id=\"setup-copy-bootstrap\">")
        .unwrap()
        .1
        .split_once("</script>")
        .unwrap()
        .0;
    let copy: Value = serde_json::from_str(bootstrap).unwrap();
    assert_eq!(
        copy,
        solstone_core_thinking_copy::setup_copy(&solstone_core_thinking_copy::current_setup_facts())
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
async fn init_redirects_after_finalize() {
    let temporary = TempDir::new();

    establish::current_candidate(temporary.path()).unwrap();
    establish::lock_in(temporary.path()).unwrap();
    let finalize = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({"lane":"later"})),
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
    establish::lock_in(temporary.path()).unwrap();
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
        json!({"success":true,"redirect":"/app/home","warnings":[]})
    );
    let config: Value = serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
    assert_eq!(config["identity"]["name"], "Ada");
    assert_eq!(config["identity"]["preferred"], "A");
    let principal = solstone_core_entity::read_journal_principal(temporary.path())
        .unwrap()
        .expect("setup creates the owner Person");
    assert_eq!(principal["name"], "A");
    assert_eq!(principal["aka"], json!(["Ada"]));
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
    let body = response_json(response).await;
    assert_eq!(body["redirect"], "/app/home");
    let config: Value = serde_json::from_slice(
        &fs::read(temporary.path().join("config").join("journal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["setup"]["thinking_choice"], "later");
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
async fn finalize_rejects_malformed_json() {
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

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["reason_code"],
        "invalid_request"
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

async fn request_with_host(
    journal: &Path,
    host: Arc<dyn SetupHost>,
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
    request_body_with_host(journal, host, basis, method, uri, body, content_type).await
}

async fn request_body_with_host(
    journal: &Path,
    host: Arc<dyn SetupHost>,
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
    init_router_with_host(journal, host)
        .layer(Extension(basis))
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

#[derive(Default)]
struct FakeSetupHost {
    start_calls: std::sync::Mutex<Vec<PathBuf>>,
    cancel_calls: std::sync::Mutex<Vec<(PathBuf, String)>>,
    retry_speech_calls: std::sync::Mutex<Vec<PathBuf>>,
    retry_speech_error: std::sync::Mutex<Option<SetupRetryError>>,
    start_error: std::sync::Mutex<Option<String>>,
    projected: std::sync::Mutex<Option<SetupDownloads>>,
    project_error: std::sync::Mutex<Option<SetupProgressError>>,
    thinking_bytes: std::sync::Mutex<Option<Option<u64>>>,
    /// How many downloads had started when each thinking check was asked for.
    brain_refreshes: std::sync::Mutex<Vec<usize>>,
}

impl SetupHost for FakeSetupHost {
    fn refresh_brain(&self, _journal_root: &Path) {
        let started = self.start_calls.lock().unwrap().len();
        self.brain_refreshes.lock().unwrap().push(started);
    }

    fn start_local(&self, journal_root: &Path) -> Result<(), String> {
        self.start_calls
            .lock()
            .unwrap()
            .push(journal_root.to_path_buf());
        if let Some(err) = self.start_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        Ok(())
    }

    fn cancel_local(&self, journal_root: &Path, attempt_id: &str) -> Result<(), String> {
        self.cancel_calls
            .lock()
            .unwrap()
            .push((journal_root.to_path_buf(), attempt_id.to_string()));
        Ok(())
    }

    fn retry_speech(&self, journal_root: &Path) -> Result<(), SetupRetryError> {
        self.retry_speech_calls
            .lock()
            .unwrap()
            .push(journal_root.to_path_buf());
        if let Some(err) = self.retry_speech_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        Ok(())
    }

    fn project_downloads(
        &self,
        _journal_root: &Path,
    ) -> Result<SetupDownloads, SetupProgressError> {
        if let Some(err) = self.project_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        if let Some(p) = self.projected.lock().unwrap().clone() {
            return Ok(p);
        }
        Ok(SetupDownloads {
            speech: SetupDownloadTask {
                state: "idle".into(),
                attempt_id: None,
                bytes_done: 0,
                bytes_total: None,
                error: None,
            },
            thinking: SetupDownloadTask {
                state: "idle".into(),
                attempt_id: None,
                bytes_done: 0,
                bytes_total: None,
                error: None,
            },
        })
    }

    fn thinking_download_bytes(&self, _journal: &Path) -> Option<u64> {
        if let Some(b) = *self.thinking_bytes.lock().unwrap() {
            return b;
        }
        self.catalog_facts().thinking_download_bytes
    }

    fn catalog_facts(&self) -> SetupCatalogFacts {
        solstone_core_thinking_copy::current_setup_facts()
    }
}

#[tokio::test]
async fn action_route_rejects_linked_device() {
    let temporary = TempDir::new();
    let linked = request(
        temporary.path(),
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
            leaf_spki: vec![0x30, 0x00],
        },
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "save_thinking", "choice": "later"})),
    )
    .await;
    assert_eq!(linked.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response_json(linked).await["reason_code"],
        "init_local_only"
    );
}

#[tokio::test]
async fn fresh_get_init_api_state_reports_uninitialized() {
    let temporary = TempDir::new();
    let res = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = response_json(res).await;
    assert_eq!(body["mark"]["state"], "unavailable");
    assert!(body["mark"]["render"].is_null());
    assert!(body["mark"]["proposal_id"].is_null());
    assert_eq!(body["thinking"]["saved"], false);
    assert_eq!(
        body["remaining_required_steps"],
        json!(["mark", "thinking"])
    );
    assert_eq!(body["done"], false);
    assert_eq!(body["redirect"], "/app/home");
    assert!(!temporary.path().join("link").join("ca").exists());
    assert!(!temporary.path().join("link").join("ca-staging").exists());
}

#[tokio::test]
async fn regenerate_mark_and_commit_proposal_locks_and_is_idempotent() {
    let temporary = TempDir::new();
    let regen = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::POST,
            "/init/api/action",
            Some(json!({"action": "regenerate_mark"})),
        )
        .await,
    )
    .await;
    let proposal_id = regen["mark"]["proposal_id"]
        .as_str()
        .expect("proposal_id")
        .to_string();
    let expected_render =
        serde_json::to_value(mark_from_jid(&proposal_id).unwrap().to_render_spec()).unwrap();
    assert_eq!(regen["mark"]["render"], expected_render);

    // Missing proposal_id
    let missing = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "commit_mark"})),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(missing).await["reason_code"],
        "proposal_required"
    );
    assert!(!temporary.path().join("link").join("ca").exists());

    // Commit with proposal_id
    let committed = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::POST,
            "/init/api/action",
            Some(json!({"action": "commit_mark", "proposal_id": proposal_id})),
        )
        .await,
    )
    .await;
    assert_eq!(committed["mark"]["state"], "locked");
    assert_eq!(committed["mark"]["render"], expected_render);

    // Repeat commit_mark
    let repeat = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "commit_mark", "proposal_id": proposal_id})),
    )
    .await;
    assert_eq!(repeat.status(), StatusCode::OK);

    // Repeat POST /init/mark/lock
    let repeat_lock = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/lock",
        Some(json!({"proposal_id": proposal_id})),
    )
    .await;
    assert_eq!(repeat_lock.status(), StatusCode::OK);
}

#[tokio::test]
async fn regenerate_again_after_commit_is_invalid_operation() {
    let temporary = committed_journal();
    let resp = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "regenerate_mark"})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(resp).await["reason_code"],
        "invalid_operation_for_state"
    );
    assert!(!temporary.path().join("link").join("ca-staging").exists());
}

#[tokio::test]
async fn cross_client_regenerate_and_commit_conflict() {
    let temporary = TempDir::new();
    let regen_a = response_json(
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
    let id_a = regen_a["proposal_id"].as_str().unwrap().to_string();

    let regen_b = response_json(
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
    let id_b = regen_b["proposal_id"].as_str().unwrap().to_string();
    assert_ne!(id_a, id_b);

    // Commit A fails with 409
    let commit_a = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/lock",
        Some(json!({"proposal_id": id_a})),
    )
    .await;
    assert_eq!(commit_a.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(commit_a).await["reason_code"],
        "proposal_conflict"
    );
    assert!(!temporary.path().join("link").join("ca").exists());

    // Commit B succeeds
    let commit_b = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/mark/lock",
        Some(json!({"proposal_id": id_b})),
    )
    .await;
    assert_eq!(commit_b.status(), StatusCode::OK);
    let state = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::GET,
            "/init/api/state",
            None,
        )
        .await,
    )
    .await;
    assert_eq!(state["mark"]["state"], "locked");
    assert_eq!(state["mark"]["proposal_id"], id_b);
}

#[tokio::test]
async fn save_thinking_choices_persist_without_active_provider() {
    for choice in ["byo", "spp", "later"] {
        let temporary = committed_journal();
        let res = response_json(
            request(
                temporary.path(),
                AccessBasis::Localhost,
                Method::POST,
                "/init/api/action",
                Some(json!({"action": "save_thinking", "choice": choice})),
            )
            .await,
        )
        .await;
        assert_eq!(res["thinking"]["choice"], choice);
        assert_eq!(res["thinking"]["saved"], true);
        assert_eq!(res["redirect"], "/app/home");
        let finalized = response_json(
            request(
                temporary.path(),
                AccessBasis::Localhost,
                Method::POST,
                "/init/api/action",
                Some(json!({"action": "finalize"})),
            )
            .await,
        )
        .await;
        assert_eq!(finalized["done"], true);
        assert_eq!(finalized["redirect"], "/app/home");

        let config: Value = serde_json::from_slice(
            &fs::read(temporary.path().join("config").join("journal.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["setup"]["thinking_choice"], choice);
        assert!(config["providers"].get("active").is_none());
        assert!(
            config
                .get("services")
                .and_then(|s| s.get("confidential"))
                .is_none()
        );
    }
}

#[tokio::test]
async fn save_thinking_local_respects_capability() {
    for capability in ["ok", "blocked", "unknown"] {
        let temporary = TempDir::new();
        crate::http::setup_probe_test::capability(temporary.path(), capability);
        let host = Arc::new(FakeSetupHost::default());
        let response = request_with_host(
            temporary.path(),
            host.clone(),
            AccessBasis::Localhost,
            Method::POST,
            "/init/api/action",
            Some(json!({"action":"save_thinking", "choice":"local"})),
        )
        .await;
        if capability == "ok" {
            assert_eq!(response.status(), StatusCode::OK);
            let state = response_json(response).await;
            assert_eq!(state["thinking"]["choice"], "local");
            assert_eq!(state["thinking"]["saved"], true);
            let config: Value = serde_json::from_slice(
                &fs::read(temporary.path().join("config/journal.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                config["providers"]["active"],
                json!({"provider":"local", "model":"local/qwen3.5-4b"})
            );
            assert_eq!(
                config["setup"]["thinking_selection"],
                config["providers"]["active"]
            );
            assert_eq!(host.start_calls.lock().unwrap().len(), 1);
            // The new choice asks for a fresh thinking check, after its download started.
            assert_eq!(*host.brain_refreshes.lock().unwrap(), vec![1]);
            let repeat = request_with_host(
                temporary.path(),
                host.clone(),
                AccessBasis::Localhost,
                Method::POST,
                "/init/api/action",
                Some(json!({"action":"save_thinking", "choice":"local"})),
            )
            .await;
            assert_eq!(repeat.status(), StatusCode::OK);
            assert_eq!(host.brain_refreshes.lock().unwrap().len(), 1);
        } else {
            assert!(host.brain_refreshes.lock().unwrap().is_empty());
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                response_json(response).await["reason_code"],
                "local_unavailable"
            );
            let state = response_json(
                request_with_host(
                    temporary.path(),
                    host.clone(),
                    AccessBasis::Localhost,
                    Method::GET,
                    "/init/api/state",
                    None,
                )
                .await,
            )
            .await;
            assert!(state["thinking"]["choice"].is_null());
            assert_eq!(state["thinking"]["saved"], false);
            assert!(host.start_calls.lock().unwrap().is_empty());
        }
    }

    let temporary = TempDir::new();
    crate::http::setup_probe_test::capability_admitted(temporary.path(), "unknown", true);
    let host = Arc::new(FakeSetupHost::default());
    let response = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action":"save_thinking", "choice":"local"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let state = response_json(response).await;
    assert_eq!(state["thinking"]["choice"], "local");
    assert_eq!(state["thinking"]["saved"], true);
    let config: Value =
        serde_json::from_slice(&fs::read(temporary.path().join("config/journal.json")).unwrap())
            .unwrap();
    assert_eq!(
        config["providers"]["active"],
        json!({"provider":"local", "model":"local/qwen3.5-4b"})
    );
    assert_eq!(
        config["setup"]["thinking_selection"],
        config["providers"]["active"]
    );
    assert_eq!(host.start_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn save_thinking_later_cleans_up_matched_selection() {
    let temporary = TempDir::new();
    let host = Arc::new(FakeSetupHost::default());
    let config_path = temporary.path().join("config").join("journal.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let local_val = json!({"provider": "local", "model": "local/qwen3.5-4b"});
    fs::write(
        &config_path,
        json!({
            "setup": {"thinking_choice": "local", "thinking_selection": local_val.clone()},
            "providers": {"active": local_val.clone()}
        })
        .to_string(),
    )
    .unwrap();

    let _ = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "save_thinking", "choice": "later"})),
    )
    .await;

    let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    assert_eq!(config["setup"]["thinking_choice"], "later");
    assert!(config["setup"].get("thinking_selection").is_none());
    assert!(config["providers"].get("active").is_none());

    // A different owner providers.active with no matching selection stays
    let custom_val = json!({"provider": "custom", "model": "gpt-4"});
    fs::write(
        &config_path,
        json!({
            "setup": {"thinking_choice": "byo"},
            "providers": {"active": custom_val.clone()}
        })
        .to_string(),
    )
    .unwrap();

    let _ = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "save_thinking", "choice": "later"})),
    )
    .await;

    let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    assert_eq!(config["providers"]["active"], custom_val);
}

#[tokio::test]
async fn start_thinking_download_validations() {
    let temporary = TempDir::new();
    let host = Arc::new(FakeSetupHost::default());

    // Without saved local choice
    let res = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "start_thinking_download"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(res).await["reason_code"],
        "thinking_choice_required"
    );

    // Save choice as byo
    let _ = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "save_thinking", "choice": "byo"})),
    )
    .await;
    let res = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "start_thinking_download"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(res).await["reason_code"],
        "thinking_choice_required"
    );
}

#[tokio::test]
async fn packaged_speech_retry_transports_the_journal_guidance_verbatim() {
    let temporary = TempDir::new();
    let host = Arc::new(FakeSetupHost::default());
    let error =
        crate::setup_progress::packaged_speech_retry(Err("broken package".into())).unwrap_err();
    let detail = error.detail.clone();
    *host.retry_speech_error.lock().unwrap() = Some(error);
    let response = request_with_host(
        temporary.path(),
        host,
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "retry_speech_download"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response_json(response).await;
    assert_eq!(body["reason_code"], "speech_package_unavailable");
    assert_eq!(body["detail"], detail);
    assert_eq!(body["error"], "speech recognition needs repair");
}

#[tokio::test]
async fn unknown_action_and_choice_rejected() {
    let temporary = TempDir::new();
    let config_path = temporary.path().join("config").join("journal.json");

    let res = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "unknown_action"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "save_thinking", "choice": "invalid_choice"})),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = request_body(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Body::from("{not json"),
        Some("application/json"),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    assert!(!config_path.exists());
}

#[tokio::test]
async fn float_completed_at_preserves_completion_and_leaves_config_untouched() {
    let temporary = committed_journal();
    let config_path = temporary.path().join("config").join("journal.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let initial_json = json!({
        "setup": {"completed_at": 1791570000.0},
        "retention": {"raw_media": "days", "raw_media_days": 14}
    });
    fs::write(&config_path, initial_json.to_string()).unwrap();

    let state = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::GET,
            "/init/api/state",
            None,
        )
        .await,
    )
    .await;
    assert_eq!(state["done"], true);
    assert_eq!(state["remaining_required_steps"], json!([]));

    // POST /init/finalize with different retention
    let fin_resp = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({"retention_mode": "keep", "lane": "byo"})),
    )
    .await;
    assert_eq!(fin_resp.status(), StatusCode::OK);

    let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    assert_eq!(config["setup"]["completed_at"], 1791570000.0);
    assert_eq!(config["retention"]["raw_media"], "days");
    assert_eq!(config["retention"]["raw_media_days"], 14);
    assert!(config["setup"].get("thinking_choice").is_none());

    // action=finalize
    let action_resp = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action": "finalize"})),
    )
    .await;
    assert_eq!(action_resp.status(), StatusCode::OK);

    let config2: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    assert_eq!(config2["setup"]["completed_at"], 1791570000.0);
    assert_eq!(config2["retention"]["raw_media"], "days");
    assert_eq!(config2["retention"]["raw_media_days"], 14);
    assert!(config2["setup"].get("thinking_choice").is_none());
}

#[tokio::test]
async fn old_finalize_lane_redirects_and_choice_storage() {
    let temporary = committed_journal();
    let res = response_json(
        request(
            temporary.path(),
            AccessBasis::Localhost,
            Method::POST,
            "/init/finalize",
            Some(json!({"lane": "confidential"})),
        )
        .await,
    )
    .await;
    assert_eq!(res["redirect"], "/app/home");
    let config: Value = serde_json::from_slice(
        &fs::read(temporary.path().join("config").join("journal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["setup"]["thinking_choice"], "spp");

    // byo
    let temp_byo = committed_journal();
    let res_byo = response_json(
        request(
            temp_byo.path(),
            AccessBasis::Localhost,
            Method::POST,
            "/init/finalize",
            Some(json!({"lane": "byo"})),
        )
        .await,
    )
    .await;
    assert_eq!(res_byo["redirect"], "/app/home");

    // local completes even when host start returns Err
    let temp_local = committed_journal();
    crate::http::setup_probe_test::capability(temp_local.path(), "ok");
    let host = Arc::new(FakeSetupHost::default());
    *host.start_error.lock().unwrap() = Some("model download blocked".into());
    let res_local = request_with_host(
        temp_local.path(),
        host,
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(json!({"lane": "local"})),
    )
    .await;
    assert_eq!(res_local.status(), StatusCode::OK);
}

#[tokio::test]
async fn finalize_principal_failure_does_not_skip_local_start_and_can_retry() {
    let temporary = committed_journal();
    crate::http::setup_probe_test::capability(temporary.path(), "ok");
    let host = Arc::new(FakeSetupHost::default());
    let entities = temporary.path().join("entities");
    fs::write(&entities, b"not a directory").unwrap();
    let body = json!({"lane":"local", "name":"Ada", "preferred":"Ada"});
    let failed = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(body.clone()),
    )
    .await;
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(host.start_calls.lock().unwrap().len(), 1);

    fs::remove_file(entities).unwrap();
    let retried = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/finalize",
        Some(body),
    )
    .await;
    assert_eq!(retried.status(), StatusCode::OK);
    assert_eq!(host.start_calls.lock().unwrap().len(), 1);
    assert!(
        solstone_core_entity::read_journal_principal(temporary.path())
            .unwrap()
            .is_some()
    );
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
    establish::lock_in(temporary.path()).unwrap();
    temporary
}

#[tokio::test]
async fn first_setup_state_read_does_not_create_any_journal_files() {
    let temporary = TempDir::new();
    let response = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let state = response_json(response).await;
    assert_eq!(state["mark"]["state"], "unavailable");
    assert_eq!(state["done"], false);
    assert!(fs::read_dir(temporary.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn mark_and_saved_choice_are_not_done_until_finalize_commits() {
    let temporary = TempDir::new();
    let proposal = establish::ensure_mark_snapshot(temporary.path()).unwrap();
    establish::commit_proposal(temporary.path(), proposal.proposal_id.as_deref().unwrap()).unwrap();
    let saved = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action":"save_thinking","choice":"later"})),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(response_json(saved).await["done"], false);
    let state = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    assert_eq!(response_json(state).await["done"], false);
    let finished = request(
        temporary.path(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action":"finalize"})),
    )
    .await;
    assert_eq!(finished.status(), StatusCode::OK);
    assert_eq!(response_json(finished).await["done"], true);
}

#[tokio::test]
async fn repeated_local_save_does_not_restart_a_cancelled_attempt() {
    let temporary = TempDir::new();
    let local = json!({"provider":"local","model":"local/qwen3.5-4b"});
    fs::create_dir_all(temporary.path().join("config")).unwrap();
    fs::write(
        temporary.path().join("config/journal.json"),
        serde_json::to_vec(&json!({
            "setup":{"thinking_choice":"local","thinking_selection":local},
            "providers":{"active":local}
        }))
        .unwrap(),
    )
    .unwrap();
    let host = Arc::new(FakeSetupHost::default());
    let mut projected = host.project_downloads(temporary.path()).unwrap();
    projected.thinking.state = "cancelled".into();
    projected.thinking.attempt_id = Some("cancelled-attempt".into());
    *host.projected.lock().unwrap() = Some(projected);
    let response = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action":"save_thinking","choice":"local"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(host.start_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retained_finalize_releases_only_its_own_local_selection() {
    for (lane, choice) in [("later", "later"), ("byo", "byo"), ("confidential", "spp")] {
        for owner_changed in [false, true] {
            let temporary = TempDir::new();
            let proposal = establish::ensure_mark_snapshot(temporary.path()).unwrap();
            establish::commit_proposal(temporary.path(), proposal.proposal_id.as_deref().unwrap())
                .unwrap();
            let local = json!({"provider":"local","model":"local/qwen3.5-4b"});
            let owner = json!({"provider":"openai","model":"owner-model"});
            fs::create_dir_all(temporary.path().join("config")).unwrap();
            fs::write(
                temporary.path().join("config/journal.json"),
                serde_json::to_vec(&json!({
                    "setup":{"thinking_choice":"local","thinking_selection":local},
                    "providers":{"active":if owner_changed { &owner } else { &local }}
                }))
                .unwrap(),
            )
            .unwrap();
            let response = request(
                temporary.path(),
                AccessBasis::Localhost,
                Method::POST,
                "/init/finalize",
                Some(json!({"lane":lane})),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let config: Value = serde_json::from_slice(
                &fs::read(temporary.path().join("config/journal.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(config["setup"]["thinking_choice"], choice);
            if owner_changed {
                assert_eq!(config["providers"]["active"], owner);
            } else {
                assert!(config["providers"].get("active").is_none());
            }
            assert!(config["setup"].get("thinking_selection").is_none());
        }
    }
}

#[tokio::test]
async fn corrupt_proposal_is_visible_and_never_silently_replaced() {
    let temporary = TempDir::new();
    establish::ensure_mark_snapshot(temporary.path()).unwrap();
    let key = temporary.path().join("link/ca-staging/private.pem");
    fs::write(&key, b"corrupt fixture").unwrap();
    for (method, route, body) in [
        (Method::GET, "/init/api/state", None),
        (Method::GET, "/init/mark", None),
        (
            Method::POST,
            "/init/api/action",
            Some(json!({"action":"regenerate_mark"})),
        ),
    ] {
        let response = request(
            temporary.path(),
            AccessBasis::Localhost,
            method,
            route,
            body,
        )
        .await;
        assert!(!response.status().is_success(), "{route}");
        assert_eq!(fs::read(&key).unwrap(), b"corrupt fixture");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_retry_cannot_reactivate_local_after_another_client_saves_later() {
    let temporary = TempDir::new();
    let root = temporary.path().to_owned();
    let local = json!({"provider":"local","model":"local/qwen3.5-4b"});
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/journal.json"), serde_json::to_vec(&json!({"setup":{"thinking_choice":"local","thinking_selection":local},"providers":{"active":local}})).unwrap()).unwrap();
    let host = Arc::new(FakeSetupHost::default());
    let (reached, release) = crate::http::setup_probe_test::pause(&root);
    let retry_root = root.clone();
    let retry_host = host.clone();
    let retry = tokio::spawn(async move {
        request_with_host(
            &retry_root,
            retry_host,
            AccessBasis::Localhost,
            Method::POST,
            "/init/api/action",
            Some(json!({"action":"start_thinking_download"})),
        )
        .await
    });
    tokio::task::spawn_blocking(move || {
        reached
            .recv_timeout(std::time::Duration::from_secs(60))
            .unwrap()
    })
    .await
    .unwrap();
    let later = request_with_host(
        &root,
        host.clone(),
        AccessBasis::Localhost,
        Method::POST,
        "/init/api/action",
        Some(json!({"action":"save_thinking","choice":"later"})),
    )
    .await;
    assert_eq!(later.status(), StatusCode::OK);
    release.send(()).unwrap();
    assert_eq!(retry.await.unwrap().status(), StatusCode::CONFLICT);
    assert!(host.start_calls.lock().unwrap().is_empty());
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("config/journal.json")).unwrap()).unwrap();
    assert_eq!(config["setup"]["thinking_choice"], "later");
    assert!(config["providers"].get("active").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_finalize_rechecks_provider_eligibility_after_probe() {
    let temporary = TempDir::new();
    let root = temporary.path().to_owned();
    let proposal = establish::ensure_mark_snapshot(&root).unwrap();
    establish::commit_proposal(&root, proposal.proposal_id.as_deref().unwrap()).unwrap();
    let host = Arc::new(FakeSetupHost::default());
    let (reached, release) = crate::http::setup_probe_test::pause(&root);
    let finalize_root = root.clone();
    let finalize_host = host.clone();
    let finalize = tokio::spawn(async move {
        request_with_host(
            &finalize_root,
            finalize_host,
            AccessBasis::Localhost,
            Method::POST,
            "/init/finalize",
            Some(json!({"lane":"local"})),
        )
        .await
    });
    tokio::task::spawn_blocking(move || {
        reached
            .recv_timeout(std::time::Duration::from_secs(60))
            .unwrap()
    })
    .await
    .unwrap();
    solstone_core_journal_config_write::mutate_journal_config(&root, Default::default(), |config| {
        config.insert("providers".into(), json!({"local":{"endpoint_url":"http://127.0.0.1:9","served_model_id":"owner-model"}}));
        solstone_core_journal_config_write::JournalConfigMutation { changed:true, value:() }
    }).unwrap();
    release.send(()).unwrap();
    assert_eq!(finalize.await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert!(host.start_calls.lock().unwrap().is_empty());
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("config/journal.json")).unwrap()).unwrap();
    assert!(config["providers"].get("active").is_none());
    assert!(config["setup"].get("completed_at").is_none());
    assert_eq!(
        config["providers"]["local"]["served_model_id"],
        "owner-model"
    );
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
        crate::http::setup_probe_test::clear(&self.path);
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[tokio::test]
async fn thinking_line_uses_the_admitted_transfer_total_before_reprobing_the_host() {
    let temporary = TempDir::new();
    let host = Arc::new(FakeSetupHost::default());
    *host.thinking_bytes.lock().unwrap() = Some(Some(99));
    let mut downloads = host.project_downloads(temporary.path()).unwrap();
    downloads.thinking.state = "downloading".into();
    downloads.thinking.attempt_id = Some("admitted".into());
    downloads.thinking.bytes_total = Some(42);
    *host.projected.lock().unwrap() = Some(downloads.clone());
    let response = request_with_host(
        temporary.path(),
        host.clone(),
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await["thinking"]["download_bytes"],
        42
    );
    downloads.thinking.state = "idle".into();
    *host.projected.lock().unwrap() = Some(downloads);
    let response = request_with_host(
        temporary.path(),
        host,
        AccessBasis::Localhost,
        Method::GET,
        "/init/api/state",
        None,
    )
    .await;
    assert_eq!(
        response_json(response).await["thinking"]["download_bytes"],
        99
    );
}
