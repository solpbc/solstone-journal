// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_shell::router;
use solstone_core_sol_link::ca::{jid_from_spki, load_ca};
use solstone_core_sol_link::pairing::addresses::PairingSnapshot;
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
        "established" => {
            journal.config(br#"{"link":{"posture":"direct"},"setup":{"completed_at":1}}"#);
            let ca_dir = journal.0.join("link/ca");
            fs::create_dir_all(&ca_dir).expect("CA directory creates");
            fs::write(
                ca_dir.join("cert.pem"),
                include_bytes!("../../../fixtures/convey_network_corpus_ca_nonproduction/cert.pem"),
            )
            .expect("CA certificate writes");
            fs::write(
                ca_dir.join("private.pem"),
                include_bytes!(
                    "../../../fixtures/convey_network_corpus_ca_nonproduction/private.pem"
                ),
            )
            .expect("CA private key writes");
            let ca = load_ca(
                std::str::from_utf8(include_bytes!(
                    "../../../fixtures/convey_network_corpus_ca_nonproduction/cert.pem"
                ))
                .expect("certificate UTF-8"),
                std::str::from_utf8(include_bytes!(
                    "../../../fixtures/convey_network_corpus_ca_nonproduction/private.pem"
                ))
                .expect("private key UTF-8"),
            )
            .expect("fixed CA loads");
            fs::write(
                journal.0.join("link/state.json"),
                serde_json::json!({
                    "instance_id": jid_from_spki(ca.spki_der()).expect("fixed CA JID"),
                    "home_label": "Network Corpus",
                })
                .to_string(),
            )
            .expect("link state writes");
        }
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
    let mut request = request.body(Body::empty()).expect("request builds");
    request.extensions_mut().insert(AccessBasis::Localhost);
    request.extensions_mut().insert(PairingSnapshot::default());
    let response = app.oneshot(request).await.expect("router responds");
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

#[tokio::test]
async fn network_corpus_replays_gated_cases_without_native_deferrals() {
    let corpus = corpus();
    let deferred_asserted = corpus["established_deferred_native_responses"]
        .as_array()
        .expect("deferred responses are an array")
        .len();
    assert_eq!(deferred_asserted, 0, "all captured routes are native");
    let mut asserted = 0;

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
            } else if phase == "established" && path == "/app/network/" {
                // The shared status/navigation presentation has deliberately changed.
                assert_eq!(response.3, include_bytes!("../assets/static/shell.html"));
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
    assert_eq!(
        deferred_asserted, 0,
        "every captured route is asserted natively"
    );
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

#[tokio::test]
async fn link_prefix_nonce_status_rejects_post() {
    let journal = journal_for_phase("established");
    let response = router(journal.0.clone())
        .oneshot(
            Request::post("/app/link/api/pair/nonce-status?nonce=corpus-nonce")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
