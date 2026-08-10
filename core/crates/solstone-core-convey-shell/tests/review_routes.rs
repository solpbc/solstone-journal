// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[path = "support/mod.rs"]
mod support;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::Value;
use solstone_core_convey_shell::router;
use tower::ServiceExt;

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
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    let body = serde_json::from_slice(&body).expect("response JSON parses");
    (status, content_type, body)
}

async fn assert_case(app: axum::Router, path: &str) -> Value {
    let expected = speakers_case(path);
    let (status, content_type, actual) = request_json(app, path).await;
    assert_eq!(status, expected["status"], "{path}");
    assert_eq!(content_type, "application/json", "{path}");
    assert_eq!(actual, expected["json"], "{path}");
    actual
}

#[tokio::test]
async fn segment_speakers_matches_every_populated_speakers_corpus_case() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for path in [
        "/app/speakers/api/speakers/20260731/field/090000_300",
        "/app/speakers/api/speakers/20260729/field/173000_240",
        "/app/speakers/api/speakers/20260728/desk/080000_180",
    ] {
        assert_case(app.clone(), path).await;
    }
}

#[tokio::test]
async fn review_matches_every_populated_speakers_corpus_case() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for path in [
        "/app/speakers/api/review/20260731/field/090000_300/mic_audio",
        "/app/speakers/api/review/20260730/field/101500_120/mic_audio",
        "/app/speakers/api/review/20260729/field/173000_240/mic_audio",
        "/app/speakers/api/review/20260731/desk/140000_600/sys_audio",
        "/app/speakers/api/review/20260731/field/999999_999/mic_audio",
    ] {
        assert_case(app.clone(), path).await;
    }
}

#[tokio::test]
async fn malformed_transcript_preserves_sentence_id_gaps() {
    let journal = support::build_populated_journal();
    let actual = assert_case(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/review/20260731/desk/140000_600/sys_audio",
    )
    .await;
    let ids = actual["sentences"]
        .as_array()
        .expect("sentences are an array")
        .iter()
        .map(|sentence| sentence["id"].as_i64().expect("sentence has an ID"))
        .collect::<Vec<_>>();
    assert_eq!(ids, [1, 3, 4]);
}
