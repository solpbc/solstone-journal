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
use solstone_core_speaker_resolve::OWNER_IDENTITY_INVALID_REASON;
use tower::ServiceExt;

use super::support::{
    PERSON_ADMISSION_DAY, PERSON_ADMISSION_SEGMENT, PERSON_ADMISSION_SOURCE,
    PERSON_ADMISSION_STREAM, PersonAdmissionMode, build_person_admission_journal, snapshot_files,
    write_embeddings_npz,
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
    fn owner_centroid(&self) {
        let mut centroid = vec![0.0; 256];
        centroid[0] = 1.0;
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            &self.0,
            "owner",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid,
                cluster_size: 5,
                timestamp: "2026-08-08T00:00:00Z".to_owned(),
                evidence_tier: "standard".to_owned(),
            },
        )
        .expect("owner centroid");
    }
}

fn resolve_names_encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

fn has_voiceprint_metadata(
    journal: &std::path::Path,
    entity: &str,
    day: &str,
    stream: &str,
    segment_key: &str,
    stream_layout: &str,
) -> bool {
    let Some(voiceprints) = solstone_core_entity::load_entity_voiceprints_file(journal, entity)
    else {
        return false;
    };
    voiceprints.metadata.iter().any(|m| {
        let Ok(v) = serde_json::from_str::<Value>(m) else {
            return false;
        };
        v.get("day").and_then(Value::as_str) == Some(day)
            && v.get("stream").and_then(Value::as_str) == Some(stream)
            && v.get("segment_key").and_then(Value::as_str) == Some(segment_key)
            && v.get("stream_layout").and_then(Value::as_str) == Some(stream_layout)
    })
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

async fn call_get(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).expect("request"))
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
        "/app/speakers/api/owner/reject-cli",
        json!({"version": "v1"}),
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
    assert!(!journal.0.join("awareness/owner_candidate.npz").exists());
}

