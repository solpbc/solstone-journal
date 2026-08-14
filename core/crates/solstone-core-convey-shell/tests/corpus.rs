// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::Local;
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_shell::authorization_gate::authorized_router;
use solstone_core_convey_shell::router;
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::AuthorizedClientsRead;
use tokio::sync::watch;
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const ESTABLISHED_DEFERRED: [&str; 0] = [];

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-convey-shell-{name}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary journal creates");
        Self(path)
    }

    fn write_config(&self, bytes: &[u8]) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(self.0.join("config/journal.json"), bytes).expect("config writes");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn corpus() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/convey_shell_corpus.json"))
        .expect("corpus parses")
}

/// Permanent documented divergence, introduced 2026-08-13, with no expiry
/// condition: the frozen corpus permanently records the deleted reference's
/// `observer`, while the shell permanently serves `devices`. Because this
/// cannot expire, narrowness is the safeguard: this is keyed to the one
/// renamed entry and exactly two keys. Never generalize this into a rule over
/// app names or other keys, and never retire it.
/// Permanent documented divergence, introduced 2026-08-14, with no expiry
/// condition: the frozen corpus permanently records the deleted reference's
/// `reflections` app, which was dropped by ruling and whose Python surface no
/// longer exists. The corpus CANNOT be regenerated -- its generator needs a
/// runnable reference tree and this wave removes it -- so the fixture is a
/// frozen record and the divergence is absorbed here instead. Because this
/// cannot expire, narrowness is the safeguard: it is keyed to the one dropped
/// entry and removes exactly that element. Never generalize this into a rule
/// over app names, and never retire it.
fn apply_permanent_reflections_drop_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    assert_eq!(
        apps.iter()
            .filter(|app| app["name"] == "reflections")
            .count(),
        1,
        "frozen shell contains exactly one reflections app"
    );
    apps.retain(|app| app["name"] != "reflections");
}

fn apply_permanent_devices_shell_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    assert_eq!(
        apps.iter().filter(|app| app["name"] == "observer").count(),
        1,
        "frozen shell contains exactly one observer app"
    );
    let observer = apps
        .iter_mut()
        .find(|app| app["name"] == "observer")
        .expect("frozen shell contains observer app");
    assert_eq!(observer["name"], "observer");
    assert_eq!(observer["workspace_url"], "/app/observer/workspace");
    observer["name"] = Value::String("devices".to_owned());
    observer["workspace_url"] = Value::String("/app/devices/workspace".to_owned());
}

fn journal_for_phase(phase: &str) -> TempDir {
    let journal = TempDir::new(phase);
    match phase {
        "unestablished" => {}
        "established" => journal.write_config(br#"{"setup":{"completed_at":1767225600}}"#),
        "corrupt" => journal.write_config(br#"{"setup":{"completed_at":17672256"#),
        _ => panic!("unknown corpus phase {phase}"),
    }
    journal
}

async fn get(app: axum::Router, path: &str) -> (StatusCode, String, Option<String>, Vec<u8>) {
    let response = app
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .expect("response content type")
        .to_str()
        .expect("content type is text")
        .to_owned();
    let location = response
        .headers()
        .get("location")
        .map(|value| value.to_str().expect("location is text").to_owned());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads")
        .to_vec();
    (status, content_type, location, body)
}

fn normalize(value: &mut Value, journal_root: &str, path: &str) {
    match value {
        Value::Object(object) => {
            for (key, item) in object {
                let next = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                normalize(item, journal_root, &next);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item, journal_root, path);
            }
        }
        Value::String(text) => {
            if text.contains(journal_root) {
                *text = text.replace(journal_root, "<JOURNAL_ROOT>");
            } else if path == "chat_bar.placeholder" {
                *text = "<CHAT_BAR_PLACEHOLDER>".to_owned();
            } else if text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_digit()) {
                *text = "<TODAY>".to_owned();
            } else if path == "version" && text.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                *text = "<VERSION>".to_owned();
            }
        }
        _ => {}
    }
}

fn normalize_body_root(body: &[u8], journal_root: &str) -> Vec<u8> {
    let needle = journal_root.as_bytes();
    let replacement = b"<JOURNAL_ROOT>";
    let mut normalized = Vec::with_capacity(body.len());
    let mut remaining = body;
    while let Some(index) = remaining
        .windows(needle.len())
        .position(|window| window == needle)
    {
        normalized.extend_from_slice(&remaining[..index]);
        normalized.extend_from_slice(replacement);
        remaining = &remaining[index + needle.len()..];
    }
    normalized.extend_from_slice(remaining);
    normalized
}

