// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use chrono::{Duration, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_brain::{begin_refresh, finish_refresh};
use solstone_core_convey_shell::{
    ConfidentialPoll, ConfidentialRuntimeOverride, PollOutcome, router,
};
use solstone_core_sol_link::ca::generate_ca;
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(phase: &str) -> Self {
        let serial = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-thinking-{phase}-{}-{nanos}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("journal creates");
        Self(path)
    }

    fn config(&self, value: Value) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(
            self.0.join("config/journal.json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&value).expect("config serializes")
            ),
        )
        .expect("config writes");
    }

    fn corrupt_config(&self) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(
            self.0.join("config/journal.json"),
            br#"{"setup": {"completed_at": 17672256"#,
        )
        .expect("corrupt config writes");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/convey_thinking_corpus.json"
    ))
    .expect("thinking corpus parses")
}

fn confidential() -> Value {
    json!({
        "endpoint_url": "https://spp.example.invalid/v1",
        "served_model_id": "served-model",
        "credential_fingerprint_sha256": sha256(b"corpus-confidential-credential"),
        "prior_active": {"provider": "openai", "model": "gpt-5"},
    })
}

fn write_link_ca(journal: &Path) {
    let ca = generate_ca().expect("test CA generates");
    let directory = journal.join("link/ca");
    fs::create_dir_all(&directory).expect("CA directory creates");
    fs::write(directory.join("cert.pem"), ca.certificate_pem()).expect("certificate writes");
    fs::write(directory.join("private.pem"), ca.private_key_pem()).expect("private key writes");
}

fn write_link_state(journal: &Path, instance_id: &str) {
    let directory = journal.join("link");
    fs::create_dir_all(&directory).expect("link directory creates");
    fs::write(
        directory.join("state.json"),
        serde_json::to_vec_pretty(&json!({
            "instance_id": instance_id,
            "home_label": "solstone",
        }))
        .expect("link state serializes"),
    )
    .expect("link state writes");
}

fn link_snapshot(journal: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn collect(root: &Path, directory: &Path, entries: &mut Vec<(PathBuf, Vec<u8>)>) {
        let Ok(children) = fs::read_dir(directory) else {
            return;
        };
        for child in children {
            let child = child.expect("link entry reads");
            let path = child.path();
            if path.is_dir() {
                collect(root, &path, entries);
            } else {
                entries.push((
                    path.strip_prefix(root)
                        .expect("link-relative path")
                        .to_owned(),
                    fs::read(&path).expect("link file reads"),
                ));
            }
        }
    }

    let root = journal.join("link");
    let mut entries = Vec::new();
    collect(&root, &root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn write_runtime_health(journal: &Path, revision: u64, phase: &str, reason_code: Value) {
    let directory = journal.join("health/providers/runtime");
    fs::create_dir_all(&directory).expect("runtime directory creates");
    fs::write(
        directory.join("local.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "provider": "local",
            "revision": revision,
            "phase": phase,
            "reason_code": reason_code,
            "detail": {},
            "desired_fingerprint_sha256": "desired-fingerprint",
            "incarnation": null,
            "generation": 0,
            "attempt": 0,
            "process": null,
            "updated_at": null,
            "display_deadline_at": null,
            "owner": null,
        }))
        .expect("health record serializes"),
    )
    .expect("health record writes");
}

fn write_runtime_retry(journal: &Path, revision: u64) {
    let directory = journal.join("health/providers/runtime");
    fs::create_dir_all(&directory).expect("runtime directory creates");
    fs::write(
        directory.join("local.retry-token.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "revision": revision,
            "token_id": "requested-token",
            "desired_fingerprint_sha256": "desired-fingerprint",
            "requested_at": "2026-01-01T00:00:00+00:00",
            "reason_code": "retry-token-requested",
            "owner": {},
        }))
        .expect("retry record serializes"),
    )
    .expect("retry record writes");
}

struct ParkedPoll {
    started: Sender<()>,
    release: Mutex<Receiver<()>>,
}

impl ConfidentialPoll for ParkedPoll {
    fn poll(&self, _base_url: &str, _nonce: &str) -> PollOutcome {
        self.started.send(()).expect("test observes poll");
        self.release
            .lock()
            .expect("release lock")
            .recv()
            .expect("test releases poll");
        PollOutcome::EarlyAccess
    }
}

struct PanicPoll;

impl ConfidentialPoll for PanicPoll {
    fn poll(&self, _base_url: &str, _nonce: &str) -> PollOutcome {
        panic!("test poll panic")
    }
}

struct EarlyAccessPoll;

impl ConfidentialPoll for EarlyAccessPoll {
    fn poll(&self, _base_url: &str, _nonce: &str) -> PollOutcome {
        PollOutcome::EarlyAccess
    }
}

struct SuccessPoll {
    payload: Map<String, Value>,
}

impl ConfidentialPoll for SuccessPoll {
    fn poll(&self, _base_url: &str, _nonce: &str) -> PollOutcome {
        PollOutcome::Success(self.payload.clone())
    }
}

fn router_with_runtime(
    journal: PathBuf,
    portal_base_url: &str,
    poll: Arc<dyn ConfidentialPoll>,
) -> axum::Router {
    router(journal).layer(Extension(ConfidentialRuntimeOverride {
        portal_base_url: portal_base_url.to_owned(),
        poll,
    }))
}

fn consent_identity_and_nonce(
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) -> (String, String) {
    let body: Value = serde_json::from_slice(&response.3).expect("enable JSON");
    let url = body["operation"]["portal_url"]
        .as_str()
        .expect("consent URL");
    let nonce = url
        .split("?nonce=")
        .nth(1)
        .and_then(|value| value.split('&').next())
        .expect("nonce")
        .to_owned();
    let instance = url
        .split_once("&instance=")
        .expect("mandatory instance")
        .1
        .to_owned();
    (instance, nonce)
}

async fn wait_for_operation_phase(app: axum::Router, phase: &str) {
    for _ in 0..20 {
        tokio::task::yield_now().await;
        let response = request(app.clone(), "GET", "/app/thinking/api/providers").await;
        let body: Value = serde_json::from_slice(&response.3).expect("providers JSON");
        if body["active_lane"]["confidential_operation"]["phase"] == phase {
            return;
        }
    }
    panic!("operation settles to {phase}")
}

fn established(extra: Value) -> Value {
    let mut value = json!({"setup": {"completed_at": 1767225600}});
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    value
}

