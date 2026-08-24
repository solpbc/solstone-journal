// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mutation-route coverage for native speaker attribution and owner screening.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use solstone_core_npy::write_npy;
use tower::ServiceExt;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::support::build_person_admission_journal;

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

fn request() -> Value {
    json!({"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"source":SOURCE,"sentence_id":1})
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

    solstone_core_speaker_resolve::owner_candidate::write_owner_candidate(
        &journal.0,
        &solstone_core_speaker_resolve::owner_candidate::OwnerCandidate {
            centroid: unit(1.0, 0.0),
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
        json!({"voiceprint":{"status":"candidate","recommendation":"ready","cluster_size":5,"streams_represented":2}}).to_string(),
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
        json!({}),
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

    let (status, rejected) = call(
        router(journal.0.clone()),
        "/app/speakers/api/owner/reject",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["status"], "needs_detection");
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
    }

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
    // No principal deliberately produces the native `no_principal` indeterminate result.
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
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {refused}");
        assert_eq!(
            refused["reason_code"], "speaker_owner_centroid_required",
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
async fn assign_confirm_and_correct_refuse_direct_layout_without_writes() {
    for (path, labels, body) in [
        (
            "/app/speakers/api/assign-attribution",
            json!({"labels":[{"sentence_id":1}]}),
            {
                let mut body = direct_request();
                body["speaker"] = json!("owner");
                body
            },
        ),
        (
            "/app/speakers/api/confirm-attribution",
            json!({"labels":[{"sentence_id":1,"speaker":"owner","confidence":"medium","method":"acoustic"}]}),
            direct_request(),
        ),
        (
            "/app/speakers/api/correct-attribution",
            json!({"labels":[{"sentence_id":1,"speaker":"other","confidence":"medium","method":"acoustic"}]}),
            {
                let mut body = direct_request();
                body["new_speaker"] = json!("owner");
                body
            },
        ),
    ] {
        let journal = Journal::new();
        journal.entity("owner", true);
        journal.entity("other", false);
        journal.direct_segment(labels);
        let before = crate::support::snapshot_files(&journal.0);
        let (status, refused) = call(router(journal.0.clone()), path, body).await;
        assert_direct_refused(status, &refused);
        assert_eq!(
            crate::support::snapshot_files(&journal.0),
            before,
            "{path} wrote the journal"
        );
    }
}

#[tokio::test]
async fn propagation_preflights_direct_and_mixed_targets_before_resolver_or_writes() {
    for mixed in [false, true] {
        let journal = Journal::new();
        journal.entity("old", false);
        journal.entity("new", false);
        if mixed {
            journal.segment(json!({"labels":[{"sentence_id":1,"speaker":"old"}]}));
            fs::write(
                journal
                    .0
                    .join("chronicle")
                    .join(DAY)
                    .join(STREAM)
                    .join(SEGMENT)
                    .join("audio.npz"),
                b"not an npz archive",
            )
            .expect("invalid named evidence writes");
        }
        journal.direct_segment_at(
            "20260809",
            "120000_1",
            json!({"labels":[{"sentence_id":1,"speaker":"old"}]}),
        );
        let before = crate::support::snapshot_files(&journal.0);
        let (status, refused) = call(
            router(journal.0.clone()),
            "/app/speakers/api/propagate-correction",
            json!({"old_speaker":"old","new_speaker":"new","commit":true}),
        )
        .await;
        assert_direct_refused(status, &refused);
        assert_eq!(
            crate::support::snapshot_files(&journal.0),
            before,
            "mixed={mixed} wrote the journal"
        );
    }
}

#[tokio::test]
async fn attribution_malformed_stream_layout_is_not_named() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.segment(json!({"labels":[{"sentence_id":1}]}));
    for layout in [json!("Direct"), json!(""), json!(true), json!(1)] {
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
        let journal = build_person_admission_journal();
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

    let journal = build_person_admission_journal();
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

    let journal = build_person_admission_journal();
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

    let journal = build_person_admission_journal();
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
        let journal = build_person_admission_journal();
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
