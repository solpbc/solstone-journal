// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mutation-route coverage for native speaker attribution and owner screening.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_shell::router;
use solstone_core_npy::write_npy;
use tower::ServiceExt;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::support::{PersonAdmissionMode, build_person_admission_journal, snapshot_files};
use solstone_core_speaker_resolve::OWNER_IDENTITY_INVALID_REASON;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const DAY: &str = "20260808";
const STREAM: &str = "main";
const SEGMENT: &str = "120000_1";
const SOURCE: &str = "audio";

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "solstone-speakers-attribution-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("config")).expect("config");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("config");
        Self(root)
    }

    fn entity(&self, id: &str, principal: bool) {
        self.entity_value(
            id,
            json!({"id":id,"name":id,"type":"Person","is_principal":principal}),
        );
    }

    fn entity_value(&self, id: &str, value: Value) {
        let directory = self.0.join("entities").join(id);
        fs::create_dir_all(&directory).expect("entity directory");
        fs::write(directory.join("entity.json"), value.to_string()).expect("entity");
    }

    fn segment(&self, labels: Value) {
        self.segment_at(SEGMENT, labels, unit(1.0, 0.0));
    }

    fn direct_segment(&self, labels: Value) {
        self.direct_segment_at(DAY, SEGMENT, labels);
    }

    fn direct_segment_at(&self, day: &str, segment_key: &str, labels: Value) {
        let directory = self.0.join("chronicle").join(day).join(segment_key);
        fs::create_dir_all(directory.join("talents")).expect("talents");
        fs::write(
            directory.join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n{\"id\":1,\"text\":\"test\"}\n",
        )
        .expect("sentences");
        fs::write(
            directory.join("talents/speaker_labels.json"),
            labels.to_string(),
        )
        .expect("labels");
        write_embeddings(&directory.join("audio.npz"), &[unit(1.0, 0.0)]);
    }

    fn named_default_segment_at(&self, day: &str, segment_key: &str, labels: Value) {
        let directory = self
            .0
            .join("chronicle")
            .join(day)
            .join("_default")
            .join(segment_key);
        fs::create_dir_all(directory.join("talents")).expect("talents");
        fs::write(
            directory.join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n{\"id\":1,\"text\":\"test\"}\n",
        )
        .expect("sentences");
        fs::write(
            directory.join("talents/speaker_labels.json"),
            labels.to_string(),
        )
        .expect("labels");
        write_embeddings(&directory.join("audio.npz"), &[unit(1.0, 0.0)]);
    }

    fn segment_at(&self, segment_key: &str, labels: Value, embedding: Vec<f32>) {
        let directory = self
            .0
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(segment_key);
        fs::create_dir_all(directory.join("talents")).expect("talents");
        fs::write(
            directory.join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n{\"id\":1,\"text\":\"test\"}\n",
        )
        .expect("sentences");
        fs::write(
            directory.join("talents/speaker_labels.json"),
            labels.to_string(),
        )
        .expect("labels");
        write_embeddings(&directory.join("audio.npz"), &[embedding]);
    }

    fn owner_centroid(&self) {
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            &self.0,
            "owner",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid: unit(1.0, 0.0),
                cluster_size: 5,
                timestamp: "2026-08-08T00:00:00Z".to_owned(),
                evidence_tier: "standard".to_owned(),
            },
        )
        .expect("owner centroid");
    }

    fn voiceprint_state(&self, voiceprint: Value) {
        fs::create_dir_all(self.0.join("awareness")).expect("awareness");
        fs::write(
            self.0.join("awareness/current.json"),
            json!({"voiceprint":voiceprint}).to_string(),
        )
        .expect("awareness state");
    }

    fn voiceprint(&self, entity_id: &str, embedding: Vec<f32>) {
        solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
            &self.0,
            entity_id,
            embedding,
            json!({"day":DAY,"stream":STREAM,"segment_key":"fixture","source":SOURCE,"sentence_id":1}),
            &solstone_core_entity::EncoderIdentity {
                id: "unresolved".to_owned(),
                sha256: "0".repeat(64),
                width: 256,
            },
        )
        .expect("voiceprint");
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn call(app: axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post(path)
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

async fn get(app: axum::Router, path: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
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

fn request() -> Value {
    json!({"day":DAY,"stream_layout":"named","stream":STREAM,"segment_key":SEGMENT,"source":SOURCE,"sentence_id":1})
}

#[tokio::test]
async fn speaker_post_routes_refuse_unsafe_source_components() {
    let journal = Journal::new();
    for source in [
        "",
        "..",
        "../outside",
        "..%2Foutside",
        "..%5Coutside",
        "/outside",
        r"..\outside",
        "C:outside",
        "a\0b",
        "a%00b",
    ] {
        for path in [
            "/app/speakers/api/assign-attribution",
            "/app/speakers/api/confirm-attribution",
            "/app/speakers/api/correct-attribution",
            "/app/speakers/api/owner/tag-cli",
            "/app/speakers/api/owner/classify",
        ] {
            let mut body = request();
            body["source"] = json!(source);
            if path.ends_with("assign-attribution") || path.ends_with("owner/tag-cli") {
                body["speaker"] = json!("owner");
            }
            if path.ends_with("correct-attribution") {
                body["new_speaker"] = json!("owner");
            }
            let (status, response) = call(router(journal.0.clone()), path, body).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{path} {source:?}: {response}"
            );
            assert_eq!(
                response["reason_code"], "invalid_request_value",
                "{path} {source:?}: {response}"
            );
        }
    }
}

fn unit(first: f32, second: f32) -> Vec<f32> {
    let mut values = vec![0.0; 256];
    values[0] = first;
    values[1] = second;
    values
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn i32_bytes(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn archive(members: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(name, options).expect("member");
        writer.write_all(&bytes).expect("member bytes");
    }
    writer.finish().expect("archive").into_inner()
}

fn write_embeddings(path: &Path, rows: &[Vec<f32>]) {
    let values = rows.iter().flatten().copied().collect::<Vec<_>>();
    let ids = (1..=rows.len()).map(|id| id as i32).collect::<Vec<_>>();
    fs::write(
        path,
        archive(vec![
            (
                "embeddings.npy",
                write_npy(
                    "<f4",
                    &format!("({}, 256)", rows.len()),
                    &f32_bytes(&values),
                ),
            ),
            (
                "statement_ids.npy",
                write_npy("<i4", &format!("({},)", ids.len()), &i32_bytes(&ids)),
            ),
        ]),
    )
    .expect("embeddings");
}

fn content_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("journal directory reads") {
            let entry = entry.expect("journal entry reads");
            let path = entry.path();
            if path.strip_prefix(root).expect("journal-relative path") == Path::new("health") {
                continue;
            }
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

fn set_labels(root: &Path, labels: Value) {
    fs::write(
        root.join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT)
            .join("talents/speaker_labels.json"),
        labels.to_string(),
    )
    .expect("labels write");
}

#[tokio::test]
async fn owner_target_successfully_exercises_all_four_routes() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity("other", false);

    journal.segment(json!({"labels":[{"sentence_id":1}]}));
    let mut assign = request();
    assign["speaker"] = json!("owner");
    let (status, body) = call(
        router(journal.0.clone()),
        "/app/speakers/api/assign-attribution",
        assign,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "assigned");
    assert_eq!(body["owner_bootstrap_outcome"], "refused");

    journal.segment(json!({"labels":[{"sentence_id":1,"speaker":"owner","confidence":"medium","method":"acoustic"}]}));
    let (status, body) = call(
        router(journal.0.clone()),
        "/app/speakers/api/confirm-attribution",
        request(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "confirmed");

    journal.segment(json!({"labels":[{"sentence_id":1,"speaker":"other","confidence":"medium","method":"acoustic"}]}));
    let mut correct = request();
    correct["new_speaker"] = json!("owner");
    let (status, body) = call(
        router(journal.0.clone()),
        "/app/speakers/api/correct-attribution",
        correct,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "corrected");

    let (status, body) = call(
        router(journal.0.clone()),
        "/app/speakers/api/propagate-correction",
        json!({"old_speaker":"other","new_speaker":"owner","commit":false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "preview");
}

#[tokio::test]
async fn owner_write_routes_cover_ready_detect_build_rebuild_confirm_reject_and_classify() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.segment(json!({"labels":[{"sentence_id":1}]}));

    let (status, ready) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/ready",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready["reason"], "no_candidate");

    let (status, detect) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/detect",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detect}");
    assert_eq!(detect["status"], "no_cluster");

    let (status, rebuild) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/rebuild",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rebuild}");
    assert_eq!(rebuild["status"], "refused");

    let (status, build) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/build-from-tags",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{build}");
    assert_eq!(build["status"], "low_quality");

    let (status, classify) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/classify",
        json!({"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"source":SOURCE}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{classify}");
    assert_eq!(classify["sentences"], json!([]));

    let emb = unit(1.0, 0.0);
    let mut le = Vec::new();
    for v in &emb {
        le.extend_from_slice(&v.to_le_bytes());
    }
    let emb_sha = format!("{:x}", Sha256::digest(&le));
    let audio_bytes = b"RIFFaudiofakebytes";
    let audio_sha = format!("{:x}", Sha256::digest(audio_bytes));

    let seg_dir = journal
        .0
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(&seg_dir).expect("seg dir");
    fs::write(
        seg_dir.join("audio.jsonl"),
        "{\"raw\":\"audio.flac\"}\n{\"sentence_id\":1,\"text\":\"test\"}\n",
    )
    .expect("jsonl");
    fs::write(seg_dir.join("audio.flac"), audio_bytes).expect("flac");
    write_embeddings(&seg_dir.join("audio.npz"), std::slice::from_ref(&emb));

    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate(
        &journal.0,
        &solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
            centroid: emb.clone(),
            cluster_size: 5,
            threshold: 0.43,
            version: "owner-candidate-v1".to_owned(),
            evidence_tier: "standard".to_owned(),
        },
    )
    .expect("candidate");
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "owner-candidate-v1",
                "recommendation": "ready",
                "cluster_size": 5,
                "streams_represented": 2,
                "samples": [{
                    "day": DAY,
                    "stream_layout": "named",
                    "stream": STREAM,
                    "segment_key": SEGMENT,
                    "source": "audio",
                    "sentence_id": 1,
                    "version": "owner-candidate-v1",
                    "embedding_sha256": emb_sha,
                    "audio_sha256": audio_sha,
                }]
            }
        })
        .to_string(),
    )
    .expect("state");
    let (status, candidate_ready) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/ready",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{candidate_ready}");
    assert_eq!(candidate_ready["reason"], "candidate_found");

    let (status, confirmed) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/confirm",
        json!({
            "version": "owner-candidate-v1",
            "day": DAY,
            "stream_layout": "named",
            "stream": STREAM,
            "segment_key": SEGMENT,
            "source": "audio",
            "sentence_id": 1,
            "audio_sha256": audio_sha,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(
        confirmed,
        json!({"status":"confirmed","principal_id":"owner"})
    );

    let (status, after_confirm) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/ready",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after_confirm}");
    assert_eq!(after_confirm["reason"], "centroid_exists");

    // Re-create candidate to test reject
    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate(
        &journal.0,
        &solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
            centroid: emb.clone(),
            cluster_size: 5,
            threshold: 0.43,
            version: "owner-candidate-v2".to_owned(),
            evidence_tier: "standard".to_owned(),
        },
    )
    .expect("candidate 2");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "owner-candidate-v2",
                "recommendation": "ready",
                "cluster_size": 5,
                "streams_represented": 2,
                "samples": []
            }
        })
        .to_string(),
    )
    .expect("state 2");

    let (status, rejected) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/reject",
        json!({"version": "owner-candidate-v2"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["status"], "needs_detection");
}