fn journal_for_phase(phase: &str) -> TempDir {
    let root = TempDir::new(phase);
    let config = match phase {
        "unestablished" => return root,
        "corrupt" => {
            root.corrupt_config();
            return root;
        }
        "none" => established(json!({})),
        "bundled_local" => established(
            json!({"providers": {"active": {"provider": "local", "model": "local/qwen3.5-4b"}}}),
        ),
        "byo_cloud" => established(
            json!({"env": {"OPENAI_API_KEY": "sk-corpus-not-a-real-key"}, "providers": {"active": {"provider": "openai", "model": "gpt-5"}, "byo_models": {"openai": "gpt-5"}, "key_validation": {"openai": {"valid": true, "timestamp": "2026-01-01T00:00:00+00:00"}}}}),
        ),
        "byo_endpoint" => established(
            json!({"providers": {"active": {"provider": "local", "model": "served-model"}, "local": {"endpoint_url": "http://127.0.0.1:1/v1", "served_model_id": "served-model", "credential": "corpus-credential"}}}),
        ),
        "confidential_inactive" => established(
            json!({"env": {"OPENAI_API_KEY": "sk-corpus-not-a-real-key"}, "providers": {"active": {"provider": "openai", "model": "gpt-5"}, "local": {"endpoint_url": "https://spp.example.invalid/v1", "served_model_id": "served-model", "credential": "corpus-confidential-credential"}}, "services": {"confidential": confidential()}}),
        ),
        "confidential" => established(
            json!({"providers": {"active": {"provider": "local", "model": "served-model"}, "local": {"endpoint_url": "https://spp.example.invalid/v1", "served_model_id": "served-model", "credential": "corpus-confidential-credential"}}, "services": {"confidential": confidential()}}),
        ),
        _ => panic!("unknown phase {phase}"),
    };
    root.config(config);
    if phase == "none" {
        assert!(
            solstone_core_thinking::read_config(&root.0)
                .expect("none config reads")
                .get("providers")
                .is_none()
        );
    }
    if phase != "none" {
        seed_brain(&root.0, phase);
    }
    root
}

fn seed_brain(journal: &Path, phase: &str) {
    let now = Utc::now();
    let expires = now + Duration::days(1);
    let component =
        json!({"status":"ok","observed_at":now.to_rfc3339(),"expires_at":expires.to_rfc3339()});
    let bundled_runtime = (phase == "bundled_local").then(|| "c".repeat(64));
    let permit = begin_refresh(journal, now, None, None, false, bundled_runtime.clone())
        .expect("brain refresh begins")
        .unwrap_or_else(|| panic!("{phase} configured brain produces a permit"));
    finish_refresh(
        journal,
        permit,
        json!({"configuration": component, "lane_prerequisites": component, "generate": component, "cogitate": component}),
        now,
        bundled_runtime,
    )
    .expect("brain refresh finishes");
}

async fn request(
    app: axum::Router,
    method: &str,
    path: &str,
) -> (StatusCode, String, Option<String>, Vec<u8>) {
    request_with_body(app, method, path, None).await
}

async fn request_with_body(
    app: axum::Router,
    method: &str,
    path: &str,
    request_json: Option<&Value>,
) -> (StatusCode, String, Option<String>, Vec<u8>) {
    let mut builder = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).expect("method parses"))
        .uri(path);
    let body = match request_json {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(value).expect("request body serializes"))
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(builder.body(body).expect("request builds"))
        .await
        .expect("router responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("content type present")
        .to_str()
        .expect("content type text")
        .to_owned();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|value| value.to_str().expect("location text").to_owned());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads")
        .to_vec();
    (status, content_type, location, body)
}

/// `native_deviations[0]` is shaped like eight corpus vectors, one per phase:
/// bodyless `PUT /api/generators`. The corrupt and unestablished vectors are
/// preempted by the session gate; exactly six established vectors reach this
/// native assertion. The reference crashes its handler while parsing a
/// bodyless request and returns a generic 500, while native deliberately
/// returns the standard typed 400 refusal used by sibling write routes.
fn is_generators_missing_body_deviation(phase: &str, case: &Value) -> bool {
    is_established_phase(phase)
        && case["method"] == "PUT"
        && case["path"] == "/app/thinking/api/generators"
        && case.get("request_json").is_none()
}

fn runtime_status_deviation_path(_phase: &str, case: &Value) -> Option<&'static str> {
    if case
        .pointer("/json/providers/local_runtime/status")
        .and_then(Value::as_str)
        == Some("ok")
    {
        Some("providers.local_runtime.status")
    } else if case
        .pointer("/json/local_runtime/status")
        .and_then(Value::as_str)
        == Some("ok")
    {
        Some("local_runtime.status")
    } else {
        None
    }
}

fn assert_generators_missing_body_deviation(
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) {
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    assert_eq!(response.1, "application/json");
    assert_eq!(response.2, None);
    let actual: Value = serde_json::from_slice(&response.3).expect("deviation envelope is JSON");
    assert_eq!(
        actual,
        json!({
            "error": "I couldn't find any data in that request.",
            "reason_code": "missing_request_body",
            "detail": "No data provided",
        })
    );
}

