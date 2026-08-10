// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Focused mutation-route contracts for the speakers CLI surface.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use solstone_core_entity::{EncoderIdentity, VoiceprintItem, save_voiceprints_batch};
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "solstone-speakers-cli-mutation-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("chronicle")).expect("chronicle");
        fs::create_dir_all(root.join("entities")).expect("entities");
        fs::create_dir_all(root.join("config")).expect("config");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("config");
        Self(root)
    }
    fn entity(&self, id: &str, principal: bool) {
        self.named_entity(id, id, principal);
    }
    fn named_entity(&self, id: &str, name: &str, principal: bool) {
        fs::create_dir_all(self.0.join("entities").join(id)).expect("entity dir");
        fs::write(
            self.0.join("entities").join(id).join("entity.json"),
            serde_json::to_vec(
                &json!({"id":id,"name":name,"type":"Person","is_principal":principal}),
            )
            .expect("json"),
        )
        .expect("entity");
    }
    fn voiceprint(&self, id: &str) {
        let mut embedding = vec![0.0; 256];
        embedding[1] = 1.0;
        save_voiceprints_batch(
            &self.0,
            id,
            &[VoiceprintItem {
                embedding,
                metadata: json!({
                    "day":"20260808",
                    "segment_key":id,
                    "source":"audio",
                    "sentence_id":1,
                }),
            }],
            &resolve_names_encoder(),
        )
        .expect("voiceprint");
    }
}

fn resolve_names_encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn call(app: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    (status, value)
}

#[tokio::test]
async fn ac7_resolve_names_defaults_to_dry_run_and_returns_native_stats() {
    let journal = Journal::new();
    journal.named_entity("alias", "Alex", false);
    journal.named_entity("canonical", "Alex Smith", false);
    journal.voiceprint("alias");
    journal.voiceprint("canonical");
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/resolve-names",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert!(value.get("reason_code").is_none());
    for key in [
        "entities_with_voiceprints",
        "pairs_compared",
        "matches_found",
        "auto_merged",
        "ambiguous",
        "errors",
    ] {
        assert!(value.get(key).is_some(), "missing {key}: {value}");
    }
    assert_eq!(value["auto_merged"][0]["alias"], "Alex");
    assert_eq!(value["auto_merged"][0]["canonical"], "Alex Smith");
    assert!(journal.0.join("entities/alias").exists());
    assert!(journal.0.join("entities/canonical").exists());
}

#[tokio::test]
async fn resolve_names_commit_merges_ready_candidate() {
    let journal = Journal::new();
    journal.named_entity("alias", "Alex", false);
    journal.named_entity("canonical", "Alex Smith", false);
    journal.voiceprint("alias");
    journal.voiceprint("canonical");
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/resolve-names",
        json!({"commit":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["errors"], json!([]));
    assert_eq!(value["auto_merged"][0]["alias"], "Alex");
    assert!(!journal.0.join("entities/alias").exists());
    let canonical: Value = serde_json::from_slice(
        &fs::read(journal.0.join("entities/canonical/entity.json")).expect("canonical"),
    )
    .expect("identity json");
    assert_eq!(canonical["aka"], json!(["Alex"]));
}

#[tokio::test]
async fn reject_declares_skipped_awareness_state() {
    let journal = Journal::new();
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/reject-cli",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["status"], "rejected");
    assert_eq!(value["partial_success"], true);
    assert_eq!(value["awareness_state"]["status"], "skipped");
    assert_eq!(
        value["awareness_state"]["reason_code"],
        "speaker_awareness_state_not_native"
    );
}

#[tokio::test]
async fn confirm_declares_skipped_awareness_state_after_native_mutation() {
    let journal = Journal::new();
    journal.entity("owner", true);
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness");
    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate(
        &journal.0,
        &solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
            centroid: vec![1.0; 256],
            cluster_size: 2,
            threshold: 0.5,
            version: "v1".to_owned(),
            evidence_tier: "standard".to_owned(),
        },
    )
    .expect("candidate");
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/confirm-cli",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["status"], "confirmed");
    assert_eq!(value["partial_success"], true);
    assert_eq!(
        value["awareness_state"]["reason_code"],
        "speaker_awareness_state_not_native"
    );
    assert!(!journal.0.join("awareness/owner_candidate.npz").exists());
}

#[tokio::test]
async fn wipe_is_safe_by_default_and_removes_only_on_commit() {
    let journal = Journal::new();
    let file = journal
        .0
        .join("chronicle/20260101/stream/120000_60/mic_audio.npz");
    fs::create_dir_all(file.parent().expect("parent")).expect("segment");
    fs::write(&file, b"synthetic").expect("embedding");
    let (status, dry_run) = call(
        router(journal.0.clone()),
        "/app/speakers/api/wipe",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dry_run["dry_run"], true);
    assert!(file.exists());
    let (status, committed) = call(
        router(journal.0.clone()),
        "/app/speakers/api/wipe",
        json!({"commit":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(committed["dry_run"], false);
    assert!(!file.exists());
}

#[tokio::test]
async fn merge_names_projects_preflight_failure_reason_codes() {
    let journal = Journal::new();
    journal.entity("one", false);
    let app = router(journal.0.clone());
    let (status, missing) = call(
        app.clone(),
        "/app/speakers/api/merge-names",
        json!({"alias":"missing","canonical":"one"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["reason_code"], "speaker_not_found");
    let (status, same) = call(
        app,
        "/app/speakers/api/merge-names",
        json!({"alias":"one","canonical":"one"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(same["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn merge_names_ready_path_returns_native_counts() {
    let journal = Journal::new();
    journal.entity("alias", false);
    journal.entity("canonical", false);
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/merge-names",
        json!({"alias":"alias","canonical":"canonical"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["merged"], true);
    assert!(value["voiceprints_merged"].is_number());
    assert!(value["segments_scanned"].is_number());
}