#[tokio::test]
async fn owner_identity_invalid_refuses_every_owner_surface_without_writes() {
    for mode in [
        PersonAdmissionMode::MissingTypePrincipal,
        PersonAdmissionMode::CollisionLoserPrincipal,
    ] {
        let journal = build_person_admission_journal(mode);
        for (path, body) in [
            ("/app/speakers/api/owner/detect", json!({})),
            ("/app/speakers/api/owner/ready", json!({})),
            ("/app/speakers/api/owner/confirm", json!({})),
            ("/app/speakers/api/owner/build-from-tags", json!({})),
            ("/app/speakers/api/owner/rebuild", json!({})),
            ("/app/speakers/api/owner/reject", json!({})),
            ("/app/speakers/api/owner/set-aside", json!({})),
            (
                "/app/speakers/api/owner/classify",
                json!({"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"source":SOURCE}),
            ),
        ] {
            let before = content_snapshot(journal.root());
            let (status, response) = call(router(journal.root().to_path_buf()), path, body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {response}");
            assert_eq!(response["reason_code"], "speaker_owner_identity_invalid");
            assert_eq!(content_snapshot(journal.root()), before, "{path}");
        }

        for path in [
            "/app/speakers/api/owner/status",
            "/app/speakers/api/quality",
            "/app/speakers/api/status",
        ] {
            let before = content_snapshot(journal.root());
            let (status, response) = get(router(journal.root().to_path_buf()), path).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {response}");
            assert_eq!(response["reason_code"], "speaker_owner_identity_invalid");
            assert_eq!(content_snapshot(journal.root()), before, "{path}");
        }
    }
}

#[tokio::test]
async fn bootstrap_routes_refuse_invalid_owner_identity_without_writes() {
    for mode in [
        PersonAdmissionMode::MissingTypePrincipal,
        PersonAdmissionMode::CollisionLoserPrincipal,
    ] {
        let journal = build_person_admission_journal(mode);
        for path in [
            "/app/speakers/api/bootstrap",
            "/app/speakers/api/seed-from-imports",
        ] {
            let before = content_snapshot(journal.root());
            let (status, response) =
                call(router(journal.root().to_path_buf()), path, json!({})).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {response}");
            assert_eq!(response["reason_code"], "speaker_owner_identity_invalid");
            assert_eq!(content_snapshot(journal.root()), before, "{path}");
        }
    }
}

#[tokio::test]
async fn owner_detect_generates_candidate_from_the_native_candidate_pool() {
    let journal = Journal::new();
    journal.entity("owner", true);
    let directory = journal
        .0
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(&directory).expect("segment");
    let mut transcript = String::from("{\"raw\":\"audio.flac\"}\n");
    for second in 0..=30 {
        let start = second * 2;
        transcript.push_str(&format!(
            "{{\"id\":{},\"start\":\"00:00:{start:02}\"}}\n",
            second + 1
        ));
    }
    fs::write(directory.join("audio.jsonl"), transcript).expect("transcript");
    write_embeddings(
        &directory.join("audio.npz"),
        &(0..30).map(|_| unit(1.0, 0.0)).collect::<Vec<_>>(),
    );
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness");
    fs::write(
        journal.0.join("awareness/speaker_candidates.json"),
        json!({"next_id":2,"candidates":[{
            "cand_id":1,"centroid":unit(1.0,0.0),"n_segments":1,"n_intervals":30,
            "total_duration_s":60.0,"status":"pending","confirmed_entity":null,"merge_events":[],
            "source_segments":[{"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"source":SOURCE,"cluster_label":1,"sentence_ids":(1..=30).collect::<Vec<_>>() }]
        }],"consolidation_summary":{"merge_count_total":0,"last_merge":null}}).to_string(),
    )
    .expect("candidate pool");

    let (status, body) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/detect",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "candidate");
    assert_eq!(body["cluster_size"], 30);
    assert_eq!(body["recommendation"], "single_stream");
    assert!(journal.0.join("awareness/owner_candidate.npz").is_file());
}

#[tokio::test]
async fn owner_detect_reports_candidate_pool_low_quality_before_expansion() {
    let journal = Journal::new();
    journal.entity("owner", true);
    fs::create_dir_all(journal.0.join("awareness")).expect("awareness");
    fs::write(
        journal.0.join("awareness/speaker_candidates.json"),
        json!({"next_id":2,"candidates":[{"cand_id":1,"centroid":unit(1.0,0.0),"n_segments":1,"n_intervals":4,"total_duration_s":8.0,"status":"pending","confirmed_entity":null,"merge_events":[],"source_segments":[]}],"consolidation_summary":{"merge_count_total":0,"last_merge":null}}).to_string(),
    )
    .expect("candidate pool");
    let (status, body) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/detect",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "low_quality");
    assert_eq!(body["low_quality_reason"], "too_few_stmts");
    assert_eq!(body["segments_available"], 1);
    assert_eq!(body["embeddings_available"], 4);
}

#[tokio::test]
async fn owner_ready_and_detect_cover_confirmed_state() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    journal.voiceprint_state(json!({
        "status":"confirmed",
        "cluster_size":5,
        "evidence_tier":"standard",
    }));

    let (status, ready) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/ready",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready, json!({"ready":false,"reason":"centroid_exists"}));

    let (status, detect) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/detect",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detect}");
    assert_eq!(detect["status"], "confirmed");
    assert_eq!(detect["recommendation"], "confirmed");
    assert_eq!(detect["cluster_size"], 5);
    assert_eq!(detect["evidence_tier"], "standard");
}

#[tokio::test]
async fn owner_ready_treats_expired_rejection_as_plain_needs_detection() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.voiceprint_state(json!({
        "status":"rejected",
        "rejected_at":(Utc::now() - Duration::days(15)).to_rfc3339(),
    }));

    let (status, ready) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/ready",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready, json!({"ready":false,"reason":"no_candidate"}));
}

#[tokio::test]
async fn owner_ready_and_detect_refuse_during_rejection_cooldown() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.voiceprint_state(json!({
        "status":"rejected",
        "rejected_at":Utc::now().to_rfc3339(),
    }));

    let (status, ready) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/ready",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready["ready"], false);
    assert_eq!(ready["reason"], "cooldown");
    assert_eq!(ready["days_remaining"], 14);

    let (status, detect) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/detect",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detect}");
    assert_eq!(detect["status"], "no_cluster");
    assert_eq!(detect["reason"], "cooldown");
    assert_eq!(detect["days_remaining"], 14);
}