fn replay_full_recorded_case(
    phase: &str,
    index: usize,
    journal: &Path,
    case: &Value,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) -> (usize, bool, bool) {
    let path = case["path"].as_str().expect("path");
    assert_eq!(
        response.0.as_u16(),
        case["status"].as_u64().expect("status") as u16,
        "{phase} {path} status: {}",
        String::from_utf8_lossy(&response.3)
    );
    assert_eq!(
        response.1,
        case["content_type"].as_str().expect("content type"),
        "{phase} {path} content type"
    );
    assert_eq!(
        response.2.as_deref(),
        case.get("location").and_then(Value::as_str),
        "{phase} {path} location"
    );
    // A record with no `body_sha256` is deliberately not body-asserted: served
    // frontend assets (html/js/css) keep their status, content-type and location
    // pins above and drop the whole-file digest. See the fixture's
    // corpus_maintenance note. `superseded_presentation_asset` below is the
    // stronger form of the same idea -- compare against the asset on disk.
    if case.get("body_sha256").and_then(Value::as_str).is_none() {
        return (
            body_arm(case),
            false,
            runtime_status_deviation_path(phase, case).is_some(),
        );
    }
    let mut normalized_response = None;
    let actual_hash = match (
        case["body_sha256_basis"].as_str(),
        case.get("body_normalized"),
    ) {
        (Some("raw-body"), None) => sha256(&response.3),
        (Some("raw-body"), Some(_)) => sha256(
            String::from_utf8(response.3.clone())
                .expect("normalized body is text")
                .replace(&journal.display().to_string(), "<JOURNAL_ROOT>")
                .as_bytes(),
        ),
        (Some("normalized-json"), _) => {
            let mut value: Value =
                serde_json::from_slice(&response.3).expect("normalized response is JSON");
            for (path, kind) in case
                .get("normalized_fields")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                set_path(&mut value, path, placeholder(kind.as_str().expect("kind")));
            }
            #[cfg(target_os = "macos")]
            if case
                .pointer("/json/providers/local_backend")
                .and_then(Value::as_str)
                == Some("local")
                && value
                    .pointer("/providers/local_backend")
                    .and_then(Value::as_str)
                    == Some("metal")
            {
                set_path(&mut value, "providers.local_backend", "local");
            }
            let runtime_status_deviation = runtime_status_deviation_path(phase, case);
            if let Some(path) = runtime_status_deviation {
                let actual_status = if path == "providers.local_runtime.status" {
                    &value["providers"]["local_runtime"]["status"]
                } else {
                    &value["local_runtime"]["status"]
                };
                let reference_status = if path == "providers.local_runtime.status" {
                    &case["json"]["providers"]["local_runtime"]["status"]
                } else {
                    &case["json"]["local_runtime"]["status"]
                };
                assert_eq!(
                    actual_status.as_str(),
                    Some("blocked"),
                    "{phase} {path} native status"
                );
                assert_eq!(
                    reference_status.as_str(),
                    Some("ok"),
                    "{phase} {path} reference status"
                );
                set_path(&mut value, path, "ok");
            }
            let hash = sha256(canonical_json(&value).as_bytes());
            normalized_response = Some(value);
            hash
        }
        other => panic!("unknown body arm {other:?}"),
    };
    if actual_hash != case["body_sha256"].as_str().expect("body hash") {
        if runtime_status_deviation_path(phase, case).is_some()
            && let Some(actual) = normalized_response.as_ref()
        {
            assert_eq!(actual, &case["json"], "{phase} {path} normalized JSON");
            return (body_arm(case), false, true);
        }
        // This explicit 198-vector allowlist is the only semantic fallback
        // outside the corrupt API-envelope bucket. Native `error_envelope()`
        // emits {"error","reason_code","detail"} in insertion order with
        // no trailing newline; recorded Flask `jsonify()` emits sorted
        // {"detail","error","reason_code"} plus a trailing newline.
        // Their decoded fields are identical. This retains that field-equality
        // pin while leaving serialization parity to the shared envelope owner;
        // all other cases remain byte-pinned.
        if is_established_error_envelope_byte_fallback(phase, index)
            && let Some(expected) = case.get("json")
            && let Ok(mut actual) = serde_json::from_slice::<Value>(&response.3)
        {
            normalize_journal_root(&mut actual, &journal.display().to_string());
            assert_eq!(&actual, expected, "{phase} {path} envelope fields");
            return (body_arm(case), true, false);
        }
        if let Some(actual) = normalized_response {
            panic!(
                "{phase} {path} normalized JSON differs at {:?}",
                differing_paths(&actual, &case["json"], "")
            );
        }
    }
    assert_eq!(
        actual_hash,
        case["body_sha256"].as_str().expect("body hash"),
        "{phase} {path} body"
    );
    (
        body_arm(case),
        false,
        runtime_status_deviation_path(phase, case).is_some(),
    )
}

fn superseded_presentation_asset(path: &str) -> Option<&'static [u8]> {
    match path {
        "/app/thinking/workspace" => Some(include_bytes!("../assets/thinking/workspace.html")),
        "/app/thinking/static/thinking.js" => {
            Some(include_bytes!("../assets/thinking/thinking.js"))
        }
        _ => None,
    }
}

fn assert_superseded_presentation_case(
    phase: &str,
    case: &Value,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
    expected_asset: &[u8],
) {
    let path = case["path"].as_str().expect("path");
    assert_eq!(case["method"], "GET", "{phase} {path} fixture method");
    assert_eq!(case["status"], 200, "{phase} {path} fixture status");
    assert_eq!(response.0, StatusCode::OK, "{phase} {path} status");
    assert_eq!(
        response.1,
        case["content_type"].as_str().expect("content type"),
        "{phase} {path} content type"
    );
    assert_eq!(response.2, None, "{phase} {path} location");
    assert_eq!(response.3, expected_asset, "{phase} {path} native asset");
}

fn is_established_phase(phase: &str) -> bool {
    matches!(
        phase,
        "none"
            | "bundled_local"
            | "byo_cloud"
            | "byo_endpoint"
            | "confidential_inactive"
            | "confidential"
    )
}

fn is_established_error_envelope_byte_fallback(phase: &str, index: usize) -> bool {
    match phase {
        "none" | "bundled_local" | "byo_cloud" | "byo_endpoint" => {
            matches!(index, 18..=47 | 49 | 50 | 54)
        }
        "confidential_inactive" | "confidential" => {
            matches!(index, 18..=34 | 36..=47 | 49 | 50 | 54 | 55)
        }
        _ => false,
    }
}

const CORRUPT_API_ENVELOPE_PATHS: &[&str] = &[
    "/app/thinking/api/state",
    "/app/thinking/api/providers",
    "/app/thinking/api/providers?local_model=local/qwen3.5-4b",
    "/app/thinking/api/providers?local_model=nope",
    "/app/thinking/api/providers?local_model=",
    "/app/thinking/api/keys",
    "/app/thinking/api/providers/local/status",
    "/app/thinking/api/local/availability",
    "/app/thinking/api/local/availability?model=nope",
    "/app/thinking/api/local/bootstrap/status",
    "/app/thinking/api/local/bootstrap/status?model=nope",
    "/app/thinking/api/local/models",
    "/app/thinking/api/local/runtime",
    "/app/thinking/api/generators",
    "/app/thinking/api/keys/check",
    "/app/thinking/api/validate-model",
    "/app/thinking/api/local/endpoint",
    "/app/thinking/api/local/runtime/retry",
];

fn is_corrupt_api_envelope_case(phase: &str, case: &Value) -> bool {
    phase == "corrupt"
        && case["content_type"] == "application/json"
        && CORRUPT_API_ENVELOPE_PATHS.contains(&case["path"].as_str().expect("path"))
}

fn assert_corrupt_api_envelope_case(
    journal: &Path,
    case: &Value,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) {
    let path = case["path"].as_str().expect("path");
    assert_eq!(
        response.0,
        StatusCode::INTERNAL_SERVER_ERROR,
        "corrupt {path}"
    );
    assert_eq!(response.1, "application/json", "corrupt {path}");
    assert_eq!(response.2, None, "corrupt {path}");
    let mut actual: Value = serde_json::from_slice(&response.3).expect("corrupt envelope is JSON");
    normalize_journal_root(&mut actual, &journal.display().to_string());
    assert_eq!(actual, case["json"], "corrupt {path} envelope fields");
}

