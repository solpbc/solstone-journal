// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Focused mutation-route contracts for the speakers CLI surface.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use solstone_core_entity::{EncoderIdentity, VoiceprintItem, save_voiceprints_batch};
use tower::ServiceExt;

use super::support::{
    PERSON_ADMISSION_DAY, PERSON_ADMISSION_SEGMENT, PERSON_ADMISSION_SOURCE,
    PERSON_ADMISSION_STREAM, PersonAdmissionMode, build_person_admission_journal,
};

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

fn content_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("journal directory reads") {
            let entry = entry.expect("journal entry reads");
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, snapshot);
            } else if path.is_file() {
                snapshot.insert(
                    path.strip_prefix(root)
                        .expect("journal-relative path")
                        .to_path_buf(),
                    fs::read(path).expect("journal file reads"),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    collect(root, root, &mut snapshot);
    snapshot
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
async fn confirm_cli_refuses_invalid_owner_identity_before_candidate_reads_or_writes() {
    let journal = Journal::new();
    let before = content_snapshot(&journal.0);

    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/confirm-cli",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
    assert_eq!(value["reason_code"], "speaker_owner_identity_invalid");
    assert_eq!(content_snapshot(&journal.0), before);
}

#[tokio::test]
async fn tag_cli_uses_the_admitted_owner_and_refuses_invalid_identity_without_writes() {
    for mode in [
        PersonAdmissionMode::MissingTypePrincipal,
        PersonAdmissionMode::CollisionLoserPrincipal,
    ] {
        let journal = build_person_admission_journal(mode);
        let before = content_snapshot(journal.root());
        let (actual_status, response) = call(
            router(journal.root().to_path_buf()),
            "/app/speakers/api/owner/tag-cli",
            json!({"day":"20260808","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
        )
        .await;
        assert_eq!(actual_status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(response["reason_code"], "speaker_owner_identity_invalid");
        assert_eq!(content_snapshot(journal.root()), before);
    }

    let journal = build_person_admission_journal(PersonAdmissionMode::Valid);
    let (status, response) = call(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/owner/tag-cli",
        json!({"day":"20260808","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "assigned");
    assert_eq!(response["speaker"], "owner");
}

#[tokio::test]
async fn backfill_last_seen_skips_ineligible_speaker_voiceprints() {
    let journal = build_person_admission_journal(PersonAdmissionMode::Valid);
    let encoder = solstone_core_entity::EncoderIdentity {
        id: "test".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    };
    for entity_id in ["person", "tool"] {
        solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
            journal.root(),
            entity_id,
            vec![1.0; 256],
            json!({"day":PERSON_ADMISSION_DAY,"stream":PERSON_ADMISSION_STREAM,"segment_key":PERSON_ADMISSION_SEGMENT,"source":PERSON_ADMISSION_SOURCE,"sentence_id":1}),
            &encoder,
        )
        .expect("voiceprint writes");
    }
    fs::write(
        journal.segment().join("talents/speaker_labels.json"),
        json!({"labels":[
            {"sentence_id":1,"speaker":"person"},
            {"sentence_id":2,"speaker":"tool"}
        ]})
        .to_string(),
    )
    .expect("labels write");
    let tool_voiceprints =
        fs::read(journal.root().join("entities/tool/voiceprints.npz")).expect("tool voiceprints");

    let (status, response) = call(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/backfill-last-seen",
        json!({"commit":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["skipped_ineligible_count"], 1);
    assert_eq!(response["skipped_ineligible"], json!(["tool"]));
    assert_eq!(
        fs::read(journal.root().join("entities/tool/voiceprints.npz")).expect("tool voiceprints"),
        tool_voiceprints
    );
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

#[tokio::test]
async fn link_import_rejects_ambiguous_alias_conflict() {
    // Polarity guard — green before and after the wave; reddens if this caller is
    // moved to the public find_matching_entity wrapper instead of the detailed entry point.
    let journal = Journal::new();
    journal.named_entity("target", "Target Person", false);
    journal.named_entity("sam-one", "Sam Person", false);
    journal.named_entity("sam-two", "Sam Person", false);
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/link-import",
        json!({"entity_id":"target","name":"Sam Person"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{value}");
    assert_eq!(value["reason_code"], "entity_alias_conflict");
}

fn direct_dir(journal: &Journal, day: &str, segment: &str) {
    fs::create_dir_all(journal.0.join("chronicle").join(day).join(segment)).expect("direct");
}

fn assert_direct_refused(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["reason_code"], "speaker_segment_layout_unsupported");
    assert_eq!(
        body["error"],
        "This command can't change that speaker review."
    );
    assert_eq!(
        body["detail"],
        "This segment uses the direct journal layout, which this command doesn't support."
    );
}

#[tokio::test]
async fn tag_cli_refuses_direct_layout_without_writes() {
    let journal = Journal::new();
    journal.entity("owner", true);
    direct_dir(&journal, "20260808", "120000_1");
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/tag-cli",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment_key": "120000_1",
            "sentence_id": 1,
            "source": "audio",
            "stream_layout": "direct",
        }),
    )
    .await;
    assert_direct_refused(status, &refused);
    assert_eq!(crate::support::snapshot_files(&journal.0), before);
}

#[tokio::test]
async fn attribute_segment_refuses_direct_layout_without_writes() {
    let journal = Journal::new();
    direct_dir(&journal, "20260808", "120000_1");
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/attribute-segment",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment": "120000_1",
            "stream_layout": "direct",
        }),
    )
    .await;
    assert_direct_refused(status, &refused);
    assert_eq!(crate::support::snapshot_files(&journal.0), before);
}

#[tokio::test]
async fn review_cli_reads_a_direct_segment() {
    let journal = Journal::new();
    let segment = journal.0.join("chronicle/20260808/120000_1");
    fs::create_dir_all(&segment).expect("direct");
    fs::write(
        segment.join("audio.jsonl"),
        "{\"raw\":\"audio.flac\"}\n{\"text\":\"hello\"}\n",
    )
    .expect("transcript");
    let response = router(journal.0.clone())
        .oneshot(
            axum::http::Request::get(
                "/app/speakers/api/review-cli/20260808/_default/120000_1/audio?stream_layout=direct",
            )
            .body(axum::body::Body::empty())
            .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(body["success"], true, "{body}");
    assert_eq!(
        body["sentences"].as_array().map(Vec::len),
        Some(1),
        "{body}"
    );
}

#[tokio::test]
async fn tag_cli_malformed_stream_layout_is_not_named() {
    let journal = Journal::new();
    journal.entity("owner", true);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/tag-cli",
        json!({
            "day": "20260808",
            "stream": "main",
            "segment_key": "120000_1",
            "sentence_id": 1,
            "source": "audio",
            "stream_layout": "Direct",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["reason_code"], "invalid_segment_or_stream");
}

#[tokio::test]
async fn backfill_last_seen_reads_direct_labels_and_preflights_all_labels_before_writes() {
    let journal = Journal::new();
    journal.entity("owner", false);
    journal.voiceprint("owner");
    let direct = journal.0.join("chronicle/20260808/120000_1/talents");
    fs::create_dir_all(&direct).expect("direct talents create");
    fs::write(
        direct.join("speaker_labels.json"),
        json!({"labels":[{"sentence_id":1,"speaker":"owner"}]}).to_string(),
    )
    .expect("direct labels write");

    let (status, result) = call(
        router(journal.0.clone()),
        "/app/speakers/api/backfill-last-seen",
        json!({"commit":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["labels_read"], 1, "{result}");
    assert_eq!(result["rows_written"], 1, "{result}");
    let rows = solstone_core_entity::load_entity_voiceprints_file(&journal.0, "owner")
        .expect("voiceprints remain readable");
    let metadata: Value = serde_json::from_str(&rows.metadata[0]).expect("metadata parses");
    assert!(metadata["last_seen_ts"].as_i64().unwrap_or_default() > 0);

    let invalid = journal.0.join("chronicle/20260809/main/120000_1/talents");
    fs::create_dir_all(&invalid).expect("invalid segment creates");
    fs::write(invalid.join("speaker_labels.json"), b"not json").expect("invalid labels write");
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/backfill-last-seen",
        json!({"commit":true}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_command_failed");
    assert_eq!(crate::support::snapshot_files(&journal.0), before);
}
