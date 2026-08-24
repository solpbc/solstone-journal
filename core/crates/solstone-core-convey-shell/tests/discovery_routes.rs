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
            "solstone-discovery-routes-{}-{nanos}-{sequence}",
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
        .unwrap_or_else(|| panic!("missing established shell case for {path}"))
        .clone()
}

async fn assert_case(app: axum::Router, path: &str, expected: Value) -> Value {
    let response = app
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status().as_u16(), expected["status"], "{path}");
    let expected_content_type = expected
        .get("content_type")
        .or_else(|| {
            expected
                .get("headers")
                .and_then(|headers| headers.get("Content-Type"))
        })
        .and_then(Value::as_str)
        .unwrap_or("application/json");
    assert_eq!(
        response.headers()["content-type"],
        expected_content_type,
        "{path}"
    );
    let actual: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads"),
    )
    .expect("response JSON parses");
    assert_eq!(actual, expected["json"], "{path}");
    actual
}

#[tokio::test]
async fn discovery_presence_reads_a_direct_segment_member() {
    let journal = EmptyEstablishedJournal::new();
    let segment = journal.0.join("chronicle/20260731/080000_300");
    fs::create_dir_all(segment.join("talents")).expect("direct");
    fs::write(
        segment.join("mic_audio.jsonl"),
        "{\"raw\":\"mic_audio.flac\"}\n{\"id\":1,\"text\":\"hello\"}\n",
    )
    .expect("transcript");
    fs::write(segment.join("talents/speakers.json"), "[\"Ada\"]").expect("speakers");
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness");
    fs::write(
        journal.0.join("awareness/discovery_clusters.json"),
        serde_json::json!({
            "clusters": {
                "1": [{
                    "day": "20260731",
                    "stream": "_default",
                    "segment_key": "080000_300",
                    "source": "mic_audio",
                    "sentence_id": 1,
                    "stream_layout": "direct"
                }]
            }
        })
        .to_string(),
    )
    .expect("cache");
    let response = router(journal.0.clone())
        .oneshot(
            Request::get("/app/speakers/api/discovery/cluster/1/presence")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(body["cluster_id"], 1, "{body}");
    assert_eq!(body["facts"]["segment_count"], 1, "{body}");
}

#[tokio::test]
async fn discovery_cache_matches_populated_and_empty_corpora() {
    let journal = support::build_populated_journal();
    assert_case(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/discovery/cache",
        speakers_case("/app/speakers/api/discovery/cache"),
    )
    .await;

    let journal = EmptyEstablishedJournal::new();
    assert_case(
        router(journal.0.clone()),
        "/app/speakers/api/discovery/cache",
        established_shell_case("/app/speakers/api/discovery/cache"),
    )
    .await;
}

#[tokio::test]
async fn discovery_presence_matches_every_populated_corpus_case() {
    let journal = support::build_populated_journal();
    let app = router(journal.root().to_path_buf());
    for path in [
        "/app/speakers/api/discovery/cluster/1/presence",
        "/app/speakers/api/discovery/cluster/999/presence",
        "/app/speakers/api/discovery/cluster/-1/presence",
    ] {
        assert_case(app.clone(), path, speakers_case(path)).await;
    }
}

#[tokio::test]
async fn resolve_statement_finds_a_cached_member_without_an_oracle() {
    let journal = support::build_populated_journal();
    let expected = serde_json::json!({"status": "hit", "cluster_id": 1});
    let response = router(journal.root().to_path_buf())
        .oneshot(
            Request::get("/app/speakers/api/discovery/resolve-statement?voice_day=20260731&voice_stream=field&voice_segment_key=090000_300&voice_source=mic_audio&voice_sentence_id=1")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status().as_u16(), 200);
    let actual: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads"),
    )
    .expect("response JSON parses");
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn resolve_statement_distinguishes_direct_and_named_default_twins() {
    let journal = EmptyEstablishedJournal::new();
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness creates");
    let member = serde_json::json!({
        "day": "20260731",
        "stream": "_default",
        "segment_key": "080000_300",
        "source": "mic_audio",
        "sentence_id": 1,
    });
    let mut direct = member.clone();
    direct["stream_layout"] = Value::String("direct".to_owned());
    let mut named = member;
    named["stream_layout"] = Value::String("named".to_owned());
    fs::write(
        journal.0.join("awareness/discovery_clusters.json"),
        serde_json::json!({"clusters": {"1": [named], "2": [direct]}}).to_string(),
    )
    .expect("cache writes");
    let base = "/app/speakers/api/discovery/resolve-statement?voice_day=20260731&voice_stream=_default&voice_segment_key=080000_300&voice_source=mic_audio&voice_sentence_id=1";
    for (query, expected_cluster) in [
        (base.to_owned(), 1),
        (format!("{base}&voice_stream_layout=direct"), 2),
    ] {
        let response = router(journal.0.clone())
            .oneshot(
                Request::get(query)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status().as_u16(), 200);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body reads"),
        )
        .expect("response JSON parses");
        assert_eq!(
            body,
            serde_json::json!({"status": "hit", "cluster_id": expected_cluster})
        );
    }
}

#[tokio::test]
async fn mixed_invalid_discovery_members_are_never_reported_as_a_complete_subset() {
    let journal = EmptyEstablishedJournal::new();
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness creates");
    let valid = serde_json::json!({
        "day": "20260731",
        "stream": "field",
        "segment_key": "090000_300",
        "source": "mic_audio",
        "sentence_id": 1,
        "stream_layout": "named",
    });
    let mut invalid = valid.clone();
    invalid["stream_layout"] = Value::String("Named".to_owned());
    fs::write(
        journal.0.join("awareness/discovery_clusters.json"),
        serde_json::json!({"clusters": {"1": [valid, invalid]}}).to_string(),
    )
    .expect("cache writes");

    let response = router(journal.0.clone())
        .oneshot(
            Request::get("/app/speakers/api/discovery/cluster/1/presence")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status().as_u16(), 500);

    let response = router(journal.0.clone())
        .oneshot(
            Request::get("/app/speakers/api/discovery/resolve-statement?voice_day=20260731&voice_stream=field&voice_segment_key=090000_300&voice_source=mic_audio&voice_sentence_id=1")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads"),
    )
    .expect("response JSON parses");
    assert_eq!(
        body,
        serde_json::json!({"status": "cache_incomplete", "cluster_id": null})
    );
}