#[tokio::test]
async fn reject_cli_missing_version_refuses_with_bad_request() {
    let journal = Journal::new();
    journal.entity("owner", true);
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/reject-cli",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn confirm_cli_refuses_in_place_with_review_required() {
    let journal = Journal::new();
    journal.entity("owner", true);
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/confirm-cli",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["reason_code"], "review_required");
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
            json!({"day":"20260808","stream_layout":"named","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
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
        json!({"day":"20260808","stream_layout":"named","stream":"main","segment_key":"120000_1","source":"audio","sentence_id":1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "assigned");
    assert_eq!(response["speaker"], "owner");
}

#[tokio::test]
async fn backfill_reports_invalid_owner_as_a_structured_error_without_writes() {
    let journal = build_person_admission_journal(PersonAdmissionMode::MissingTypePrincipal);
    let _before = snapshot_files(journal.root());

    let (status, response) = call(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/backfill",
        json!({"commit":true,"reattribute":true}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{response}");
    let op_id = response["operation_id"]
        .as_str()
        .expect("operation_id")
        .to_owned();

    let mut current_status = response;
    while current_status["status"] == "active_preparing"
        || current_status["status"] == "active_running"
    {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (status, resp) = call_get(
            router(journal.root().to_path_buf()),
            &format!("/app/speakers/api/backfill/operations/{op_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{resp}");
        current_status = resp;
    }

    assert_eq!(current_status["completed_count"], 0, "{current_status}");
    assert_eq!(current_status["error_count"], 1, "{current_status}");
    assert_eq!(
        current_status["error_segments"][0]["detail"],
        OWNER_IDENTITY_INVALID_REASON
    );
    assert_eq!(
        current_status["error_segments"][0]["segment"]["day"],
        PERSON_ADMISSION_DAY
    );
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

#[tokio::test]
async fn tag_cli_mutates_direct_and_named_twins_independently() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.voiceprint("owner");

    let direct_dir = journal.0.join("chronicle/20260808/120000_1");
    fs::create_dir_all(direct_dir.join("talents")).expect("direct talents");
    fs::write(
        direct_dir.join("audio.jsonl"),
        "{\"sentence_id\":1,\"text\":\"direct hello\"}\n",
    )
    .expect("direct transcript");
    write_embeddings_npz(&direct_dir.join("audio.npz"), 1, true, 0);
    fs::write(
        direct_dir.join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":1,"text":"direct"}]}).to_string(),
    )
    .expect("direct labels");

    let named_dir = journal.0.join("chronicle/20260808/_default/120000_1");
    fs::create_dir_all(named_dir.join("talents")).expect("named talents");
    fs::write(
        named_dir.join("audio.jsonl"),
        "{\"sentence_id\":1,\"text\":\"named hello\"}\n",
    )
    .expect("named transcript");
    write_embeddings_npz(&named_dir.join("audio.npz"), 1, true, 1);
    fs::write(
        named_dir.join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":1,"text":"named"}]}).to_string(),
    )
    .expect("named labels");

    let named_snapshot = crate::support::snapshot_files(&named_dir);

    // 1. Direct tag
    let (status, response) = call(
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
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "assigned");
    assert_eq!(response["stream_layout"], "direct");
    assert_eq!(
        crate::support::snapshot_files(&named_dir),
        named_snapshot,
        "named twin was modified by direct tag"
    );
    let direct_labels: Value = serde_json::from_str(
        &fs::read_to_string(direct_dir.join("talents/speaker_labels.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(direct_labels["labels"][0]["speaker"], "owner");
    assert!(
        has_voiceprint_metadata(
            &journal.0, "owner", "20260808", "_default", "120000_1", "direct"
        ),
        "direct voiceprint present after direct tag"
    );
    assert!(
        !has_voiceprint_metadata(
            &journal.0, "owner", "20260808", "_default", "120000_1", "named"
        ),
        "named voiceprint absent before named tag"
    );

    // 2. Named tag
    let direct_snapshot = crate::support::snapshot_files(&direct_dir);
    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/tag-cli",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment_key": "120000_1",
            "sentence_id": 1,
            "source": "audio",
            "stream_layout": "named",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "assigned");
    assert_eq!(response["stream_layout"], "named");
    assert_eq!(
        crate::support::snapshot_files(&direct_dir),
        direct_snapshot,
        "direct twin was modified by named tag"
    );
    let named_labels: Value = serde_json::from_str(
        &fs::read_to_string(named_dir.join("talents/speaker_labels.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(named_labels["labels"][0]["speaker"], "owner");
    assert!(
        has_voiceprint_metadata(
            &journal.0, "owner", "20260808", "_default", "120000_1", "direct"
        ),
        "direct voiceprint still present after named tag"
    );
    assert!(
        has_voiceprint_metadata(
            &journal.0, "owner", "20260808", "_default", "120000_1", "named"
        ),
        "named voiceprint present after named tag"
    );

    // 3. Collapse refusal in both directions
    let direct_only_dir = journal.0.join("chronicle/20260808/120000_direct_only");
    fs::create_dir_all(direct_only_dir.join("talents")).expect("direct only");
    fs::write(
        direct_only_dir.join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":1}]}).to_string(),
    )
    .expect("labels");
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/tag-cli",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment_key": "120000_direct_only",
            "sentence_id": 1,
            "source": "audio",
            "stream_layout": "named",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_review_unavailable");
    assert_eq!(crate::support::snapshot_files(&journal.0), before);

    let named_only_dir = journal
        .0
        .join("chronicle/20260808/_default/120000_named_only");
    fs::create_dir_all(named_only_dir.join("talents")).expect("named only");
    fs::write(
        named_only_dir.join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":1}]}).to_string(),
    )
    .expect("labels");
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/tag-cli",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment_key": "120000_named_only",
            "sentence_id": 1,
            "source": "audio",
            "stream_layout": "direct",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_review_unavailable");
    assert_eq!(crate::support::snapshot_files(&journal.0), before);
}

#[tokio::test]
async fn attribute_segment_mutates_direct_and_named_twins_independently() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.voiceprint("owner");
    journal.owner_centroid();

    let direct_dir = journal.0.join("chronicle/20260808/120000_1");
    fs::create_dir_all(&direct_dir).expect("direct");
    fs::write(
        direct_dir.join("audio.jsonl"),
        "{\"sentence_id\":1,\"text\":\"direct hello\"}\n",
    )
    .expect("direct transcript");
    write_embeddings_npz(&direct_dir.join("audio.npz"), 1, true, 0);

    let named_dir = journal.0.join("chronicle/20260808/_default/120000_1");
    fs::create_dir_all(&named_dir).expect("named");
    fs::write(
        named_dir.join("audio.jsonl"),
        "{\"sentence_id\":1,\"text\":\"named hello\"}\n",
    )
    .expect("named transcript");
    write_embeddings_npz(&named_dir.join("audio.npz"), 1, true, 1);

    let named_snapshot = crate::support::snapshot_files(&named_dir);

    // 1. Direct attribute
    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/attribute-segment",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment": "120000_1",
            "stream_layout": "direct",
            "commit": true,
            "save": true,
            "accumulate": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["stream_layout"], "direct");
    assert_eq!(response["segment_key"], "120000_1");
    assert_eq!(
        crate::support::snapshot_files(&named_dir),
        named_snapshot,
        "named twin was modified by direct attribute"
    );
    assert!(direct_dir.join("talents/speaker_labels.json").is_file());

    // 2. Named attribute
    let direct_snapshot = crate::support::snapshot_files(&direct_dir);
    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/attribute-segment",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment": "120000_1",
            "stream_layout": "named",
            "commit": true,
            "save": true,
            "accumulate": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["stream_layout"], "named");
    assert_eq!(response["segment_key"], "120000_1");
    assert_eq!(
        crate::support::snapshot_files(&direct_dir),
        direct_snapshot,
        "direct twin was modified by named attribute"
    );
    assert!(named_dir.join("talents/speaker_labels.json").is_file());

    // 3. Collapse refusal in both directions
    let direct_only_dir = journal.0.join("chronicle/20260808/120000_direct_only");
    fs::create_dir_all(&direct_only_dir).expect("direct only");
    fs::write(
        direct_only_dir.join("audio.jsonl"),
        "{\"sentence_id\":1,\"text\":\"direct\"}\n",
    )
    .expect("transcript");
    write_embeddings_npz(&direct_only_dir.join("audio.npz"), 1, true, 0);
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/attribute-segment",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment": "120000_direct_only",
            "stream_layout": "named",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_review_unavailable");
    assert_eq!(crate::support::snapshot_files(&journal.0), before);

    let named_only_dir = journal
        .0
        .join("chronicle/20260808/_default/120000_named_only");
    fs::create_dir_all(&named_only_dir).expect("named only");
    fs::write(
        named_only_dir.join("audio.jsonl"),
        "{\"sentence_id\":1,\"text\":\"named\"}\n",
    )
    .expect("transcript");
    write_embeddings_npz(&named_only_dir.join("audio.npz"), 1, true, 0);
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/attribute-segment",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment": "120000_named_only",
            "stream_layout": "direct",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_review_unavailable");
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
    for layout in [
        json!("Direct"),
        json!(""),
        json!(true),
        json!(1),
        Value::Null,
    ] {
        let before = crate::support::snapshot_files(&journal.0);
        let (status, refused) = call(
            router(journal.0.clone()),
            "/app/speakers/api/owner/tag-cli",
            json!({
                "day": "20260808",
                "stream": "main",
                "segment_key": "120000_1",
                "sentence_id": 1,
                "source": "audio",
                "stream_layout": layout,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["reason_code"], "invalid_segment_or_stream");
        assert_eq!(crate::support::snapshot_files(&journal.0), before);
    }

    // Missing stream_layout field
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/tag-cli",
        json!({
            "day": "20260808",
            "stream": "main",
            "segment_key": "120000_1",
            "sentence_id": 1,
            "source": "audio",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["reason_code"], "invalid_segment_or_stream");
    assert_eq!(crate::support::snapshot_files(&journal.0), before);
}

#[tokio::test]
async fn attribute_segment_malformed_stream_layout_is_not_named() {
    let journal = Journal::new();
    journal.entity("owner", true);
    for layout in [
        json!("Direct"),
        json!(""),
        json!(true),
        json!(1),
        Value::Null,
    ] {
        let before = crate::support::snapshot_files(&journal.0);
        let (status, refused) = call(
            router(journal.0.clone()),
            "/app/speakers/api/attribute-segment",
            json!({
                "day": "20260808",
                "stream": "_default",
                "segment": "120000_1",
                "stream_layout": layout,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["reason_code"], "invalid_segment_or_stream");
        assert_eq!(crate::support::snapshot_files(&journal.0), before);
    }

    // Missing stream_layout field
    let before = crate::support::snapshot_files(&journal.0);
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/attribute-segment",
        json!({
            "day": "20260808",
            "stream": "_default",
            "segment": "120000_1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["reason_code"], "invalid_segment_or_stream");
    assert_eq!(crate::support::snapshot_files(&journal.0), before);
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

async fn get_call(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).expect("request"))
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
async fn owner_status_unbound_on_identities_only_legacy_samples() {
    let journal = Journal::new();
    journal.entity("owner", true);
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness dir");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "20260808T120000Z",
                "samples": [
                    {
                        "day": "20260808",
                        "stream": "main",
                        "segment_key": "120000_1",
                        "sentence_id": 1,
                        "source": "audio",
                        "stream_layout": "standard"
                    }
                ]
            }
        })
        .to_string(),
    )
    .expect("awareness write");

    let mut centroid = vec![0.0; 256];
    centroid[0] = 1.0;
    let candidate = solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
        centroid,
        cluster_size: 1,
        threshold: 0.5,
        version: "20260808T120000Z".to_owned(),
        evidence_tier: "evidentiary".to_owned(),
    };
    let _lock =
        solstone_core_speaker_resolve::owner_candidate::hold_owner_candidate_lock(&journal.0)
            .unwrap();
    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate_in_lock(
        &journal.0, &candidate,
    )
    .unwrap();
    drop(_lock);

    let (status, value) =
        get_call(router(journal.0.clone()), "/app/speakers/api/owner/status").await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["status"], "candidate", "{value}");
    let samples = value["samples"].as_array().expect("samples array");
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0]["evidence_state"], "unbound");
    assert_eq!(samples[0]["artifact_eligible"], false);
}

#[tokio::test]
async fn owner_confirm_empty_body_refuses() {
    let journal = Journal::new();
    journal.entity("owner", true);
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness dir");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "20260808T120000Z",
                "samples": []
            }
        })
        .to_string(),
    )
    .expect("awareness write");

    let mut centroid = vec![0.0; 256];
    centroid[0] = 1.0;
    let candidate = solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
        centroid,
        cluster_size: 1,
        threshold: 0.5,
        version: "20260808T120000Z".to_owned(),
        evidence_tier: "evidentiary".to_owned(),
    };
    let _lock =
        solstone_core_speaker_resolve::owner_candidate::hold_owner_candidate_lock(&journal.0)
            .unwrap();
    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate_in_lock(
        &journal.0, &candidate,
    )
    .unwrap();
    drop(_lock);

    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/confirm",
        json!({}),
    )
    .await;
    assert!(status.is_client_error(), "{value}");
}

#[tokio::test]
async fn owner_set_aside_stale_version_refuses() {
    let journal = Journal::new();
    journal.entity("owner", true);
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness dir");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "20260808T120000Z",
                "samples": []
            }
        })
        .to_string(),
    )
    .expect("awareness write");

    let mut centroid = vec![0.0; 256];
    centroid[0] = 1.0;
    let candidate = solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
        centroid,
        cluster_size: 1,
        threshold: 0.5,
        version: "20260808T120000Z".to_owned(),
        evidence_tier: "evidentiary".to_owned(),
    };
    let _lock =
        solstone_core_speaker_resolve::owner_candidate::hold_owner_candidate_lock(&journal.0)
            .unwrap();
    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate_in_lock(
        &journal.0, &candidate,
    )
    .unwrap();
    drop(_lock);

    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/set-aside",
        json!({"version": "20260807T000000Z_stale"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{value}");
    assert_eq!(value["reason_code"], "speaker_candidate_stale_version");
}

#[tokio::test]
async fn owner_reject_cli_missing_version_refuses() {
    let journal = Journal::new();
    journal.entity("owner", true);
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/reject-cli",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
    assert_eq!(value["reason_code"], "missing_required_field");
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
async fn test_hooks_pause_after_snapshot_concurrent_status_waits() {
    use solstone_core_convey_shell::speakers_owner_write::test_hooks;
    use solstone_core_speaker_resolve::owner_candidate::{
        OwnerCandidate, hold_owner_candidate_lock, write_owner_candidate_in_lock,
    };

    test_hooks::reset();
    test_hooks::set_pause_after_snapshot(true);

    let journal = Journal::new();
    journal.entity("owner", true);

    let j_root = journal.0.clone();
    let writer_handle = std::thread::spawn(move || {
        let _lock = hold_owner_candidate_lock(&j_root).expect("candidate lock");
        let mut centroid = vec![0.0; 256];
        centroid[0] = 1.0;
        let candidate = OwnerCandidate {
            centroid,
            cluster_size: 3,
            threshold: 0.5,
            version: "20260808T120000Z".to_owned(),
            evidence_tier: "evidentiary".to_owned(),
        };
        write_owner_candidate_in_lock(&j_root, &candidate).expect("write candidate");

        test_hooks::wait_if_pause_after_snapshot();

        fs::create_dir_all(j_root.join("awareness")).expect("awareness dir");
        fs::write(
            j_root.join("awareness/current.json"),
            json!({
                "voiceprint": {
                    "status": "candidate",
                    "detected_at": "20260808T120000Z",
                    "cluster_size": 3,
                    "evidence_tier": "evidentiary",
                    "samples": []
                }
            })
            .to_string(),
        )
        .expect("awareness write");
    });

    test_hooks::wait_entered_pause_after_snapshot(1);

    let j_root_get = journal.0.clone();
    let status_handle = tokio::spawn(async move {
        get_call(router(j_root_get), "/app/speakers/api/owner/status").await
    });

    // Give the status request a moment to spawn and attempt to acquire the candidate lock
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    test_hooks::set_pause_after_snapshot(false);
    writer_handle.join().expect("writer thread join");

    let (status, value) = status_handle.await.expect("status task join");
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["status"], "candidate", "{value}");
    assert_eq!(value["version"], "20260808T120000Z", "{value}");
    assert_ne!(value.get("review"), Some(&json!("incomplete")), "{value}");

    test_hooks::reset();
}

#[cfg(feature = "test-hooks")]
#[tokio::test]
async fn test_hooks_stale_confirm_cannot_clear_replacement() {
    use solstone_core_convey_shell::speakers_owner_write::test_hooks;
    use solstone_core_speaker_resolve::owner_candidate::{
        OwnerCandidate, hold_owner_candidate_lock, load_owner_candidate,
        write_owner_candidate_in_lock,
    };

    test_hooks::reset();

    let journal = Journal::new();
    journal.entity("owner", true);

    // Initial candidate version A
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness dir");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "20260808T100000Z",
                "samples": []
            }
        })
        .to_string(),
    )
    .expect("awareness write");

    let mut centroid_a = vec![0.0; 256];
    centroid_a[0] = 1.0;
    let candidate_a = OwnerCandidate {
        centroid: centroid_a,
        cluster_size: 1,
        threshold: 0.5,
        version: "20260808T100000Z".to_owned(),
        evidence_tier: "evidentiary".to_owned(),
    };
    {
        let _lock = hold_owner_candidate_lock(&journal.0).expect("candidate lock");
        write_owner_candidate_in_lock(&journal.0, &candidate_a).expect("write candidate A");
    }

    test_hooks::set_hold_confirm_or_reject(true);

    let j_root = journal.0.clone();
    let confirm_handle = tokio::spawn(async move {
        call(
            router(j_root),
            "/app/speakers/api/owner/confirm",
            json!({
                "version": "20260808T100000Z",
                "samples": []
            }),
        )
        .await
    });

    test_hooks::wait_entered_hold_confirm_or_reject(1);

    // Install candidate replacement B while confirm is paused before taking the lock
    {
        let _lock = hold_owner_candidate_lock(&journal.0).expect("candidate lock");
        let mut centroid_b = vec![0.0; 256];
        centroid_b[0] = 2.0;
        let candidate_b = OwnerCandidate {
            centroid: centroid_b,
            cluster_size: 2,
            threshold: 0.5,
            version: "20260808T120000Z".to_owned(),
            evidence_tier: "evidentiary".to_owned(),
        };
        write_owner_candidate_in_lock(&journal.0, &candidate_b).expect("write candidate B");
        fs::write(
            journal.0.join("awareness/current.json"),
            json!({
                "voiceprint": {
                    "status": "candidate",
                    "detected_at": "20260808T120000Z",
                    "samples": []
                }
            })
            .to_string(),
        )
        .expect("awareness write B");
    }

    test_hooks::set_hold_confirm_or_reject(false);

    let (status, value) = confirm_handle.await.expect("confirm task join");
    assert_ne!(status, StatusCode::OK, "{value}");
    assert_eq!(status, StatusCode::CONFLICT, "{value}");
    assert_eq!(value["reason_code"], "speaker_candidate_stale_version");

    let loaded = load_owner_candidate(&journal.0)
        .expect("load candidate")
        .expect("candidate exists");
    assert_eq!(loaded.version, "20260808T120000Z");
    assert!(
        !journal
            .0
            .join("state/speakers/owner_centroid.json")
            .exists()
    );

    test_hooks::reset();
}

