// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempJournal(PathBuf);

impl TempJournal {
    fn new(phase: &str) -> Self {
        let serial = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-network-{phase}-{}-{nanos}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("journal creates");
        Self(path)
    }

    fn config(&self, body: &[u8]) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(self.0.join("config/journal.json"), body).expect("config writes");
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn corpus() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/convey_network_corpus.json"))
        .expect("network corpus parses")
}

fn journal_for_phase(phase: &str) -> TempJournal {
    let journal = TempJournal::new(phase);
    match phase {
        "unestablished" => {}
        "established" => journal.config(br#"{"setup":{"completed_at":1}}"#),
        "corrupt" => journal.config(br#"{"setup":{"completed_at":1"#),
        _ => panic!("unknown network corpus phase {phase}"),
    }
    journal
}

async fn request(
    app: axum::Router,
    method: &str,
    path: &str,
    headers: &serde_json::Map<String, Value>,
) -> (StatusCode, String, Option<String>, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(name, value.as_str().expect("header is text"));
    }
    let response = app
        .oneshot(request.body(Body::empty()).expect("request builds"))
        .await
        .expect("router responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("content type present")
        .to_str()
        .expect("content type is text")
        .to_owned();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|value| value.to_str().expect("location is text").to_owned());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads")
        .to_vec();
    (status, content_type, location, body)
}

fn deferred_cases(corpus: &Value) -> BTreeSet<(String, String)> {
    corpus["established_deferred_native_responses"]
        .as_array()
        .expect("deferred responses are an array")
        .iter()
        .map(|entry| {
            (
                entry["phase"].as_str().expect("deferred phase").to_owned(),
                entry["path"].as_str().expect("deferred path").to_owned(),
            )
        })
        .collect()
}

fn set_path(value: &mut Value, path: &str, replacement: &str, journal: &Path) {
    let mut current = value;
    let mut fields = path.split('.').peekable();
    while let Some(field) = fields.next() {
        if fields.peek().is_none() {
            let object = current.as_object_mut().expect("normalization object");
            if replacement == "<JOURNAL_ROOT>" {
                let text = object[field].as_str().expect("journal root field is text");
                object.insert(
                    field.to_owned(),
                    Value::String(text.replace(&journal.display().to_string(), replacement)),
                );
            } else {
                object.insert(field.to_owned(), Value::String(replacement.to_owned()));
            }
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
        "host" => "<HOST_DEPENDENT>",
        "capture-clock" => "<CAPTURE_CLOCK>",
        "journal-root" => "<JOURNAL_ROOT>",
        other => panic!("unknown normalization kind {other}"),
    }
}

fn normalized_json(body: &[u8], fields: &serde_json::Map<String, Value>, journal: &Path) -> Value {
    let mut value: Value = serde_json::from_slice(body).expect("JSON response parses");
    for (path, kind) in fields {
        set_path(
            &mut value,
            path,
            placeholder(kind.as_str().expect("normalization kind")),
            journal,
        );
    }
    value
}

fn normalized_raw(body: &[u8], fields: &serde_json::Map<String, Value>, journal: &Path) -> Vec<u8> {
    match fields.get("body").and_then(Value::as_str) {
        None => body.to_vec(),
        Some("journal-root") => String::from_utf8(body.to_vec())
            .expect("journal-root body is text")
            .replace(&journal.display().to_string(), "<JOURNAL_ROOT>")
            .into_bytes(),
        Some(other) => panic!("unsupported raw normalization {other}"),
    }
}

fn assert_not_converted(response: &(StatusCode, String, Option<String>, Vec<u8>), path: &str) {
    assert_eq!(response.0, StatusCode::NOT_IMPLEMENTED, "{path}");
    assert_eq!(response.1, "application/json", "{path}");
    let body: Value = serde_json::from_slice(&response.3).expect("native refusal is JSON");
    assert_eq!(
        body,
        json!({
            "error": "This app isn't available yet.",
            "reason_code": "app_not_converted",
            "detail": "The network app has not been ported to the native shell.",
            "app": "network",
        }),
        "{path}"
    );
}

#[tokio::test]
async fn network_corpus_replays_gated_cases_and_asserts_native_deferrals() {
    let corpus = corpus();
    let deferred = deferred_cases(&corpus);
    assert_eq!(
        deferred.len(),
        6,
        "all six established routes defer natively"
    );
    let mut asserted = 0;
    let mut deferred_asserted = 0;

    for (phase, cases) in corpus["phases"].as_object().expect("phases are object") {
        let journal = journal_for_phase(phase);
        let app = router(journal.0.clone());
        for case in cases.as_array().expect("phase cases are an array") {
            let path = case["path"].as_str().expect("case path");
            let response = request(
                app.clone(),
                case["method"].as_str().expect("case method"),
                path,
                case["request_headers"]
                    .as_object()
                    .expect("request headers are an object"),
            )
            .await;
            asserted += 1;
            if deferred.contains(&(phase.clone(), path.to_owned())) {
                assert_not_converted(&response, path);
                deferred_asserted += 1;
                continue;
            }
            assert_eq!(
                response.0.as_u16(),
                case["status"].as_u64().expect("status") as u16,
                "{phase} {path}"
            );
            assert_eq!(
                response.1,
                case["content_type"].as_str().expect("content type"),
                "{phase} {path}"
            );
            assert_eq!(
                response.2.as_deref(),
                case.get("location").and_then(Value::as_str),
                "{phase} {path}"
            );
            let fields = case["normalized_fields"]
                .as_object()
                .expect("normalized fields are an object");
            if response.1.contains("json") {
                assert_eq!(
                    normalized_json(&response.3, fields, &journal.0),
                    case["body"],
                    "{phase} {path} body"
                );
            } else {
                let body = normalized_raw(&response.3, fields, &journal.0);
                assert_eq!(
                    body.len(),
                    case["body"]["byte_length"].as_u64().expect("body length") as usize,
                    "{phase} {path} body length"
                );
                assert_eq!(
                    format!("{:x}", Sha256::digest(&body)),
                    case["body"]["digest"].as_str().expect("body digest"),
                    "{phase} {path} body digest"
                );
            }
        }
    }
    assert_eq!(asserted, 18, "every fixture case issues an assertion");
    assert_eq!(deferred_asserted, 6, "every deferral is actively asserted");
}

#[tokio::test]
async fn network_nonce_status_rejects_post() {
    let journal = journal_for_phase("established");
    let response = router(journal.0.clone())
        .oneshot(
            Request::post("/app/network/api/pair/nonce-status?nonce=corpus-nonce")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