#[tokio::test]
async fn validation_and_owner_contamination_refuse_before_writing_voiceprints() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity("target", false);
    journal.owner_centroid();
    journal.segment(json!({"labels":[{"sentence_id":1}]}));

    let (status, missing) = call(
        router(journal.0.clone()),
        "/app/speakers/api/assign-attribution",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(missing["reason_code"], "missing_required_field");

    let mut body = request();
    body["speaker"] = json!("target");
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/assign-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_owner_voice_too_close");
    assert!(!journal.0.join("entities/target/voiceprints.npz").exists());
}

#[tokio::test]
async fn every_route_rejects_its_required_fields() {
    let journal = Journal::new();
    for (path, body) in [
        ("/app/speakers/api/assign-attribution", json!({})),
        ("/app/speakers/api/confirm-attribution", json!({})),
        ("/app/speakers/api/correct-attribution", json!({})),
        ("/app/speakers/api/propagate-correction", json!({})),
    ] {
        let (status, refused) = call(router(journal.0.clone()), path, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {refused}");
        assert_eq!(refused["reason_code"], "missing_required_field");
    }
}

#[tokio::test]
async fn confirm_and_correct_preserve_the_reference_non_string_regex_failure_class() {
    let journal = Journal::new();
    for path in [
        "/app/speakers/api/confirm-attribution",
        "/app/speakers/api/correct-attribution",
    ] {
        let mut body = request();
        body["day"] = json!({"not":"a string"});
        if path.ends_with("correct-attribution") {
            body["new_speaker"] = json!("target");
        }
        let (status, refused) = call(router(journal.0.clone()), path, body).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path}: {refused}"
        );
        assert_eq!(refused["reason_code"], "internal_error");
    }
}

