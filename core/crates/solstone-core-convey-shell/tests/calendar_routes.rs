// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
use crate::support;

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
async fn speaker_dates_follow_ingest_retention_admission_and_label_changes() {
    async fn get(app: &axum::Router, route: &str) -> Value {
        let response = app
            .clone()
            .oneshot(Request::get(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response.status().is_success());
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    let journal = EmptyEstablishedJournal::new();
    let app = router(journal.0.clone());
    let index = "/app/speakers/api/index";
    let grid = "/app/speakers/api/grid";
    assert_eq!(get(&app, index).await["months"], serde_json::json!({}));

    let segment = journal.0.join("chronicle/20260101/main/090000_60");
    fs::create_dir_all(segment.join("talents")).unwrap();
    fs::write(segment.join("audio.npz"), []).unwrap();
    // The date count needs embedding presence, not a readable speaker payload.
    fs::write(segment.join("talents/speakers.json"), "{malformed").unwrap();
    let labels = segment.join("talents/speaker_labels.json");
    fs::write(
        &labels,
        serde_json::json!({"labels":[{"speaker":"reader","confidence":"high"}]}).to_string(),
    )
    .unwrap();
    assert_eq!(get(&app, index).await["months"]["202601"], 1);
    assert_eq!(get(&app, grid).await["activity"]["20260101"], 1);
    assert_eq!(get(&app, grid).await["days"]["20260101"], 1);

    let entity = journal.0.join("entities/reader/entity.json");
    fs::create_dir_all(entity.parent().unwrap()).unwrap();
    for (kind, blocked, review) in [
        ("Person", false, false),
        ("Tool", false, true),
        ("Person", true, true),
        ("Person", false, false),
    ] {
        fs::write(
            &entity,
            serde_json::json!({"id":"reader","name":"Reader","type":kind,"blocked":blocked})
                .to_string(),
        )
        .unwrap();
        let value = get(&app, grid).await;
        assert_eq!(value["activity"]["20260101"], 1);
        assert_eq!(value["days"]["20260101"].as_u64(), review.then_some(1));
    }
    for (confidence, review) in [("medium", true), ("high", false)] {
        fs::write(
            &labels,
            serde_json::json!({"labels":[{"speaker":"reader","confidence":confidence}]})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            get(&app, grid).await["days"]["20260101"].as_u64(),
            review.then_some(1),
        );
    }

    fs::remove_file(segment.join("audio.npz")).unwrap();
    fs::create_dir(segment.join("audio.npz")).unwrap();
    fs::write(segment.join("unrelated.npz"), []).unwrap();
    assert_eq!(get(&app, index).await["months"], serde_json::json!({}));
    assert_eq!(get(&app, grid).await["activity"], serde_json::json!({}));
    fs::write(segment.join("mic_audio.npz"), []).unwrap();
    assert_eq!(get(&app, index).await["months"]["202601"], 1);
    fs::remove_dir_all(&segment).unwrap();
    assert_eq!(get(&app, index).await["months"], serde_json::json!({}));
    assert_eq!(get(&app, grid).await["activity"], serde_json::json!({}));
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

#[tokio::test]
async fn segments_keep_direct_and_named_default_twins_distinct() {
    let journal = EmptyEstablishedJournal::new();
    let direct = journal.0.join("chronicle/20260101/080000_60");
    let named = journal.0.join("chronicle/20260101/_default/080000_60");
    fs::create_dir_all(&direct).expect("direct segment creates");
    fs::create_dir_all(&named).expect("named segment creates");
    fs::write(direct.join("audio.npz"), []).expect("direct evidence writes");
    fs::write(named.join("audio.npz"), []).expect("named evidence writes");

    let response = router(journal.0.clone())
        .oneshot(
            Request::get("/app/speakers/api/segments/20260101")
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
    let rows = body["segments"].as_array().expect("segments are an array");
    assert_eq!(rows.len(), 2, "{body}");
    assert!(rows.iter().any(|row| {
        row["key"] == "080000_60" && row["stream"] == "_default" && row["stream_layout"] == "direct"
    }));
    assert!(rows.iter().any(|row| {
        row["key"] == "080000_60" && row["stream"] == "_default" && row["stream_layout"] == "named"
    }));
}

#[test]
fn workspace_uses_full_collision_safe_segment_identity_and_fallible_loading() {
    let source = include_str!("../assets/speakers/workspace.html");
    for required in [
        "encodeURIComponent(JSON.stringify([",
        "segmentLayout(seg)",
        "data-segment-identity",
        "legacyMatches.length === 1",
        "legacyKey = decodeURIComponent(rawHash)",
        "const nextSegments = validatedSegments(data.segments)",
        "apiJson(segmentsApiUrl",
        "stream_layout: segmentLayout(selectedSegment)",
        "throw new Error('invalid segment layout')",
    ] {
        assert!(
            source.contains(required),
            "missing workspace contract: {required}"
        );
    }
    assert!(!source.contains("fetch(segmentsApiUrl"));
    assert!(!source.contains("data-key="));
}

#[tokio::test]
async fn segments_name_the_filtered_speaker() {
    let journal = EmptyEstablishedJournal::new();
    solstone_core_entity::save_entity_identity(
        &journal.0,
        "ada",
        &serde_json::json!({"id":"ada","name":"Ada Lovelace","type":"Person"}),
        None,
    )
    .expect("entity saves");
    let name_for = |path: &'static str| {
        let app = router(journal.0.clone());
        async move {
            let response = app
                .oneshot(
                    Request::get(path)
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("router responds");
            assert_eq!(response.status().as_u16(), 200);
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body reads"),
            )
            .expect("body parses");
            body.get("speaker_filter_name").cloned()
        }
    };
    assert_eq!(
        name_for("/app/speakers/api/segments/20260101?speaker=ada").await,
        Some(Value::String("Ada Lovelace".to_owned()))
    );
    assert_eq!(
        name_for("/app/speakers/api/segments/20260101?speaker=nobody").await,
        Some(Value::Null)
    );
    assert_eq!(name_for("/app/speakers/api/segments/20260101").await, None);
}
