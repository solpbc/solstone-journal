// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[path = "support/mod.rs"]
mod support;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::Value;
use solstone_core_convey_shell::router;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
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
            "solstone-calendar-routes-{}-{nanos}-{sequence}",
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

fn expected_case(path: &str) -> Value {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../fixtures/convey_speakers_corpus.json"
    ))
    .expect("speakers corpus parses");
    corpus["cases"]
        .as_array()
        .expect("cases are an array")
        .iter()
        .find(|case| case["path"] == path)
        .unwrap_or_else(|| panic!("missing corpus case for {path}"))
        .clone()
}

async fn assert_corpus_case(app: axum::Router, path: &str) {
    let expected = expected_case(path);
    let response = app
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status().as_u16(), expected["status"]);
    assert_eq!(response.headers()["content-type"], "application/json");
    let actual: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads"),
    )
    .expect("response JSON parses");
    assert_eq!(actual, expected["json"], "{path}");
}

fn expected_shell_case(path: &str) -> Value {
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

async fn assert_shell_corpus_case(app: axum::Router, path: &str) {
    let expected = expected_shell_case(path);
    let response = app
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status().as_u16(), expected["status"]);
    assert_eq!(
        response.headers()["content-type"],
        expected["content_type"]
            .as_str()
            .expect("content type is text")
    );
    let actual: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads"),
    )
    .expect("response JSON parses");
    assert_eq!(actual, expected["json"], "{path}");
}

#[tokio::test]
async fn index_matches_the_populated_speakers_corpus() {
    let journal = support::build_populated_journal();
    assert_corpus_case(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/index",
    )
    .await;
}

#[tokio::test]
async fn grid_matches_the_populated_speakers_corpus() {
    let journal = support::build_populated_journal();
    assert_corpus_case(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/grid",
    )
    .await;
}

#[tokio::test]
async fn stats_matches_the_populated_speakers_corpus() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for path in [
        "/app/speakers/api/stats/202607",
        "/app/speakers/api/stats/999999",
        "/app/speakers/api/stats/nope",
    ] {
        assert_corpus_case(app.clone(), path).await;
    }
}

#[tokio::test]
async fn segments_matches_the_populated_speakers_corpus() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for path in [
        "/app/speakers/api/segments/20260731",
        "/app/speakers/api/segments/20260731?limit=1",
        "/app/speakers/api/segments/20260731?limit=1&offset=1",
        "/app/speakers/api/segments/20260731?limit=notanint",
        "/app/speakers/api/segments/20260731?speaker=%20",
    ] {
        assert_corpus_case(app.clone(), path).await;
    }
}

#[tokio::test]
async fn stats_and_segments_match_the_empty_shell_corpus() {
    let journal = EmptyEstablishedJournal::new();
    let app = router(journal.0.clone());
    assert_shell_corpus_case(app.clone(), "/app/speakers/api/stats/202601").await;
    assert_shell_corpus_case(app, "/app/speakers/api/segments/20260101").await;
}