#[tokio::test]
async fn assign_cleanly_refuses_non_string_route_fields() {
    let journal = Journal::new();
    for (field, expected) in [
        ("day", "invalid_day"),
        ("segment_key", "invalid_segment_or_stream"),
        ("stream", "invalid_segment_or_stream"),
    ] {
        let mut body = request();
        body["speaker"] = json!("target");
        body[field] = json!({"not":"a string"});
        let (status, refused) = call(
            router(journal.0.clone()),
            "/app/speakers/api/assign-attribution",
            body,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {refused}");
        assert_eq!(refused["reason_code"], expected, "{field}: {refused}");
    }
}

#[tokio::test]
async fn invalid_stream_is_refused_by_every_attribution_write_route() {
    for path in [
        "/app/speakers/api/assign-attribution",
        "/app/speakers/api/confirm-attribution",
        "/app/speakers/api/correct-attribution",
    ] {
        let journal = Journal::new();
        let mut body = request();
        body["stream"] = json!("Upper case");
        if path.ends_with("assign-attribution") {
            body["speaker"] = json!("target");
        }
        if path.ends_with("correct-attribution") {
            body["new_speaker"] = json!("target");
        }
        let (status, refused) = call(router(journal.0.clone()), path, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {refused}");
        assert_eq!(refused["reason_code"], "invalid_segment_or_stream");
    }
}

#[tokio::test]
async fn correct_reports_no_old_speaker_in_its_propagation_offer() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.segment(json!({"labels":[{"sentence_id":1}]}));
    let mut body = request();
    body["new_speaker"] = json!("owner");
    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/correct-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["propagation_offer"],
        json!({
            "available":false,
            "reason":"no_old_speaker",
            "statement_count":0,
            "segment_count":0,
        })
    );
}

#[tokio::test]
async fn correct_reports_a_real_nonzero_propagation_preview() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity("old", false);
    journal.entity("new", false);
    journal.owner_centroid();
    journal.voiceprint("new", unit(0.0, 1.0));
    journal.segment_at(
        SEGMENT,
        json!({"labels":[{"sentence_id":1,"speaker":"old","confidence":"high","method":"user_assigned"}]}),
        unit(0.0, 1.0),
    );
    journal.segment_at(
        "120100_1",
        json!({"labels":[{"sentence_id":1,"speaker":"old","confidence":"high","method":"user_assigned"}]}),
        unit(0.0, 1.0),
    );
    let mut body = request();
    body["new_speaker"] = json!("new");
    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/correct-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let offer = &response["propagation_offer"];
    assert_eq!(offer["available"], true, "{offer}");
    assert!(
        offer["statement_count"].as_u64().unwrap_or(0) > 0,
        "{offer}"
    );
    assert!(offer["segment_count"].as_u64().unwrap_or(0) > 0, "{offer}");
    assert_eq!(offer["route"], "/app/speakers/api/propagate-correction");
    assert_eq!(
        offer["request"],
        json!({"old_speaker":"old","new_speaker":"new","commit":false})
    );
}

#[tokio::test]
async fn propagate_allows_legacy_old_speakers_but_requires_an_admitted_new_speaker() {
    for (old_speaker, legacy_entity) in [
        (
            "tool",
            Some(json!({"id":"tool","name":"tool","type":"Tool"})),
        ),
        (
            "blocked",
            Some(json!({"id":"blocked","name":"blocked","type":"Person","blocked":true})),
        ),
        (
            "missing_type",
            Some(json!({"id":"missing_type","name":"missing type","is_principal":false})),
        ),
        ("missing", None),
    ] {
        let journal = Journal::new();
        journal.entity("owner", true);
        journal.entity("new", false);
        if let Some(entity) = legacy_entity {
            journal.entity_value(old_speaker, entity);
        }
        journal.owner_centroid();
        journal.voiceprint("new", unit(0.0, 1.0));
        journal.segment(json!({"labels":[{"sentence_id":1,"speaker":old_speaker,"confidence":"high","method":"user_assigned"}]}));

        let (status, response) = call(
            router(journal.0.clone()),
            "/app/speakers/api/propagate-correction",
            json!({"old_speaker":old_speaker,"new_speaker":"new","commit":false}),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{old_speaker}: {response}");
        assert_eq!(response["status"], "preview", "{old_speaker}: {response}");
        assert!(
            response["statement_count"].as_u64().unwrap_or(0) > 0,
            "{old_speaker}: {response}"
        );
        assert!(
            !journal
                .0
                .join("entities")
                .join(old_speaker)
                .join("voiceprints.npz")
                .exists(),
            "propagation must not create a voiceprint for the historical selection key"
        );
    }

    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity("new", false);
    journal.owner_centroid();
    journal.segment_at(
        SEGMENT,
        json!({"labels":[{"sentence_id":1,"speaker":"legacy","confidence":"medium","method":"acoustic"}]}),
        unit(0.0, 1.0),
    );

    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/propagate-correction",
        json!({"old_speaker":"legacy","new_speaker":"new","commit":false}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "preview", "{response}");
    assert_eq!(
        response["changes"][0]["to_speaker"],
        Value::Null,
        "propagation must re-resolve an unmatched legacy label instead of relabeling it to new"
    );
    assert!(
        !journal.0.join("entities/legacy/voiceprints.npz").exists(),
        "propagation must not create a voiceprint for the legacy value"
    );

    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity_value("tool", json!({"id":"tool","name":"tool","type":"Tool"}));
    journal.entity_value(
        "project",
        json!({"id":"project","name":"project","type":"Project"}),
    );
    journal.segment(json!({"labels":[{"sentence_id":1,"speaker":"tool","confidence":"high","method":"user_assigned"}]}));
    let before = content_snapshot(&journal.0);

    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/propagate-correction",
        json!({"old_speaker":"tool","new_speaker":"project","commit":false}),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["reason_code"], "speaker_not_person");
    assert_eq!(content_snapshot(&journal.0), before);
}