#[tokio::test]
async fn corpus_gate_and_converted_surface_match_all_non_deferred_cases() {
    let corpus = corpus();
    let phases = corpus["phases"].as_object().expect("phases are object");
    let mut established_asserted = 0;
    let mut established_deferred = 0;

    for (phase, cases) in phases {
        let journal = journal_for_phase(phase);
        let (_, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = authorized_router(journal.0.clone(), authorization)
            .into_inner()
            .layer(Extension(AccessBasis::Localhost));
        for case in cases.as_array().expect("phase cases are array") {
            let path = case["path"].as_str().expect("case path");
            if phase == "established" && ESTABLISHED_DEFERRED.contains(&path) {
                established_deferred += 1;
                continue;
            }
            if phase == "established" {
                established_asserted += 1;
            }
            let (status, content_type, location, body) = get(app.clone(), path).await;
            assert_eq!(
                status.as_u16(),
                case["status"].as_u64().unwrap() as u16,
                "{phase} {path}"
            );
            assert_eq!(
                content_type,
                case["content_type"].as_str().unwrap(),
                "{phase} {path}"
            );
            assert_eq!(
                location.as_deref(),
                case.get("location").and_then(Value::as_str),
                "{phase} {path}"
            );

            if let Some(expected_json) = case.get("json") {
                let mut actual: Value =
                    serde_json::from_slice(&body).expect("JSON response parses");
                let mut expected = expected_json.clone();
                if phase == "established" && path == "/api/shell" {
                    apply_permanent_devices_shell_divergence(&mut expected);
                    apply_permanent_reflections_drop_divergence(&mut expected);
                }
                normalize(&mut actual, &journal.0.display().to_string(), "");
                normalize(&mut expected, &journal.0.display().to_string(), "");
                assert_eq!(actual, expected, "{phase} {path}");
            } else {
                let body = if case.get("body_normalized").is_some() {
                    normalize_body_root(&body, &journal.0.display().to_string())
                } else {
                    body
                };
                let digest = format!("{:x}", Sha256::digest(&body));
                assert_eq!(
                    digest,
                    case["body_sha256"].as_str().unwrap(),
                    "{phase} {path}"
                );
            }
        }
    }

    assert_eq!(
        established_asserted, 18,
        "all 18 established probes are asserted"
    );
    assert_eq!(established_deferred, 0);
}

#[tokio::test]
async fn speakers_state_uses_the_python_local_date_semantics() {
    let journal = journal_for_phase("established");
    let (_, _, _, body) = get(router(journal.0.clone()), "/app/speakers/api/state").await;
    let state: Value = serde_json::from_slice(&body).expect("speakers state parses");
    assert_eq!(
        state["today"],
        Value::String(Local::now().format("%Y%m%d").to_string())
    );
    assert_eq!(state["speaker_copy"].as_object().unwrap().len(), 120);
}

#[tokio::test]
async fn registry_and_unconverted_refusal_contract_are_stable() {
    let journal = journal_for_phase("established");
    let (_, _, _, shell_body) = get(router(journal.0.clone()), "/api/shell").await;
    let shell: Value = serde_json::from_slice(&shell_body).expect("shell parses");
    let apps = shell["apps"].as_array().expect("apps array");
    assert_eq!(shell["chat_bar"]["placeholder"], "send a message…");
    assert_eq!(apps.len(), 22);
    for app in apps {
        assert_eq!(app.as_object().unwrap().len(), 10);
        assert!(app["icon_svg"].is_string());
    }
    let backgrounds: Vec<_> = apps
        .iter()
        .filter_map(|app| app["background_url"].as_str())
        .collect();
    assert_eq!(
        backgrounds,
        ["/app/support/background", "/app/timeline/background"]
    );

    let (status, content_type, _, body) =
        get(router(journal.0.clone()), "/app/home/workspace").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(content_type, "application/json");
    let refusal: Value = serde_json::from_slice(&body).expect("refusal parses");
    assert_eq!(refusal["reason_code"], "app_not_converted");
    assert_eq!(refusal["app"], "home");

    let (status, content_type, _, body) = get(router(journal.0.clone()), "/app/home/").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(content_type, "application/json");
    let refusal: Value = serde_json::from_slice(&body).expect("refusal parses");
    assert_eq!(refusal["reason_code"], "app_not_converted");
    assert_eq!(refusal["app"], "home");
}

#[tokio::test]
async fn sse_is_gated_and_exposes_a_heartbeat_after_establishment() {
    let unestablished = journal_for_phase("unestablished");
    let (status, _, location, _) = get(router(unestablished.0.clone()), "/sse/events").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(location.as_deref(), Some("/init"));

    let established = journal_for_phase("established");
    let response = router(established.0.clone())
        .oneshot(Request::get("/sse/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-cache");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
}

#[tokio::test]
async fn an_unconverted_app_refusal_is_never_a_success_status() {
    // Found in a browser, not a test: shell_boot.js only evaluates a response
    // when `response.ok`, so a refusal at 200 was parsed as JavaScript. Every
    // client branches on that bit -- pin it rather than the exact code.
    let journal = journal_for_phase("established");
    for path in [
        "/app/home/",
        "/app/home/workspace",
        "/app/home/background",
        // `/app/timeline/background` was here until 2026-08-14, when the
        // timeline conversion landed and made it a real 200 serving the app's
        // background fragment. The INVARIANT is unchanged -- an unconverted
        // app's refusal is never 2xx -- only this example went stale. Replaced
        // with a still-unconverted app rather than deleted, so the assertion
        // keeps covering a background path.
        "/app/sol/background",
    ] {
        let (status, _content_type, _location, body) = get(router(journal.0.clone()), path).await;
        assert!(
            !status.is_success(),
            "{path} returned a success status for an unconverted app: {status}"
        );
        let refusal: Value = serde_json::from_slice(&body)
            .unwrap_or_else(|_| panic!("{path} refusal parses as JSON"));
        assert_eq!(refusal["reason_code"], "app_not_converted", "{path}");
    }
}
