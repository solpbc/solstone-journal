// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
use crate::support;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
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
            "solstone-quality-known-routes-{}-{nanos}-{sequence}",
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

fn speakers_case(path: &str) -> Value {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../fixtures/convey_speakers_corpus.json"
    ))
    .expect("speakers corpus parses");
    corpus["cases"]
        .as_array()
        .expect("cases are an array")
        .iter()
        .find(|case| case["path"] == path)
        .unwrap_or_else(|| panic!("missing speakers corpus case for {path}"))
        .clone()
}

fn established_shell_case(path: &str) -> Value {
    let corpus: Value =
        serde_json::from_str(include_str!("../../../fixtures/convey_shell_corpus.json"))
            .expect("shell corpus parses");
    corpus["phases"]["established"]
        .as_array()
        .expect("established cases are an array")
        .iter()
        .find(|case| case["path"] == path)
        .unwrap_or_else(|| panic!("missing shell corpus case for {path}"))
        .clone()
}

#[tokio::test]
async fn quality_route_refuses_an_unconfigured_owner_identity() {
    let journal = EmptyEstablishedJournal::new();
    std::fs::create_dir_all(journal.0.join("chronicle/20260731/080000_300")).expect("direct");
    let response = router(journal.0.clone())
        .oneshot(
            Request::get("/app/speakers/api/quality")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response reads"),
    )
    .expect("response JSON parses");
    assert_eq!(body["reason_code"], "speaker_owner_identity_invalid");
}

async fn assert_case(app: axum::Router, path: &str, expected: Value) {
    let (status, content_type, actual) = request_json(app, path).await;
    assert_eq!(status, expected["status"], "{path}");
    assert_eq!(content_type, expected_content_type(&expected), "{path}");
    assert_eq!(actual, expected["json"], "{path}");
}

async fn assert_known_case(app: axum::Router, path: &str, expected: Value) {
    let (status, content_type, actual) = request_json(app, path).await;
    assert_eq!(status, expected["status"], "{path}");
    assert_eq!(content_type, expected_content_type(&expected), "{path}");
    if expected["json"].get("speakers").is_some() {
        assert_known_json(&actual, &expected["json"], path);
    } else {
        assert_eq!(actual, expected["json"], "{path}");
    }
}

async fn request_json(app: axum::Router, path: &str) -> (u16, String, Value) {
    let response = app
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status().as_u16();
    let content_type = response.headers()["content-type"]
        .to_str()
        .expect("content type is UTF-8")
        .to_owned();
    let actual: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads"),
    )
    .expect("response JSON parses");
    (status, content_type, actual)
}

fn expected_content_type(expected: &Value) -> &str {
    expected
        .get("content_type")
        .or_else(|| {
            expected
                .get("headers")
                .and_then(|headers| headers.get("Content-Type"))
        })
        .and_then(Value::as_str)
        .unwrap_or("application/json")
}

fn assert_known_json(actual: &Value, expected: &Value, path: &str) {
    let mut actual = actual
        .as_object()
        .expect("known response is an object")
        .clone();
    let mut expected = expected
        .as_object()
        .expect("known corpus response is an object")
        .clone();
    let actual_speakers = actual.remove("speakers").expect("actual speakers present");
    let expected_speakers = expected
        .remove("speakers")
        .expect("expected speakers present");
    assert_eq!(actual, expected, "{path}");
    let actual_speakers = actual_speakers
        .as_array()
        .expect("actual speakers is an array");
    let expected_speakers = expected_speakers
        .as_array()
        .expect("expected speakers is an array");
    assert_eq!(actual_speakers.len(), expected_speakers.len(), "{path}");
    for (actual, expected) in actual_speakers.iter().zip(expected_speakers) {
        let mut actual = actual
            .as_object()
            .expect("actual speaker is an object")
            .clone();
        let mut expected = expected
            .as_object()
            .expect("expected speaker is an object")
            .clone();
        let actual_p25 = actual.remove("intra_cosine_p25");
        let expected_p25 = expected.remove("intra_cosine_p25");
        assert_eq!(actual, expected, "{path}");
        match (actual_p25, expected_p25) {
            (Some(Value::Null), Some(Value::Null)) => {}
            (Some(actual), Some(expected)) => {
                let actual = actual.as_f64().expect("actual p25 is a number");
                let expected = expected.as_f64().expect("expected p25 is a number");
                assert!(
                    (actual - expected).abs() < 1e-4,
                    "{path}: p25 {actual} differs from {expected}"
                );
            }
            values => panic!("{path}: p25 nullability differs: {values:?}"),
        }
    }
}

#[tokio::test]
async fn quality_matches_the_populated_speakers_corpus() {
    let journal = support::build_populated_journal();
    assert_case(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/quality",
        speakers_case("/app/speakers/api/quality"),
    )
    .await;
}

#[tokio::test]
async fn known_voices_matches_every_populated_speakers_corpus_case() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for path in [
        "/app/speakers/api/speakers/known",
        "/app/speakers/api/speakers/known?sort=recent",
        "/app/speakers/api/speakers/known?sort=most_samples",
        "/app/speakers/api/speakers/known?sort=alphabetical",
        "/app/speakers/api/speakers/known?sort=",
        "/app/speakers/api/speakers/known?sort=bogus",
    ] {
        assert_known_case(app.clone(), path, speakers_case(path)).await;
    }
}

#[tokio::test]
async fn quality_and_known_voices_match_the_empty_established_shell_corpus() {
    let journal = EmptyEstablishedJournal::new();
    let app = router(journal.0.clone());
    let (status, _, body) = request_json(app.clone(), "/app/speakers/api/quality").await;
    assert_eq!(status, StatusCode::BAD_REQUEST.as_u16());
    assert_eq!(body["reason_code"], "speaker_owner_identity_invalid");
    assert_case(
        app,
        "/app/speakers/api/speakers/known",
        established_shell_case("/app/speakers/api/speakers/known"),
    )
    .await;
}

#[tokio::test]
async fn owner_status_matches_the_populated_corpus_and_refuses_empty_identity() {
    let journal = support::build_populated_journal();
    assert_case(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/owner/status",
        speakers_case("/app/speakers/api/owner/status"),
    )
    .await;

    let journal = EmptyEstablishedJournal::new();
    let (status, _, body) =
        request_json(router(journal.0.clone()), "/app/speakers/api/owner/status").await;
    assert_eq!(status, StatusCode::BAD_REQUEST.as_u16());
    assert_eq!(body["reason_code"], "speaker_owner_identity_invalid");
}