#[tokio::test]
async fn propagate_refuses_an_invalid_owner_without_writes_in_preview_or_commit() {
    for commit in [false, true] {
        let journal = build_person_admission_journal(PersonAdmissionMode::MissingTypePrincipal);
        fs::write(
            journal.segment().join("talents/speaker_labels.json"),
            json!({"labels":[{"sentence_id":1,"speaker":"legacy","confidence":"high","method":"user_assigned"}]}).to_string(),
        )
        .expect("labels");
        // Materialize the lock file before snapshotting so the route's lock is not a mutation.
        let trust = solstone_core_entity::hold_entity_trust_lock(journal.root())
            .expect("initialize entity trust lock");
        drop(trust);
        let before = snapshot_files(journal.root());

        let (status, refused) = call(
            router(journal.root().to_path_buf()),
            "/app/speakers/api/propagate-correction",
            json!({"old_speaker":"legacy","new_speaker":"person","commit":commit}),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "commit={commit}: {refused}"
        );
        assert_eq!(
            refused["reason_code"], OWNER_IDENTITY_INVALID_REASON,
            "commit={commit}: {refused}"
        );
        for field in [
            "status",
            "statement_count",
            "segment_count",
            "changes",
            "segments",
        ] {
            assert!(
                refused.get(field).is_none(),
                "commit={commit} returned a non-top-level refusal field {field}: {refused}"
            );
        }
        assert_eq!(
            snapshot_files(journal.root()),
            before,
            "commit={commit} mutated the journal"
        );
    }
}

#[test]
fn propagation_commit_waits_for_the_entity_trust_lock() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity("new", false);
    journal.owner_centroid();
    journal.voiceprint("new", unit(0.0, 1.0));
    journal.segment_at(
        SEGMENT,
        json!({"labels":[{"sentence_id":1,"speaker":"legacy","confidence":"medium","method":"acoustic"}]}),
        unit(0.0, 1.0),
    );
    let trust =
        solstone_core_entity::hold_entity_trust_lock(&journal.0).expect("hold entity trust lock");
    let root = journal.0.clone();
    let (started_sender, started_receiver) = mpsc::channel();
    let (result_sender, result_receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        started_sender.send(()).expect("worker starts");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("worker runtime");
        result_sender
            .send(runtime.block_on(call(
                router(root),
                "/app/speakers/api/propagate-correction",
                json!({"old_speaker":"legacy","new_speaker":"new","commit":true}),
            )))
            .expect("worker reports propagation result");
    });
    started_receiver
        .recv_timeout(StdDuration::from_secs(1))
        .expect("worker starts before the lock assertion");
    assert_eq!(
        result_receiver.recv_timeout(StdDuration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "commit propagation must wait for the entity trust lock"
    );

    drop(trust);
    let (status, result) = result_receiver
        .recv_timeout(StdDuration::from_secs(2))
        .expect("commit finishes after the lock releases");
    worker.join().expect("propagation worker joins");
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["status"], "applied", "{result}");
}