#[tokio::test]
async fn speakers_repair_report_only_and_commit_route() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();

    // Create a non-person entity with voiceprint
    let np_dir = journal.0.join("entities/acme");
    fs::create_dir_all(&np_dir).unwrap();
    fs::write(
        np_dir.join("entity.json"),
        serde_json::to_vec(&json!({"id": "acme", "name": "acme", "type": "Organization", "is_principal": false})).unwrap(),
    ).unwrap();

    let mut embedding = vec![0.0; 256];
    embedding[0] = 1.0;
    save_voiceprints_batch(
        &journal.0,
        "acme",
        &[VoiceprintItem {
            embedding,
            metadata: json!({
                "day": "20260808",
                "segment_key": "120000_300",
                "source": "audio",
                "sentence_id": 1,
            }),
        }],
        &resolve_names_encoder(),
    ).unwrap();

    // 1. Report-only (commit: false)
    let (status, value) = call(
        router(journal.0.clone()),
        "/app/speakers/api/repair",
        json!({"commit": false}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["mode"], "dry_run");
    assert_eq!(value["complete"], true);
    assert_eq!(value["clean"], false);

    // 2. Commit (commit: true)
    let (status_commit, value_commit) = call(
        router(journal.0.clone()),
        "/app/speakers/api/repair",
        json!({"commit": true, "operation_id": "op_repair_test"}),
    ).await;
    assert_eq!(status_commit, StatusCode::OK);
    assert_eq!(value_commit["status"], "completed");
    assert_eq!(value_commit["summary"]["complete"], true);
}
