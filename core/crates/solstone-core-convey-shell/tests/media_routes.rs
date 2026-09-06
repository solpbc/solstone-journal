// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
use crate::support;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct EmptyEstablishedJournal(PathBuf);

impl EmptyEstablishedJournal {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-media-routes-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary journal creates");
        fs::create_dir(path.join("config")).expect("config directory creates");
        fs::write(
            path.join("config/journal.json"),
            br#"{"setup":{"completed_at":1767225600}}"#,
        )
        .expect("journal config writes");
        Self(path)
    }
}

impl Drop for EmptyEstablishedJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn speakers_cases() -> Vec<Value> {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../fixtures/convey_speakers_corpus.json"
    ))
    .expect("speakers corpus parses");
    corpus["cases"]
        .as_array()
        .expect("cases are an array")
        .clone()
}

async fn request(
    app: axum::Router,
    path: &str,
    headers: &serde_json::Map<String, Value>,
) -> (u16, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::get(path);
    for (name, value) in headers {
        request = request.header(name, value.as_str().expect("request header is text"));
    }
    let response = app
        .oneshot(request.body(Body::empty()).expect("request builds"))
        .await
        .expect("router responds");
    let status = response.status().as_u16();
    let response_headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads")
        .to_vec();
    (status, response_headers, body)
}

async fn assert_json_case(app: axum::Router, expected: &Value) {
    let headers = expected["request_headers"]
        .as_object()
        .expect("request headers are an object");
    let (status, actual_headers, body) = request(
        app,
        expected["path"].as_str().expect("path is text"),
        headers,
    )
    .await;
    assert_eq!(status, expected["status"], "{}", expected["path"]);
    assert_eq!(actual_headers["content-type"], "application/json");
    let actual: Value = serde_json::from_slice(&body).expect("response JSON parses");
    assert_eq!(actual, expected["json"], "{}", expected["path"]);
}

#[tokio::test]
async fn people_search_matches_every_populated_corpus_case() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for expected in speakers_cases().iter().filter(|case| {
        case["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("/app/speakers/api/people/search"))
    }) {
        assert_json_case(app.clone(), expected).await;
    }
}

#[tokio::test]
async fn people_search_is_empty_for_an_established_empty_journal() {
    let journal = EmptyEstablishedJournal::new();
    let (status, headers, body) = request(
        router(journal.0.clone()),
        "/app/speakers/api/people/search?q=anyone",
        &serde_json::Map::new(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("response JSON parses"),
        serde_json::json!({"query": "anyone", "people": []})
    );
}

#[tokio::test]
async fn serve_audio_matches_every_captured_case_except_the_declared_refusal() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for expected in speakers_cases().iter().filter(|case| {
        case["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("/app/speakers/api/serve_audio"))
    }) {
        let path = expected["path"].as_str().expect("path is text");
        let request_headers = expected["request_headers"]
            .as_object()
            .expect("request headers are an object");
        let (status, headers, body) = request(app.clone(), path, request_headers).await;
        if path.ends_with("mic_audio.xyz") {
            assert_eq!(status, 400);
            assert_eq!(headers["content-type"], "application/json");
            assert_eq!(
                serde_json::from_slice::<Value>(&body).expect("response JSON parses"),
                serde_json::json!({
                    "error": "one of those values couldn't be used.",
                    "reason_code": "invalid_request_value",
                    "detail": "Unregistered media extension",
                })
            );
            continue;
        }
        assert_eq!(status, expected["status"], "{path}");
        for (name, value) in expected["headers"]
            .as_object()
            .expect("headers are an object")
        {
            assert_eq!(
                headers[name],
                value.as_str().expect("header is text"),
                "{path}: {name}"
            );
        }
        assert_eq!(
            body.len(),
            expected["body_bytes"].as_u64().expect("body byte count") as usize,
            "{path}"
        );
        if expected.get("json").is_some() {
            assert_eq!(
                serde_json::from_slice::<Value>(&body).expect("response JSON parses"),
                expected["json"],
                "{path}"
            );
        } else {
            assert_eq!(
                format!("{:x}", Sha256::digest(&body)),
                expected["body_sha256"].as_str().expect("body hash"),
                "{path}"
            );
        }
    }
}