#[tokio::test(flavor = "current_thread")]
async fn propagation_keeps_owner_reconfiguration_outside_the_operation_boundary() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity("new", false);
    journal.owner_centroid();
    journal.voiceprint("new", unit(0.0, 1.0));
    journal.segment_at(
        SEGMENT,
        json!({"labels":[{"sentence_id":1,"speaker":"legacy","confidence":"medium","method":"acoustic"}]}),
        unit(0.0, 1.0),
    );

    // The route is synchronous after parsing, so this holds the same reentrant lock
    // around the exact helper, segment writes, and action append that the route uses.
    let trust =
        solstone_core_entity::hold_entity_trust_lock(&journal.0).expect("hold entity trust lock");
    let root = journal.0.clone();
    let (started_sender, started_receiver) = mpsc::channel();
    let (changed_sender, changed_receiver) = mpsc::channel();
    let reconfigure = thread::spawn(move || {
        started_sender.send(()).expect("reconfiguration starts");
        let _trust = solstone_core_entity::hold_entity_trust_lock(&root)
            .expect("reconfiguration holds entity trust lock");
        fs::write(
            root.join("entities/owner/entity.json"),
            json!({"id":"owner","name":"owner","is_principal":true}).to_string(),
        )
        .expect("owner reconfiguration writes identity");
        changed_sender
            .send(())
            .expect("reconfiguration reports change");
    });
    started_receiver
        .recv_timeout(StdDuration::from_secs(1))
        .expect("reconfiguration starts before propagation");

    let (status, applied) = call(
        router(journal.0.clone()),
        "/app/speakers/api/propagate-correction",
        json!({"old_speaker":"legacy","new_speaker":"new","commit":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["status"], "applied", "{applied}");
    assert_eq!(applied["statement_count"], 1, "{applied}");
    let labels: Value = serde_json::from_slice(
        &fs::read(
            journal
                .0
                .join("chronicle")
                .join(DAY)
                .join(STREAM)
                .join(SEGMENT)
                .join("talents/speaker_labels.json"),
        )
        .expect("propagated labels"),
    )
    .expect("propagated labels parse");
    assert_eq!(
        labels["labels"][0]["speaker"], applied["changes"][0]["to_speaker"],
        "the completed write must be exactly the resolver's result"
    );
    assert_eq!(
        changed_receiver.recv_timeout(StdDuration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "owner reconfiguration must remain blocked through the operation boundary"
    );

    drop(trust);
    changed_receiver
        .recv_timeout(StdDuration::from_secs(1))
        .expect("owner reconfiguration completes after propagation");
    reconfigure.join().expect("reconfiguration worker joins");
    let before_next_operation = snapshot_files(&journal.0);

    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/propagate-correction",
        json!({"old_speaker":"legacy","new_speaker":"new","commit":false}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["reason_code"], OWNER_IDENTITY_INVALID_REASON);
    assert_eq!(
        snapshot_files(&journal.0),
        before_next_operation,
        "the next identity-invalid operation must not leave a stale partial write"
    );
}

#[tokio::test]
async fn attribution_trust_lock_timeout_is_the_python_compatible_labels_busy_refusal() {
    let journal = Journal::new();
    journal.segment(json!({"labels":[{"sentence_id":1}]}));
    let _held = solstone_core_entity::hold_entity_trust_lock_raw_for_test(&journal.0)
        .expect("hold trust lock outside the route coordinator");
    let mut body = request();
    body["speaker"] = json!("target");
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/assign-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_labels_busy");
}

#[tokio::test]
async fn indeterminate_owner_screen_refuses_assign_confirm_and_correct_without_writes() {
    // No admitted owner deliberately produces the native identity-invalid indeterminate result.
    // This is the AC3 falsification oracle: changing the route's Indeterminate arm to allow
    // would make each of these three refusal assertions fail and create a voiceprint.
    for (path, labels, body) in [
        (
            "/app/speakers/api/assign-attribution",
            json!({"labels":[{"sentence_id":1}]}),
            {
                let mut body = request();
                body["speaker"] = json!("target");
                body
            },
        ),
        (
            "/app/speakers/api/confirm-attribution",
            json!({"labels":[{"sentence_id":1,"speaker":"target","confidence":"medium","method":"acoustic"}]}),
            request(),
        ),
        (
            "/app/speakers/api/correct-attribution",
            json!({"labels":[{"sentence_id":1,"speaker":"other","confidence":"medium","method":"acoustic"}]}),
            {
                let mut body = request();
                body["new_speaker"] = json!("target");
                body
            },
        ),
    ] {
        let journal = Journal::new();
        journal.entity("target", false);
        journal.entity("other", false);
        journal.segment(labels);
        let (status, refused) = call(router(journal.0.clone()), path, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {refused}");
        assert_eq!(
            refused["reason_code"], "speaker_owner_identity_invalid",
            "{path}: {refused}"
        );
        assert_ne!(refused["reason_code"], "speaker_owner_voice_too_close");
        assert!(!journal.0.join("entities/target/voiceprints.npz").exists());
    }
}

fn direct_request() -> Value {
    let mut body = request();
    body["stream"] = json!("_default");
    body["stream_layout"] = json!("direct");
    body
}

fn named_default_request() -> Value {
    let mut body = request();
    body["stream"] = json!("_default");
    body["stream_layout"] = json!("named");
    body
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

#[tokio::test]
async fn assign_confirm_and_correct_mutate_direct_and_named_twins_independently() {
    for (path, labels, direct_body, named_body, expected_status) in [
        (
            "/app/speakers/api/assign-attribution",
            json!({"labels":[{"sentence_id":1}]}),
            {
                let mut body = direct_request();
                body["speaker"] = json!("owner");
                body
            },
            {
                let mut body = named_default_request();
                body["speaker"] = json!("owner");
                body
            },
            "assigned",
        ),
        (
            "/app/speakers/api/confirm-attribution",
            json!({"labels":[{"sentence_id":1,"speaker":"owner","confidence":"medium","method":"acoustic"}]}),
            direct_request(),
            named_default_request(),
            "confirmed",
        ),
        (
            "/app/speakers/api/correct-attribution",
            json!({"labels":[{"sentence_id":1,"speaker":"other","confidence":"medium","method":"acoustic"}]}),
            {
                let mut body = direct_request();
                body["new_speaker"] = json!("owner");
                body
            },
            {
                let mut body = named_default_request();
                body["new_speaker"] = json!("owner");
                body
            },
            "corrected",
        ),
    ] {
        let journal = Journal::new();
        journal.entity("owner", true);
        journal.entity("other", false);
        journal.owner_centroid();

        let direct_dir = journal.0.join("chronicle").join(DAY).join(SEGMENT);
        let named_dir = journal
            .0
            .join("chronicle")
            .join(DAY)
            .join("_default")
            .join(SEGMENT);
        journal.direct_segment_at(DAY, SEGMENT, labels.clone());
        journal.named_default_segment_at(DAY, SEGMENT, labels.clone());

        // 1. Mutate Direct twin
        let named_snapshot = crate::support::snapshot_files(&named_dir);
        let (status, response) = call(router(journal.0.clone()), path, direct_body).await;
        assert_eq!(status, StatusCode::OK, "{path}: {response}");
        assert_eq!(response["status"], expected_status, "{path}: {response}");
        assert_eq!(response["stream_layout"], "direct", "{path}: {response}");
        assert_eq!(
            crate::support::snapshot_files(&named_dir),
            named_snapshot,
            "{path}: named twin was modified by direct mutation"
        );
        let direct_labels: Value = serde_json::from_str(
            &fs::read_to_string(direct_dir.join("talents/speaker_labels.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(direct_labels["labels"][0]["speaker"], "owner", "{path}");
        assert!(
            has_voiceprint_metadata(&journal.0, "owner", DAY, "_default", SEGMENT, "direct"),
            "{path}: direct voiceprint present after direct mutation"
        );
        assert!(
            !has_voiceprint_metadata(&journal.0, "owner", DAY, "_default", SEGMENT, "named"),
            "{path}: named voiceprint absent before named mutation"
        );

        // 2. Mutate Named twin
        let direct_snapshot = crate::support::snapshot_files(&direct_dir);
        let (status, response) = call(router(journal.0.clone()), path, named_body).await;
        assert_eq!(status, StatusCode::OK, "{path}: {response}");
        assert_eq!(response["status"], expected_status, "{path}: {response}");
        assert_eq!(response["stream_layout"], "named", "{path}: {response}");
        assert_eq!(
            crate::support::snapshot_files(&direct_dir),
            direct_snapshot,
            "{path}: direct twin was modified by named mutation"
        );
        let named_labels: Value = serde_json::from_str(
            &fs::read_to_string(named_dir.join("talents/speaker_labels.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(named_labels["labels"][0]["speaker"], "owner", "{path}");
        assert!(
            has_voiceprint_metadata(&journal.0, "owner", DAY, "_default", SEGMENT, "direct"),
            "{path}: direct voiceprint still present after named mutation"
        );
        assert!(
            has_voiceprint_metadata(&journal.0, "owner", DAY, "_default", SEGMENT, "named"),
            "{path}: named voiceprint present after named mutation"
        );

        // 3. Collapse refusal in both directions
        let direct_only_key = "120000_direct_only";
        journal.direct_segment_at(DAY, direct_only_key, labels.clone());
        let before = crate::support::snapshot_files(&journal.0);
        let mut collapse_named = named_default_request();
        collapse_named["segment_key"] = json!(direct_only_key);
        if path == "/app/speakers/api/assign-attribution" {
            collapse_named["speaker"] = json!("owner");
        } else if path == "/app/speakers/api/correct-attribution" {
            collapse_named["new_speaker"] = json!("owner");
        }
        let (status, refused) = call(router(journal.0.clone()), path, collapse_named).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {refused}");
        assert_eq!(
            refused["reason_code"], "speaker_review_unavailable",
            "{path}"
        );
        assert_eq!(crate::support::snapshot_files(&journal.0), before);

        let named_only_key = "120000_named_only";
        journal.named_default_segment_at(DAY, named_only_key, labels.clone());
        let before = crate::support::snapshot_files(&journal.0);
        let mut collapse_direct = direct_request();
        collapse_direct["segment_key"] = json!(named_only_key);
        if path == "/app/speakers/api/assign-attribution" {
            collapse_direct["speaker"] = json!("owner");
        } else if path == "/app/speakers/api/correct-attribution" {
            collapse_direct["new_speaker"] = json!("owner");
        }
        let (status, refused) = call(router(journal.0.clone()), path, collapse_direct).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {refused}");
        assert_eq!(
            refused["reason_code"], "speaker_review_unavailable",
            "{path}"
        );
        assert_eq!(crate::support::snapshot_files(&journal.0), before);
    }
}

#[tokio::test]
async fn propagation_applies_across_direct_and_named_targets() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.entity("old", false);
    journal.entity("new", false);
    journal.owner_centroid();

    // 1. Direct target matching "old"
    journal.direct_segment_at(
        "20260809",
        "120000_1",
        json!({"labels":[{"sentence_id":1,"speaker":"old"}]}),
    );
    // 2. Named target matching "old"
    journal.named_default_segment_at(
        "20260809",
        "120000_2",
        json!({"labels":[{"sentence_id":1,"speaker":"old"}]}),
    );
    // 3. Unaddressed Named twin of 120000_1
    journal.named_default_segment_at(
        "20260809",
        "120000_1",
        json!({"labels":[{"sentence_id":1,"speaker":"other"}]}),
    );
    // 4. Unaddressed Direct twin of 120000_2
    journal.direct_segment_at(
        "20260809",
        "120000_2",
        json!({"labels":[{"sentence_id":1,"speaker":"other"}]}),
    );

    let unaddressed_named_dir = journal.0.join("chronicle/20260809/_default/120000_1");
    let unaddressed_direct_dir = journal.0.join("chronicle/20260809/120000_2");
    let unaddressed_named_snapshot = crate::support::snapshot_files(&unaddressed_named_dir);
    let unaddressed_direct_snapshot = crate::support::snapshot_files(&unaddressed_direct_dir);

    let (status, response) = call(
        router(journal.0.clone()),
        "/app/speakers/api/propagate-correction",
        json!({"old_speaker":"old","new_speaker":"new","commit":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "applied", "{response}");
    assert_eq!(response["statement_count"], 2, "{response}");

    // Assert per-row actual layout in segments
    let segments = response["segments"].as_array().expect("segments array");
    let res1 = segments
        .iter()
        .find(|r| r["segment_key"] == "120000_1")
        .expect("res 120000_1");
    assert_eq!(res1["stream_layout"], "direct", "{res1}");
    let res2 = segments
        .iter()
        .find(|r| r["segment_key"] == "120000_2")
        .expect("res 120000_2");
    assert_eq!(res2["stream_layout"], "named", "{res2}");

    // Assert per-row actual layout in changes
    let changes = response["changes"].as_array().expect("changes array");
    let ch1 = changes
        .iter()
        .find(|c| c["segment_key"] == "120000_1")
        .expect("ch 120000_1");
    assert_eq!(ch1["stream_layout"], "direct", "{ch1}");
    let ch2 = changes
        .iter()
        .find(|c| c["segment_key"] == "120000_2")
        .expect("ch 120000_2");
    assert_eq!(ch2["stream_layout"], "named", "{ch2}");

    // Assert unaddressed twins are byte-identical
    assert_eq!(
        crate::support::snapshot_files(&unaddressed_named_dir),
        unaddressed_named_snapshot
    );
    assert_eq!(
        crate::support::snapshot_files(&unaddressed_direct_dir),
        unaddressed_direct_snapshot
    );
}

#[tokio::test]
async fn attribution_direct_trust_lock_timeout_is_the_python_compatible_labels_busy_refusal() {
    let journal = Journal::new();
    journal.direct_segment(json!({"labels":[{"sentence_id":1}]}));
    let _held = solstone_core_entity::hold_entity_trust_lock_raw_for_test(&journal.0)
        .expect("hold trust lock outside the route coordinator");
    let mut body = direct_request();
    body["speaker"] = json!("target");
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/assign-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
    assert_eq!(refused["reason_code"], "speaker_labels_busy");
}

#[tokio::test]
async fn attribution_malformed_stream_layout_is_not_named() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.segment(json!({"labels":[{"sentence_id":1}]}));
    for layout in [
        json!("Direct"),
        json!(""),
        json!(true),
        json!(1),
        Value::Null,
    ] {
        let mut body = request();
        body["speaker"] = json!("owner");
        body["stream_layout"] = layout.clone();
        let (status, refused) = call(
            router(journal.0.clone()),
            "/app/speakers/api/assign-attribution",
            body,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{layout}: {refused}");
        assert_eq!(
            refused["reason_code"], "invalid_segment_or_stream",
            "{layout}: {refused}"
        );
    }

    // Missing stream_layout field
    let mut body = request();
    body["speaker"] = json!("owner");
    body.as_object_mut().unwrap().remove("stream_layout");
    let (status, refused) = call(
        router(journal.0.clone()),
        "/app/speakers/api/assign-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing layout: {refused}");
    assert_eq!(
        refused["reason_code"], "invalid_segment_or_stream",
        "missing layout: {refused}"
    );
}

#[tokio::test]
async fn classify_reads_a_direct_segment() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    journal.direct_segment(json!({"labels":[{"sentence_id":1}]}));
    let (status, body) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/classify",
        json!({
            "day": DAY,
            "stream": "_default",
            "segment_key": SEGMENT,
            "source": SOURCE,
            "stream_layout": "direct",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["sentences"].as_array().map(Vec::len),
        Some(1),
        "{body}"
    );
}

#[tokio::test]
async fn person_admission_refusals_precede_idempotency_and_leave_the_journal_unchanged() {
    for (speaker, labels, status, reason) in [
        (
            "tool",
            json!({"labels":[{"sentence_id":1,"speaker":"tool","confidence":"high","method":"user_assigned"}]}),
            StatusCode::BAD_REQUEST,
            "speaker_not_person",
        ),
        (
            "project",
            json!({"labels":[{"sentence_id":1}]}),
            StatusCode::BAD_REQUEST,
            "speaker_not_person",
        ),
        (
            "company",
            json!({"labels":[{"sentence_id":1}]}),
            StatusCode::BAD_REQUEST,
            "speaker_not_person",
        ),
        (
            "blocked_person",
            json!({"labels":[{"sentence_id":1}]}),
            StatusCode::BAD_REQUEST,
            "entity_blocked",
        ),
        (
            "malformed",
            json!({"labels":[{"sentence_id":1}]}),
            StatusCode::NOT_FOUND,
            "speaker_not_found",
        ),
        (
            "missing",
            json!({"labels":[{"sentence_id":1}]}),
            StatusCode::NOT_FOUND,
            "speaker_not_found",
        ),
    ] {
        let journal = build_person_admission_journal(PersonAdmissionMode::Valid);
        set_labels(journal.root(), labels);
        let before = content_snapshot(journal.root());
        let mut body = request();
        body["speaker"] = json!(speaker);
        let (actual_status, response) = call(
            router(journal.root().to_path_buf()),
            "/app/speakers/api/assign-attribution",
            body,
        )
        .await;
        assert_eq!(actual_status, status, "{speaker}: {response}");
        assert_eq!(response["reason_code"], reason, "{speaker}: {response}");
        assert_eq!(content_snapshot(journal.root()), before, "{speaker}");
    }

    let journal = build_person_admission_journal(PersonAdmissionMode::Valid);
    set_labels(
        journal.root(),
        json!({"labels":[{"sentence_id":1,"speaker":"tool","confidence":"high","method":"user_confirmed"}]}),
    );
    let before = content_snapshot(journal.root());
    let (status, response) = call(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/confirm-attribution",
        request(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["reason_code"], "speaker_not_person");
    assert_eq!(content_snapshot(journal.root()), before);

    let journal = build_person_admission_journal(PersonAdmissionMode::Valid);
    let before = content_snapshot(journal.root());
    let mut body = request();
    body["new_speaker"] = json!("project");
    let (status, response) = call(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/correct-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["reason_code"], "speaker_not_person");
    assert_eq!(content_snapshot(journal.root()), before);

    let journal = build_person_admission_journal(PersonAdmissionMode::Valid);
    let mut body = request();
    body["speaker"] = json!("person");
    let (status, response) = call(
        router(journal.root().to_path_buf()),
        "/app/speakers/api/assign-attribution",
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "assigned");
}

#[tokio::test]
async fn correct_repairs_legacy_invalid_old_speakers_into_an_admitted_person() {
    for old_speaker in ["tool", "blocked_person", "malformed", "deleted"] {
        let journal = build_person_admission_journal(PersonAdmissionMode::Valid);
        if old_speaker == "tool" {
            solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
                journal.root(),
                old_speaker,
                unit(0.0, 1.0),
                json!({"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"source":SOURCE,"sentence_id":1}),
                &solstone_core_entity::EncoderIdentity {
                    id: "unresolved".to_owned(),
                    sha256: "0".repeat(64),
                    width: 256,
                },
            )
            .expect("legacy tool voiceprint writes");
        }
        set_labels(
            journal.root(),
            json!({"labels":[{"sentence_id":1,"speaker":old_speaker,"confidence":"medium","method":"acoustic"}]}),
        );
        let mut body = request();
        body["new_speaker"] = json!("person");
        let (status, response) = call(
            router(journal.root().to_path_buf()),
            "/app/speakers/api/correct-attribution",
            body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{old_speaker}: {response}");
        assert_eq!(response["status"], "corrected");
        assert_eq!(response["old_speaker"], old_speaker);
        assert_eq!(response["new_speaker"], "person");

        let corrections: Value = serde_json::from_slice(
            &fs::read(journal.segment().join("talents/speaker_corrections.json"))
                .expect("corrections read"),
        )
        .expect("corrections parse");
        assert_eq!(
            corrections["corrections"][0]["original_speaker"],
            old_speaker
        );
        if old_speaker == "tool" {
            assert!(
                !journal
                    .root()
                    .join("entities/tool/voiceprints.npz")
                    .exists()
            );
        } else {
            assert_eq!(response["voiceprint_removal"]["outcome"], "not_found");
        }
    }
}

#[tokio::test]
async fn owner_confirm_evidentiary_rejections() {
    let journal = Journal::new();
    journal.entity("owner", true);

    let emb = unit(1.0, 0.0);
    let mut le = Vec::new();
    for v in &emb {
        le.extend_from_slice(&v.to_le_bytes());
    }
    let emb_sha = format!("{:x}", Sha256::digest(&le));
    let audio_bytes = b"RIFFaudiofakebytes";
    let audio_sha = format!("{:x}", Sha256::digest(audio_bytes));

    let seg_dir = journal
        .0
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(&seg_dir).expect("seg dir");
    fs::write(
        seg_dir.join("audio.jsonl"),
        "{\"raw\":\"audio.flac\"}\n{\"sentence_id\":0,\"text\":\"test\"}\n",
    )
    .expect("jsonl");
    fs::write(seg_dir.join("audio.flac"), audio_bytes).expect("flac");
    write_embeddings(&seg_dir.join("audio.npz"), std::slice::from_ref(&emb));

    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate(
        &journal.0,
        &solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
            centroid: emb.clone(),
            cluster_size: 5,
            threshold: 0.43,
            version: "v-candidate-1".to_owned(),
            evidence_tier: "standard".to_owned(),
        },
    )
    .expect("candidate");

    fs::create_dir_all(journal.0.join("awareness")).expect("awareness");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "v-candidate-1",
                "recommendation": "ready",
                "cluster_size": 5,
                "streams_represented": 2,
                "samples": [{
                    "day": DAY,
                    "stream_layout": "named",
                    "stream": STREAM,
                    "segment_key": SEGMENT,
                    "source": "audio",
                    "sentence_id": 0,
                    "version": "v-candidate-1",
                    "embedding_sha256": emb_sha,
                    "audio_sha256": audio_sha,
                }]
            }
        })
        .to_string(),
    )
    .expect("state");

    // 1. Hash mismatch
    let (status, resp) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/confirm",
        json!({
            "version": "v-candidate-1",
            "day": DAY,
            "stream_layout": "named",
            "stream": STREAM,
            "segment_key": SEGMENT,
            "source": "audio",
            "sentence_id": 0,
            "audio_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(resp["reason_code"], "speaker_candidate_unverified");

    // 2. Version mismatch
    let (status, resp) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/confirm",
        json!({
            "version": "v-candidate-wrong",
            "day": DAY,
            "stream_layout": "named",
            "stream": STREAM,
            "segment_key": SEGMENT,
            "source": "audio",
            "sentence_id": 0,
            "audio_sha256": audio_sha,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(resp["reason_code"], "speaker_candidate_stale_version");
}

#[tokio::test]
async fn owner_set_aside_clears_candidate_and_sets_no_cluster() {
    let journal = Journal::new();
    journal.entity("owner", true);

    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate(
        &journal.0,
        &solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
            centroid: unit(1.0, 0.0),
            cluster_size: 5,
            threshold: 0.43,
            version: "v-aside-1".to_owned(),
            evidence_tier: "standard".to_owned(),
        },
    )
    .expect("candidate");

    fs::create_dir_all(journal.0.join("awareness")).expect("awareness");
    fs::write(
        journal.0.join("awareness/current.json"),
        json!({
            "voiceprint": {
                "status": "candidate",
                "detected_at": "v-aside-1",
                "recommendation": "ready",
                "cluster_size": 5,
                "streams_represented": 2,
                "samples": []
            }
        })
        .to_string(),
    )
    .expect("state");

    let (status, resp) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/set-aside",
        json!({"version": "v-aside-1"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["status"], "no_cluster");
    assert!(!journal.0.join("awareness/owner_candidate.npz").exists());

    let awareness: Value = serde_json::from_str(
        &fs::read_to_string(journal.0.join("awareness/current.json")).expect("awareness read"),
    )
    .expect("awareness parse");
    assert_eq!(awareness["voiceprint"]["status"], "no_cluster");
    assert!(awareness["voiceprint"]["detected_at"].is_null());
}