fn assert_no_slash_deviation(
    phase: &str,
    journal: &Path,
    response: &(StatusCode, String, Option<String>, Vec<u8>),
) {
    let (status, content_type, location, body) = response;
    match phase {
        "unestablished" => {
            assert_eq!(*status, StatusCode::FOUND);
            assert_eq!(content_type, "text/html; charset=utf-8");
            assert_eq!(location.as_deref(), Some("/init"));
            assert_eq!(
                body,
                b"<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"/init\">/init</a>. If not, click the link.\n"
            );
        }
        "corrupt" => {
            assert_eq!(*status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(content_type, "text/plain; charset=utf-8");
            assert_eq!(location, &None);
            assert_eq!(
                body,
                &format!("I couldn't read your settings file at {}/config/journal.json. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.", journal.display()).into_bytes()
            );
        }
        other => panic!("unexpected no-slash deviation phase {other}"),
    }
}

fn normalize_journal_root(value: &mut Value, journal_root: &str) {
    match value {
        Value::String(text) => *text = text.replace(journal_root, "<JOURNAL_ROOT>"),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| normalize_journal_root(value, journal_root)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|value| normalize_journal_root(value, journal_root)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn body_arm(case: &Value) -> usize {
    match (
        case["body_sha256_basis"].as_str(),
        case.get("body_normalized"),
    ) {
        (Some("raw-body"), None) => 0,
        (Some("raw-body"), Some(_)) => 1,
        (Some("normalized-json"), _) => 2,
        _ => unreachable!(),
    }
}

fn differing_paths(actual: &Value, expected: &Value, prefix: &str) -> Vec<String> {
    if actual == expected {
        return Vec::new();
    }
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => actual
            .keys()
            .chain(expected.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .flat_map(|key| {
                differing_paths(
                    actual.get(key).unwrap_or(&Value::Null),
                    expected.get(key).unwrap_or(&Value::Null),
                    &format!("{prefix}.{key}"),
                )
            })
            .collect(),
        _ => vec![prefix.to_owned()],
    }
}

fn set_path(value: &mut Value, path: &str, replacement: &str) {
    let mut current = value;
    let mut fields = path.split('.').peekable();
    while let Some(field) = fields.next() {
        if fields.peek().is_none() {
            current
                .as_object_mut()
                .expect("normalization object")
                .insert(field.to_owned(), Value::String(replacement.to_owned()));
            return;
        }
        current = current
            .as_object_mut()
            .expect("normalization object")
            .get_mut(field)
            .expect("normalization field");
    }
}

fn placeholder(kind: &str) -> &'static str {
    match kind {
        "capture-clock" => "<CAPTURE_CLOCK>",
        "host" => "<HOST_DEPENDENT>",
        "version" => "<VERSION>",
        other => panic!("unknown normalization kind {other}"),
    }
}
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => json_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut fields: Vec<_> = values.iter().collect();
            fields.sort_unstable_by(|left, right| left.0.cmp(right.0));
            format!(
                "{{{}}}",
                fields
                    .into_iter()
                    .map(|(key, value)| format!("{}:{}", json_string(key), canonical_json(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' || !character.is_ascii() => {
                let code = character as u32;
                if code <= 0xffff {
                    output.push_str(&format!("\\u{code:04x}"));
                } else {
                    let code = code - 0x1_0000;
                    output.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (code >> 10),
                        0xdc00 + (code & 0x3ff)
                    ));
                }
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn assert_top_level_keys(body: &Value, mut expected: Vec<&str>) {
    let mut actual: Vec<_> = body
        .as_object()
        .expect("response is an object")
        .keys()
        .map(String::as_str)
        .collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

/// Replays all 448 fixture cases against one journal per phase in the
/// generator's recorded order. Every recorded request body is sent. Each
/// established phase contains 35 non-GET cases; the trailing
/// `DELETE /api/local/endpoint` remains order-dependent, but is not unique.
/// The result has one arm per fixture record: byte pins, three documented native
/// deviations, corrupt semantic envelopes, and the explicit shared-envelope
/// serialization fallback remain mutually exclusive assertion buckets.
#[tokio::test]
async fn all_fixture_cases_replay_in_recorded_phase_order_with_bodies() {
    let corpus = corpus();
    let mut count = 0;
    let mut arms = [0; 3];
    let mut byte_pinned = 0;
    let mut corrupt_semantic = 0;
    let mut established_error_envelope_fallback = 0;
    let mut superseded_presentation = 0;
    let mut no_slash_deviations = 0;
    let mut generators_missing_body_deviation = 0;
    let mut runtime_status_deviations = 0;
    for phase in [
        "unestablished",
        "corrupt",
        "none",
        "bundled_local",
        "byo_cloud",
        "byo_endpoint",
        "confidential_inactive",
        "confidential",
    ] {
        let journal = journal_for_phase(phase);
        for (index, case) in corpus["phases"][phase]
            .as_array()
            .expect("phase cases")
            .iter()
            .enumerate()
        {
            let response = request_with_body(
                router(journal.0.clone()),
                case["method"].as_str().expect("method"),
                case["path"].as_str().expect("path"),
                case.get("request_json"),
            )
            .await;
            let arm = body_arm(case);
            if is_corrupt_api_envelope_case(phase, case) {
                assert_corrupt_api_envelope_case(&journal.0, case, &response);
                corrupt_semantic += 1;
            } else if case["path"] == "/app/thinking"
                && matches!(phase, "unestablished" | "corrupt")
            {
                assert_no_slash_deviation(phase, &journal.0, &response);
                no_slash_deviations += 1;
            } else if is_generators_missing_body_deviation(phase, case) {
                assert_generators_missing_body_deviation(&response);
                generators_missing_body_deviation += 1;
            } else if is_established_phase(phase)
                && let Some(expected_asset) =
                    superseded_presentation_asset(case["path"].as_str().expect("path"))
            {
                assert_superseded_presentation_case(phase, case, &response, expected_asset);
                superseded_presentation += 1;
            } else {
                let (replayed_arm, fallback, runtime_status_deviation) =
                    replay_full_recorded_case(phase, index, &journal.0, case, &response);
                assert_eq!(replayed_arm, arm, "{phase} index {index} arm");
                if fallback {
                    established_error_envelope_fallback += 1;
                } else if runtime_status_deviation {
                    runtime_status_deviations += 1;
                } else {
                    byte_pinned += 1;
                }
            }
            arms[arm] += 1;
            count += 1;
        }
    }
    assert_eq!(count, 448);
    assert_eq!(arms, [361, 55, 32]);
    assert_eq!(byte_pinned, 155);
    assert_eq!(corrupt_semantic, 49);
    assert_eq!(established_error_envelope_fallback, 198);
    assert_eq!(superseded_presentation, 12);
    assert_eq!(no_slash_deviations, 2);
    assert_eq!(generators_missing_body_deviation, 6);
    assert_eq!(runtime_status_deviations, 26);
    assert_eq!(
        byte_pinned
            + corrupt_semantic
            + established_error_envelope_fallback
            + superseded_presentation
            + no_slash_deviations
            + generators_missing_body_deviation
            + runtime_status_deviations,
        448
    );
}

#[test]
fn fixture_body_arms_are_361_55_32_across_all_448_cases() {
    let corpus = corpus();
    let phases = corpus["phases"].as_object().expect("phase map");
    let mut arms = [0; 3];
    let mut count = 0;
    let mut generators_missing_body_vectors = 0;
    for (phase, cases) in phases {
        for case in cases.as_array().expect("phase cases") {
            arms[body_arm(case)] += 1;
            count += 1;
            if case["method"] == "PUT"
                && case["path"] == "/app/thinking/api/generators"
                && case.get("request_json").is_none()
            {
                generators_missing_body_vectors += 1;
            }
        }
        if is_established_phase(phase) {
            assert_eq!(
                cases
                    .as_array()
                    .expect("phase cases")
                    .iter()
                    .filter(|case| case["method"] != "GET")
                    .count(),
                35,
                "{phase} non-GET cases"
            );
        }
    }
    assert_eq!(count, 448);
    assert_eq!(arms, [361, 55, 32]);
    assert_eq!(arms.iter().sum::<usize>(), 448);
    assert_eq!(generators_missing_body_vectors, 8);
    assert_eq!(
        corpus["native_deviations"]
            .as_array()
            .expect("deviations")
            .len(),
        3
    );
}

#[tokio::test]
async fn copy_payload_round_trips_from_api_state() {
    let corpus = corpus();
    let journal = journal_for_phase("none");
    let response = request(router(journal.0.clone()), "GET", "/app/thinking/api/state").await;
    let actual: Value = serde_json::from_slice(&response.3).expect("state is JSON");
    let expected = corpus["phases"]["none"]
        .as_array()
        .expect("none cases")
        .iter()
        .find(|case| case["path"] == "/app/thinking/api/state")
        .expect("state case");
    assert_eq!(actual["copy"], expected["json"]["copy"]);
}

#[tokio::test]
async fn invalid_brain_record_degrades_the_brain_read_projections() {
    let journal = journal_for_phase("byo_cloud");
    let path = journal.0.join("health/brain.json");
    let mut invalid: Value = serde_json::from_slice(&fs::read(&path).expect("seed record reads"))
        .expect("seed record is JSON");
    invalid["fingerprint_sha256"] = Value::String("x".repeat(64));
    fs::write(
        &path,
        serde_json::to_vec(&invalid).expect("invalid record serializes"),
    )
    .expect("invalid record writes");
    for path in ["/app/thinking/api/state", "/app/thinking/api/providers"] {
        let response = request(router(journal.0.clone()), "GET", path).await;
        let body: Value = serde_json::from_slice(&response.3).expect("projection is JSON");
        let brain = if path.ends_with("state") {
            &body["providers"]["brain"]
        } else {
            &body["brain"]
        };
        assert_eq!(brain["state"], "unknown", "{path}");
        assert_eq!(brain["reason_code"], "brain_record_invalid", "{path}");
    }
    let response = request(
        router(journal.0.clone()),
        "GET",
        "/app/thinking/api/providers/local/status",
    )
    .await;
    let body: Value = serde_json::from_slice(&response.3).expect("local status is JSON");
    assert_eq!(body["generate_ready"], false);
    assert_eq!(body["cogitate_ready"], false);
}

#[tokio::test]
async fn post_keys_check_refusals_have_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    for request_json in [
        None,
        Some(json!({"env_var":"OPENAI_API_KEY","value":""})),
        Some(json!({"env_var":"bogus","value":"x"})),
    ] {
        let response = request_with_body(
            router(journal.0.clone()),
            "POST",
            "/app/thinking/api/keys/check",
            request_json.as_ref(),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
        assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
    }
}

#[tokio::test]
async fn post_validate_model_missing_key_has_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/validate-model",
        Some(&json!({"provider":"openai","model":"gpt-5"})),
    )
    .await;
    assert_eq!(response.0, StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(
        &body,
        vec!["message", "model", "provider", "reason_code", "valid"],
    );

    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/validate-model",
        Some(&json!({"provider":"local","model":"m"})),
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
}

#[tokio::test]
async fn post_local_bootstrap_byo_refusal_has_exact_top_level_keys() {
    let journal = TempDir::new("bootstrap-byo");
    journal.config(established(json!({
        "providers": {
            "local": {
                "endpoint_url": "http://127.0.0.1:1/v1",
                "served_model_id": "served-model"
            }
        }
    })));
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/local/bootstrap",
        None,
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
}

#[tokio::test]
async fn post_local_runtime_retry_refusal_has_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/local/runtime/retry",
        Some(&json!({"health_revision":1})),
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
}

#[tokio::test]
async fn post_brain_check_without_callosum_has_exact_top_level_keys() {
    let journal = journal_for_phase("none");
    let response = request_with_body(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/brain/check",
        None,
    )
    .await;
    assert_eq!(response.0, StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.3).expect("response is JSON");
    assert_top_level_keys(&body, vec!["brain", "error", "ok"]);
}

struct PanicValidator;
impl solstone_core_thinking::providers::ManagedKeyValidator for PanicValidator {
    fn validate(&self, _provider: &str, _key: &str) -> Result<Value, String> {
        panic!("managed-provider validation must be stubbed")
    }
}
struct FailureValidator;
impl solstone_core_thinking::providers::ManagedKeyValidator for FailureValidator {
    fn validate(&self, provider: &str, _key: &str) -> Result<Value, String> {
        Ok(
            json!({"valid":false,"reason_code":"provider_rejected","error":format!("{provider} rejected the key")}),
        )
    }
}

#[tokio::test]
async fn validate_keys_is_non_persisting_and_has_the_exact_contract_shape() {
    let zero = Map::new();
    assert_eq!(
        solstone_core_thinking::providers::validate_keys_with(&zero, &PanicValidator),
        json!({"key_validation":{}})
    );
    for env in [
        json!({"OPENAI_API_KEY":"one"}),
        json!({"GOOGLE_API_KEY":"one","OPENAI_API_KEY":"two","ANTHROPIC_API_KEY":"three"}),
    ] {
        let journal = TempDir::new("validate-keys");
        journal.config(established(json!({"env":env})));
        let before =
            fs::read(journal.0.join("config/journal.json")).expect("config reads before GET");
        let response = request(
            router(journal.0.clone()),
            "GET",
            "/app/thinking/api/validate-keys",
        )
        .await;
        assert_eq!(response.0, StatusCode::OK);
        let body: Value = serde_json::from_slice(&response.3).expect("validation is JSON");
        assert_eq!(
            body.as_object()
                .expect("contract object")
                .keys()
                .collect::<Vec<_>>(),
            ["key_validation"]
        );
        let validation = body["key_validation"]
            .as_object()
            .expect("validation object");
        assert_eq!(validation.len(), env.as_object().expect("env object").len());
        for result in validation.values() {
            assert!(result.get("reason_code").is_some());
        }
        assert_eq!(
            fs::read(journal.0.join("config/journal.json")).expect("config reads after GET"),
            before
        );
    }
    let one = Map::from_iter([(String::from("env"), json!({"OPENAI_API_KEY":"one"}))]);
    assert_eq!(
        solstone_core_thinking::providers::validate_keys_with(&one, &FailureValidator)["key_validation"]
            ["openai"]["reason_code"],
        "provider_rejected"
    );
}

#[tokio::test]
async fn confidential_operations_are_router_scoped_and_report_a_live_busy_operation() {
    let first_journal = journal_for_phase("none");
    write_link_ca(&first_journal.0);
    let second_journal = journal_for_phase("confidential_inactive");
    let (started_sender, started_receiver) = channel();
    let (release_sender, release_receiver) = channel();
    let first = router_with_runtime(
        first_journal.0.clone(),
        "https://portal.example/",
        Arc::new(ParkedPoll {
            started: started_sender,
            release: Mutex::new(release_receiver),
        }),
    );
    let second = router_with_runtime(
        second_journal.0.clone(),
        "https://portal.example/",
        Arc::new(PanicPoll),
    );

    let enable = request(
        first.clone(),
        "POST",
        "/app/thinking/api/confidential/enable",
    )
    .await;
    assert_eq!(enable.0, StatusCode::ACCEPTED);
    let enable_body: Value = serde_json::from_slice(&enable.3).expect("enable JSON");
    assert_eq!(enable_body["operation"]["phase"], "starting");
    assert_eq!(enable_body["operation"]["elapsed_ms"], 0);
    let url = enable_body["operation"]["portal_url"]
        .as_str()
        .expect("consent URL");
    assert!(url.starts_with("https://portal.example/enable/spp?nonce="));
    let (_, instance) = url.split_once("&instance=").expect("mandatory instance");
    assert_eq!(instance.len(), 36);
    assert_eq!(instance.matches('-').count(), 4);
    let nonce = url
        .split("?nonce=")
        .nth(1)
        .and_then(|value| value.split('&').next())
        .expect("nonce");
    assert_eq!(nonce.len(), 52);
    assert!(
        nonce
            .bytes()
            .all(|byte| b"23456789ABCDEFGHJKMNPQRSTUVWXYZ".contains(&byte))
    );
    // ⛔ This was a `yield_now()` loop, and a yield is NOT a synchronization
    // primitive: the operation runs on a runtime worker, so yielding the test's
    // own task guarantees nothing about the worker's progress. Measured flaky at
    // 1 pass / 5 fail. Wait on the channel with a real deadline instead.
    // ⛔ And NOT `recv_timeout` either: that is a BLOCKING std call, and blocking
    // inside an async test holds the runtime worker so the operation can never be
    // scheduled -- measured 0 pass / 6 fail, strictly worse than the yield loop,
    // which at least yielded. Await between attempts instead.
    let began_by = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match started_receiver.try_recv() {
            Ok(()) => break,
            Err(TryRecvError::Disconnected) => panic!("poll sender disconnected"),
            Err(TryRecvError::Empty) => {
                assert!(std::time::Instant::now() < began_by, "poll began");
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    }

    // ⚠ And observing that poll STARTED does not mean the operation's phase has
    // settled -- the fake signals at the top of `poll()`, so the transition to
    // "waiting" can land after. Poll for the state rather than assuming it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let providers_body: Value = loop {
        let providers = request(first.clone(), "GET", "/app/thinking/api/providers").await;
        let body: Value = serde_json::from_slice(&providers.3).expect("providers JSON");
        if body["active_lane"]["confidential_operation"]["phase"] == "waiting" {
            break body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "confidential operation never reached \"waiting\"; last phase: {}",
            body["active_lane"]["confidential_operation"]["phase"]
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    assert_eq!(
        providers_body["active_lane"]["confidential_operation"]["phase"],
        "waiting"
    );
    let updated = request_with_body(
        first.clone(),
        "PUT",
        "/app/thinking/api/providers",
        Some(&json!({"lane":"byo","provider":"openai"})),
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK);
    let updated_body: Value = serde_json::from_slice(&updated.3).expect("updated providers JSON");
    // ⛔ Comparing the two snapshots WHOLE was unsatisfiable: the operation is
    // live and `elapsed_ms` advances between the reads, so equality could only
    // hold if both landed in the same millisecond. Measured failing on 26 vs 28.
    // ✅ The claim is that the PUT does not DISTURB the operation, and an
    // advancing clock is the operation working, not a disturbance -- so compare
    // the stable identity and assert the clock only moves forward.
    let strip_clock = |value: &Value| {
        let mut operation = value["active_lane"]["confidential_operation"].clone();
        operation
            .as_object_mut()
            .expect("operation object")
            .remove("elapsed_ms");
        operation
    };
    assert_eq!(strip_clock(&updated_body), strip_clock(&providers_body));
    let elapsed_of = |value: &Value| {
        value["active_lane"]["confidential_operation"]["elapsed_ms"]
            .as_u64()
            .expect("elapsed_ms")
    };
    assert!(
        elapsed_of(&updated_body) >= elapsed_of(&providers_body),
        "elapsed_ms must not go backwards across the PUT"
    );
    let busy = request(
        first.clone(),
        "POST",
        "/app/thinking/api/confidential/enable",
    )
    .await;
    assert_eq!(busy.0, StatusCode::SERVICE_UNAVAILABLE);
    let busy_body: Value = serde_json::from_slice(&busy.3).expect("busy JSON");
    assert_top_level_keys(&busy_body, vec!["detail", "error", "reason_code"]);
    assert_eq!(
        busy_body["error"],
        "The service operation is already running. Try again in a moment."
    );
    assert_eq!(busy_body["reason_code"], "service_busy");
    assert_eq!(busy_body["detail"], "operation already running");

    let second_providers = request(second, "GET", "/app/thinking/api/providers").await;
    let second_body: Value =
        serde_json::from_slice(&second_providers.3).expect("second providers JSON");
    assert_eq!(
        second_body["active_lane"]["confidential_operation"],
        Value::Null
    );
    let cases = corpus();
    let case = &cases["phases"]["confidential_inactive"][5];
    replay_full_recorded_case(
        "confidential_inactive",
        5,
        &second_journal.0,
        case,
        &second_providers,
    );
    release_sender.send(()).expect("release poll");
}

#[tokio::test]
async fn confidential_enable_reads_identity_without_mutating_link_state() {
    let journal = journal_for_phase("none");
    write_link_ca(&journal.0);
    let app = router_with_runtime(
        journal.0.clone(),
        "https://portal.example",
        Arc::new(EarlyAccessPoll),
    );

    let before = link_snapshot(&journal.0);
    let first = request(app.clone(), "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    let (derived, first_nonce) = consent_identity_and_nonce(&first);
    assert_eq!(link_snapshot(&journal.0), before);
    wait_for_operation_phase(app.clone(), "early_access").await;

    let second = request(app.clone(), "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(second.0, StatusCode::ACCEPTED);
    let (second_identity, second_nonce) = consent_identity_and_nonce(&second);
    assert_eq!(second_identity, derived);
    assert_ne!(second_nonce, first_nonce);
    assert_eq!(link_snapshot(&journal.0), before);
    wait_for_operation_phase(app.clone(), "early_access").await;

    write_link_state(&journal.0, &derived);
    let before = link_snapshot(&journal.0);
    let matching = request(app.clone(), "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(matching.0, StatusCode::ACCEPTED);
    assert_eq!(consent_identity_and_nonce(&matching).0, derived);
    assert_eq!(link_snapshot(&journal.0), before);
    wait_for_operation_phase(app.clone(), "early_access").await;

    let drifted = "11111111-1111-8111-8111-111111111111";
    write_link_state(&journal.0, drifted);
    let before = link_snapshot(&journal.0);
    let repaired = request(app, "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(repaired.0, StatusCode::ACCEPTED);
    assert_eq!(consent_identity_and_nonce(&repaired).0, derived);
    assert_ne!(consent_identity_and_nonce(&repaired).0, drifted);
    assert_eq!(link_snapshot(&journal.0), before);

    let fallback_journal = journal_for_phase("none");
    let stored = "22222222-2222-8222-8222-222222222222";
    write_link_state(&fallback_journal.0, stored);
    let fallback = router_with_runtime(
        fallback_journal.0.clone(),
        "https://portal.example",
        Arc::new(EarlyAccessPoll),
    );
    let before = link_snapshot(&fallback_journal.0);
    let response = request(fallback, "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(response.0, StatusCode::ACCEPTED);
    assert_eq!(consent_identity_and_nonce(&response).0, stored);
    assert_eq!(link_snapshot(&fallback_journal.0), before);
}

#[tokio::test]
async fn confidential_enable_refuses_missing_identity_without_starting_an_operation() {
    let journal = journal_for_phase("none");
    let app = router(journal.0.clone());
    let before = link_snapshot(&journal.0);
    let response = request(app.clone(), "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(response.0, StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = serde_json::from_slice(&response.3).expect("refusal JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
    assert_eq!(body["error"], "I couldn't save those settings.");
    assert_eq!(body["reason_code"], "settings_operation_failed");
    assert_eq!(
        body["detail"],
        "something went wrong - try again, and if it persists, check the health dashboard"
    );
    assert_eq!(link_snapshot(&journal.0), before);
    let providers = request(app, "GET", "/app/thinking/api/providers").await;
    let providers_body: Value = serde_json::from_slice(&providers.3).expect("providers JSON");
    assert_eq!(
        providers_body["active_lane"]["confidential_operation"],
        Value::Null
    );
}

#[tokio::test]
async fn confidential_handoff_provisions_and_disable_restores_the_prior_provider() {
    let journal = TempDir::new("confidential-provision");
    let prior_local = json!({
        "endpoint_url": "https://prior.example/v1",
        "served_model_id": "prior-model",
        "credential": "prior-credential",
        "unrelated": "preserved",
    });
    let prior_active = json!({"provider": "openai", "model": "gpt-5"});
    journal.config(established(json!({
        "providers": {"local": prior_local.clone(), "active": prior_active.clone()},
        "services": {"other": {"kept": true}},
    })));
    write_link_ca(&journal.0);
    let handoff = json!({
        "endpoint_url": "https://handoff.example/v1",
        "served_model_id": "handoff-model",
        "credential": "handoff-credential",
        "account_id": "account",
        "created_at": "2026-01-01T00:00:00+00:00",
    })
    .as_object()
    .expect("handoff object")
    .clone();
    let app = router_with_runtime(
        journal.0.clone(),
        "https://portal.example",
        Arc::new(SuccessPoll { payload: handoff }),
    );

    let enable = request(app.clone(), "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(enable.0, StatusCode::ACCEPTED);
    wait_for_operation_phase(app.clone(), "not_verified").await;
    let provisioned = solstone_core_thinking::read_config(&journal.0).expect("config reads");
    assert_eq!(
        provisioned["providers"]["active"],
        json!({"provider": "local", "model": "local/qwen3.5-4b"})
    );
    assert_eq!(
        provisioned["providers"]["local"],
        json!({
            "endpoint_url": "https://handoff.example",
            "served_model_id": "handoff-model",
            "credential": "handoff-credential",
            "unrelated": "preserved",
        })
    );
    assert_eq!(
        provisioned["services"]["confidential"]["prior_active"],
        prior_active
    );
    assert_eq!(
        provisioned["services"]["confidential"]["prior_local_endpoint"],
        prior_local
    );

    let disable = request(app, "POST", "/app/thinking/api/confidential/disable").await;
    assert_eq!(disable.0, StatusCode::OK);
    let disable_body: Value = serde_json::from_slice(&disable.3).expect("disable JSON");
    assert_eq!(
        disable_body["result"],
        json!({"was_enabled": true, "credential_preserved": false})
    );
    let restored = solstone_core_thinking::read_config(&journal.0).expect("config rereads");
    assert_eq!(restored["providers"]["active"], prior_active);
    assert_eq!(restored["providers"]["local"], prior_local);
    assert_eq!(restored["services"], json!({"other": {"kept": true}}));
}

#[tokio::test]
async fn confidential_worker_panic_is_supervised_and_allows_a_later_enable() {
    let journal = journal_for_phase("none");
    write_link_ca(&journal.0);
    let app = router_with_runtime(
        journal.0.clone(),
        "https://portal.example",
        Arc::new(PanicPoll),
    );
    let first = request(app.clone(), "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    let mut body = Value::Null;
    for _ in 0..20 {
        tokio::task::yield_now().await;
        let response = request(app.clone(), "GET", "/app/thinking/api/providers").await;
        body = serde_json::from_slice(&response.3).expect("providers JSON");
        if body["active_lane"]["confidential_operation"]["phase"] == "repair_needed" {
            break;
        }
    }
    assert_eq!(
        body["active_lane"]["confidential_operation"]["phase"],
        "repair_needed"
    );
    assert_eq!(
        body["active_lane"]["confidential_operation"]["retryable"],
        true
    );
    let second = request(app, "POST", "/app/thinking/api/confidential/enable").await;
    assert_eq!(second.0, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn confidential_disable_and_recheck_refusals_preserve_the_exact_envelope() {
    let journal = journal_for_phase("none");
    let before = fs::read(journal.0.join("config/journal.json")).expect("config reads");
    let disable = request(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/confidential/disable",
    )
    .await;
    assert_eq!(disable.0, StatusCode::OK);
    let disable_body: Value = serde_json::from_slice(&disable.3).expect("disable JSON");
    assert_eq!(
        disable_body,
        json!({
            "success": true,
            "service": "spp",
            "result": {"was_enabled": false, "credential_preserved": false},
        })
    );
    assert_eq!(
        fs::read(journal.0.join("config/journal.json")).expect("config rereads"),
        before
    );

    let recheck = request(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/confidential/recheck",
    )
    .await;
    assert_eq!(recheck.0, StatusCode::BAD_REQUEST);
    let recheck_body: Value = serde_json::from_slice(&recheck.3).expect("recheck JSON");
    assert_top_level_keys(&recheck_body, vec!["detail", "error", "reason_code"]);
    assert_eq!(recheck_body["reason_code"], "invalid_operation_for_state");
    assert_eq!(
        recheck_body["detail"],
        "confidential processing is not active."
    );

    let configured = journal_for_phase("confidential_inactive");
    let enable = request(
        router(configured.0.clone()),
        "POST",
        "/app/thinking/api/confidential/enable",
    )
    .await;
    assert_eq!(enable.0, StatusCode::BAD_REQUEST);
    let enable_body: Value = serde_json::from_slice(&enable.3).expect("enable JSON");
    assert_top_level_keys(&enable_body, vec!["detail", "error", "reason_code"]);
    assert_eq!(enable_body["reason_code"], "invalid_operation_for_state");
    assert_eq!(
        enable_body["detail"],
        "confidential processing is already set up."
    );
}

#[tokio::test]
async fn confidential_disable_config_lock_refusal_has_the_exact_envelope() {
    let journal = journal_for_phase("none");
    fs::create_dir(journal.0.join("config/journal.json.lock")).expect("lock obstruction creates");
    let response = request(
        router(journal.0.clone()),
        "POST",
        "/app/thinking/api/confidential/disable",
    )
    .await;
    assert_eq!(response.0, StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = serde_json::from_slice(&response.3).expect("refusal JSON");
    assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
    assert_eq!(body["reason_code"], "config_busy");
    assert_eq!(
        body["error"],
        "I couldn't save those settings right now because they were busy. Try again in a moment."
    );
    assert_eq!(body["detail"], "settings are busy; try again");
}

#[tokio::test]
async fn provider_runtime_projection_derives_retry_state_in_both_directions() {
    let journal = journal_for_phase("none");
    write_runtime_health(&journal.0, 7, "failed", Value::Null);
    let app = router(journal.0.clone());
    let first = request(app.clone(), "GET", "/app/thinking/api/providers").await;
    let first_body: Value = serde_json::from_slice(&first.3).expect("providers JSON");
    let first_runtime = &first_body["local_runtime"];
    assert_eq!(first_runtime["health_revision"], 7);
    assert_eq!(first_runtime["retry_revision"], 0);
    assert_eq!(first_runtime["retry_pending"], false);
    assert_eq!(first_runtime["can_retry"], true);
    assert_eq!(first_runtime["poll"], false);

    write_runtime_health(
        &journal.0,
        8,
        "retry-requested",
        json!("retry-token-requested"),
    );
    write_runtime_retry(&journal.0, 4);
    let second = request(app, "GET", "/app/thinking/api/providers").await;
    let second_body: Value = serde_json::from_slice(&second.3).expect("providers JSON");
    let second_runtime = &second_body["local_runtime"];
    assert_eq!(second_runtime["phase"], "retry-requested");
    assert_eq!(second_runtime["reason_code"], "retry-token-requested");
    assert_eq!(second_runtime["health_revision"], 8);
    assert_eq!(second_runtime["retry_revision"], 4);
    assert_eq!(second_runtime["retry_pending"], true);
    assert_eq!(second_runtime["can_retry"], false);
    assert_eq!(second_runtime["poll"], true);
}

#[tokio::test]
async fn confidential_routes_are_session_gated_and_registered_404s_stay_unchanged() {
    let sibling = "/app/thinking/api/brain/check";
    for path in [
        "/app/thinking/api/confidential/enable",
        "/app/thinking/api/confidential/disable",
        "/app/thinking/api/confidential/recheck",
    ] {
        let unestablished = TempDir::new("confidential-unestablished");
        let response = request(router(unestablished.0.clone()), "POST", path).await;
        let sibling_response = request(router(unestablished.0.clone()), "POST", sibling).await;
        assert_eq!(response.0, StatusCode::FOUND, "{path} unestablished");
        assert_eq!(
            response.0, sibling_response.0,
            "{path} unestablished sibling"
        );
        assert_eq!(response.2.as_deref(), Some("/init"));
        let corrupt = TempDir::new("confidential-corrupt");
        corrupt.corrupt_config();
        let response = request(router(corrupt.0.clone()), "POST", path).await;
        let sibling_response = request(router(corrupt.0.clone()), "POST", sibling).await;
        assert_eq!(
            response.0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path} corrupt"
        );
        assert_eq!(response.0, sibling_response.0, "{path} corrupt sibling");
        let body: Value = serde_json::from_slice(&response.3).expect("corrupt envelope JSON");
        assert_top_level_keys(&body, vec!["detail", "error", "reason_code"]);
        assert_eq!(body["reason_code"], "corrupt_config");
    }
    for phase in [
        "none",
        "bundled_local",
        "byo_cloud",
        "byo_endpoint",
        "confidential_inactive",
        "confidential",
    ] {
        let journal = journal_for_phase(phase);
        for path in [
            "/app/thinking/background",
            "/app/thinking/static/nope.js",
            "/app/thinking/static/../../../etc/passwd",
        ] {
            let response = request(router(journal.0.clone()), "GET", path).await;
            assert_eq!(response.0, StatusCode::NOT_FOUND, "{phase} {path}");
        }
    }
}

#[test]
fn thinking_conversion_is_explicit_at_the_catch_all_boundary() {
    let shell = include_str!("../src/lib.rs");
    let registry = include_str!("../src/registry.rs");
    assert_eq!(
        shell
            .matches("Some(definition) if definition.converted => not_found_response(),")
            .count(),
        1
    );
    assert!(!shell.contains("struct ShellApp {\n    pub converted"));
    let thinking = registry
        .split("name: \"thinking\",")
        .nth(1)
        .and_then(|tail| tail.split("    },\n    AppDefinition").next())
        .expect("thinking registry entry");
    assert!(thinking.contains("converted: true"));
}
