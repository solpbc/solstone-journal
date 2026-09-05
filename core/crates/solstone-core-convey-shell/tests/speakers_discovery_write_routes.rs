// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use solstone_core_npy::write_npy;
use tower::ServiceExt;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static DISCOVERY_HELPER: Once = Once::new();
const DAY: &str = "20260808";
const STREAM: &str = "main";
const SOURCE: &str = "audio";

struct Journal(PathBuf);
impl Journal {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "solstone-discovery-write-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("config")).expect("config");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("config");
        Self(root)
    }
    fn cache(&self, members: Value) {
        fs::create_dir_all(self.0.join("awareness")).expect("awareness");
        fs::write(
            self.0.join("awareness/discovery_clusters.json"),
            json!({"version":"x","clusters":{"7":members}}).to_string(),
        )
        .expect("cache");
    }

    fn cache_markers(&self) {
        self.cache(json!([{
            "day": DAY,
            "stream": STREAM,
            "segment_key": "120000_1",
            "source": SOURCE,
            "sentence_id": 1,
        }]));
        fs::write(
            self.0.join("awareness/discovery_clusters.resolved.json"),
            "resolved",
        )
        .expect("resolved marker");
    }

    fn entity(&self, id: &str, principal: bool) {
        let directory = self.0.join("entities").join(id);
        fs::create_dir_all(&directory).expect("entity directory");
        fs::write(
            directory.join("entity.json"),
            json!({"id":id,"name":id,"type":"Person","is_principal":principal}).to_string(),
        )
        .expect("entity");
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

    fn candidate_segment(&self, index: usize, labels: Value, embeddings: &[Vec<f32>]) {
        let directory = self
            .0
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(format!("120000_{index}"));
        fs::create_dir_all(directory.join("talents")).expect("talents");
        fs::write(
            directory.join("talents/speaker_labels.json"),
            labels.to_string(),
        )
        .expect("labels");
        write_embeddings(&directory.join("audio.npz"), embeddings);
    }

    fn direct_segment(&self, key: &str) {
        let directory = self.0.join("chronicle").join(DAY).join(key);
        fs::create_dir_all(directory.join("talents")).expect("direct talents");
        fs::write(directory.join("marker"), "unchanged").expect("direct marker");
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn call(root: &Path, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router(root.to_path_buf())
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&body).expect("json"))
}

fn members() -> Value {
    json!([
        {"source":"audio","sentence_id":10,"stream":"b","day":"20260102","segment_key":"2_1","ignored":true},
        {"day":"20260101","stream":"a","segment_key":"1_1","source":"audio","sentence_id":2},
        {"day":"20260102","stream":"b","segment_key":"2_1","source":"audio","sentence_id":10}
    ])
}

fn candidate_members(count: usize) -> Value {
    Value::Array(
        (1..=count)
            .map(|index| {
                json!({
                    "day": DAY,
                    "stream": STREAM,
                    "segment_key": format!("120000_{index}"),
                    "source": SOURCE,
                    "sentence_id": 1,
                })
            })
            .collect(),
    )
}

fn unit(first: f32, second: f32) -> Vec<f32> {
    let mut values = vec![0.0; 256];
    values[0] = first;
    values[1] = second;
    values
}

fn threshold_boundary() -> Vec<f32> {
    unit(0.43, (1.0_f32 - 0.43_f32.powi(2)).sqrt())
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

fn add_candidates(journal: &Journal, count: usize) {
    for index in 1..=count {
        journal.candidate_segment(index, json!({"labels":[]}), &[unit(0.0, 1.0)]);
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = fs::read_dir(path)
            .expect("snapshot directory")
            .map(|entry| entry.expect("snapshot entry"))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, output);
            } else {
                output.insert(
                    path.strip_prefix(root)
                        .expect("relative snapshot")
                        .to_path_buf(),
                    fs::read(&path).expect("snapshot file"),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}

fn install_discovery_helper() {
    DISCOVERY_HELPER.call_once(|| {
        let path = std::env::current_exe()
            .expect("current test executable")
            .parent()
            .expect("test executable parent")
            .join("solstone-core-speakers-analyze");
        fs::write(
            &path,
            r##"#!/bin/sh
request=$(cat)
case "$request" in
  *'"shape":[5,256]'*)
    printf '%s\n' '{"schema":"solstone-speaker-discovery-cluster-response-v1","labels":[0,0,0,0,0],"parameters":{"min_cluster_size":5,"min_samples":3},"algorithm":"hdbscan-eom-euclidean-f64-prim-mst","noise_count":0,"cluster_count":1}'
    ;;
  *'"shape":[6,256]'*) exit 17 ;;
  *'"shape":[7,256]'*) printf '%s\n' 'not-json' ;;
  *'"shape":[8,256]'*)
    printf '%s\n' '{"schema":"solstone-speaker-discovery-cluster-response-v1","labels":[-1,-1,-1,-1,-1,-1,-1,-1],"parameters":{"min_cluster_size":5,"min_samples":3},"algorithm":"hdbscan-eom-euclidean-f64-prim-mst","noise_count":8,"cluster_count":0}'
    ;;
  *) exit 18 ;;
esac
"##,
        )
        .expect("helper script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("helper mode");
        }
    });
}

#[tokio::test]
async fn dismiss_is_a_canonical_locked_jsonl_route() {
    let journal = Journal::new();
    journal.cache(members());
    let (status, body) = call(
        &journal.0,
        "/app/speakers/api/discovery/dismiss",
        json!({"cluster_id":7,"disposition":"quiet"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "dismissed");
    assert!(
        body["dismiss_event_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("cdev_"))
    );
    let line =
        fs::read_to_string(journal.0.join("speakers/cluster-dismissals.jsonl")).expect("event");
    let event: Value = serde_json::from_str(line.trim()).expect("event json");
    assert_eq!(event["member_count"], 2);
    assert_eq!(event["members"][0]["day"], "20260101");
    assert_eq!(event["dismiss_event_id"], body["dismiss_event_id"]);
}

#[tokio::test]
async fn dismiss_validates_and_requires_a_cached_cluster() {
    let journal = Journal::new();
    let (status, _) = call(&journal.0, "/app/speakers/api/discovery/dismiss", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    journal.cache(members());
    let (status, _) = call(
        &journal.0,
        "/app/speakers/api/discovery/dismiss",
        json!({"cluster_id":7,"disposition":"nope"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &journal.0,
        "/app/speakers/api/discovery/dismiss",
        json!({"cluster_id":8,"disposition":"quiet"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn identify_uses_the_standard_voiceprint_busy_response_for_a_held_trust_lock() {
    let journal = Journal::new();
    journal.candidate_segment(1, json!({"labels":[]}), &[unit(0.0, 1.0)]);
    journal.cache(candidate_members(1));
    let _held = solstone_core_entity::hold_entity_trust_lock_raw_for_test(&journal.0)
        .expect("hold trust lock outside the route coordinator");

    let (status, body) = call(
        &journal.0,
        "/app/speakers/api/discovery/identify",
        json!({"cluster_id":7,"name":"target"}),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["reason_code"], "speaker_voiceprint_busy");
    assert_eq!(
        body["error"],
        "I couldn't update that voice because another update is running."
    );
}

#[tokio::test]
async fn web_and_cli_identify_refuse_direct_members_before_any_write() {
    for route in [
        "/app/speakers/api/discovery/identify",
        "/app/speakers/api/discovery/identify-cli",
    ] {
        let journal = Journal::new();
        journal.direct_segment("120000_1");
        journal.cache(json!([{
            "day": DAY,
            "stream_layout": "direct",
            "stream": "_default",
            "segment_key": "120000_1",
            "source": SOURCE,
            "sentence_id": 1,
        }]));
        let before = snapshot(&journal.0);

        let (status, body) = call(
            &journal.0,
            route,
            json!({"cluster_id":7,"name":"target","create_new":true}),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{route}: {body}");
        assert_eq!(
            body["reason_code"], "speaker_segment_layout_unsupported",
            "{route}: {body}"
        );
        assert_eq!(
            body["error"],
            "This command can't change that speaker review."
        );
        assert_eq!(
            body["detail"],
            "This segment uses the direct journal layout, which this command doesn't support."
        );
        assert_eq!(snapshot(&journal.0), before, "{route} mutated the journal");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn scan_catalog_failure_preserves_both_cache_files() {
    use std::os::unix::fs::symlink;

    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    journal.cache_markers();
    let real = journal.0.join("outside-segment");
    fs::create_dir_all(&real).expect("real segment");
    let stream = journal.0.join("chronicle").join(DAY).join(STREAM);
    fs::create_dir_all(&stream).expect("stream");
    symlink(&real, stream.join("120000_1")).expect("segment symlink");
    let before = snapshot(&journal.0);

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["reason_code"], "speaker_command_failed");
    assert_eq!(
        snapshot(&journal.0),
        before,
        "failed scan changed the cache"
    );
}

#[tokio::test]
async fn scan_invalid_attributed_evidence_preserves_both_cache_files() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    journal.candidate_segment(
        1,
        json!({"labels":[{"speaker":"someone"}]}),
        &[unit(0.0, 1.0)],
    );
    journal.cache_markers();
    let before = snapshot(&journal.0);

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["reason_code"], "speaker_command_failed");
    assert_eq!(
        snapshot(&journal.0),
        before,
        "invalid evidence changed the cache"
    );
}

#[tokio::test]
async fn scan_invalid_owner_identity_preserves_both_cache_files() {
    let journal = Journal::new();
    journal.cache_markers();
    let before = snapshot(&journal.0);

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["reason_code"], "speaker_owner_identity_invalid");
    assert_eq!(snapshot(&journal.0), before);
}

#[tokio::test]
async fn identify_logs_the_app_action_after_a_successful_identification() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.candidate_segment(1, json!({"labels":[]}), &[unit(0.0, 1.0)]);
    journal.cache(candidate_members(1));

    let (status, body) = call(
        &journal.0,
        "/app/speakers/api/discovery/identify",
        json!({
            "cluster_id":7,
            "name":"target",
            "create_new":true,
            "request_id":"identify-action-log",
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "identified");
    let actions = fs::read_dir(journal.0.join("config/actions"))
        .expect("action directory")
        .flatten()
        .flat_map(|entry| {
            fs::read_to_string(entry.path())
                .expect("action log")
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .map(|line| serde_json::from_str::<Value>(&line).expect("action json"))
        .collect::<Vec<_>>();
    let action = actions
        .iter()
        .find(|entry| entry["action"] == "speaker_identified")
        .expect("speaker identify action");
    assert_eq!(action["params"]["entity_id"], body["entity_id"]);
    assert_eq!(action["params"]["cluster_id"], 7);
    assert_eq!(
        action["params"]["voiceprints_saved"],
        body["voiceprints_saved"]
    );
    assert_eq!(
        action["params"]["segments_updated"],
        body["segments_updated"]
    );
}

#[tokio::test]
async fn scan_without_a_confirmed_owner_leaves_cache_untouched() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.cache_markers();
    let cache = fs::read(journal.0.join("awareness/discovery_clusters.json")).expect("cache");
    let resolved =
        fs::read(journal.0.join("awareness/discovery_clusters.resolved.json")).expect("resolved");

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "degraded");
    assert_eq!(
        body["issues"][0]["reason_code"],
        "speaker_discovery_owner_voice_unavailable"
    );
    assert_eq!(
        fs::read(journal.0.join("awareness/discovery_clusters.json")).expect("cache"),
        cache
    );
    assert_eq!(
        fs::read(journal.0.join("awareness/discovery_clusters.resolved.json")).expect("resolved"),
        resolved
    );
}

#[tokio::test]
async fn scan_with_too_few_candidates_clears_both_cache_files() {
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    add_candidates(&journal, 4);
    journal.cache_markers();

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["clusters"], json!([]));
    assert!(!journal.0.join("awareness/discovery_clusters.json").exists());
    assert!(
        !journal
            .0
            .join("awareness/discovery_clusters.resolved.json")
            .exists()
    );
}

#[tokio::test]
async fn scan_helper_invoke_failure_is_retryable_and_preserves_cache() {
    install_discovery_helper();
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    add_candidates(&journal, 6);
    journal.cache_markers();
    let cache = fs::read(journal.0.join("awareness/discovery_clusters.json")).expect("cache");

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["reason_code"], "speaker_discovery_failed");
    assert_eq!(body["retryable"], true);
    assert_eq!(
        fs::read(journal.0.join("awareness/discovery_clusters.json")).expect("cache"),
        cache
    );
    assert!(
        journal
            .0
            .join("awareness/discovery_clusters.resolved.json")
            .exists()
    );
}

#[tokio::test]
async fn scan_helper_response_failure_is_not_retryable_and_preserves_cache() {
    install_discovery_helper();
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    add_candidates(&journal, 7);
    journal.cache_markers();
    let cache = fs::read(journal.0.join("awareness/discovery_clusters.json")).expect("cache");

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["reason_code"], "speaker_discovery_failed");
    assert_eq!(body["retryable"], false);
    assert_eq!(
        fs::read(journal.0.join("awareness/discovery_clusters.json")).expect("cache"),
        cache
    );
    assert!(
        journal
            .0
            .join("awareness/discovery_clusters.resolved.json")
            .exists()
    );
}

#[tokio::test]
async fn scan_all_noise_clears_both_cache_files() {
    install_discovery_helper();
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    add_candidates(&journal, 8);
    journal.cache_markers();

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["clusters"], json!([]));
    assert!(!journal.0.join("awareness/discovery_clusters.json").exists());
    assert!(
        !journal
            .0
            .join("awareness/discovery_clusters.resolved.json")
            .exists()
    );
}

#[tokio::test]
async fn scan_publishes_viable_clusters_and_keeps_null_label_rows_eligible() {
    install_discovery_helper();
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    journal.candidate_segment(
        1,
        json!({"labels":[{"sentence_id":1,"speaker":null}]}),
        &[
            unit(0.0, 3.0),
            threshold_boundary(),
            unit(0.0, 0.0),
            unit(f32::INFINITY, 0.0),
            unit(f32::NAN, 0.0),
            unit(0.5, 0.0),
        ],
    );
    for index in 2..=5 {
        journal.candidate_segment(index, json!({"labels":[]}), &[unit(0.0, 1.0)]);
    }
    journal.cache_markers();

    let (status, body) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "degraded");
    assert_eq!(
        body["issues"][0]["reason_code"],
        "speaker_discovery_invalid_embeddings"
    );
    assert_eq!(body["issues"][0]["count"], 3);
    assert_eq!(body["clusters"][0]["cluster_id"], 0);
    assert_eq!(body["clusters"][0]["size"], 5);

    let path = journal.0.join("awareness/discovery_clusters.json");
    let raw = fs::read_to_string(&path).expect("cache");
    assert!(raw.starts_with("{\n  \"version\":"), "{raw}");
    let cache: Value = serde_json::from_str(&raw).expect("cache json");
    let members = cache["clusters"]["0"].as_array().expect("members");
    assert!(
        members
            .iter()
            .any(|member| { member["segment_key"] == "120000_1" && member["sentence_id"] == 1 })
    );
    assert!(
        !members
            .iter()
            .any(|member| { member["segment_key"] == "120000_1" && member["sentence_id"] != 1 })
    );
    assert!(
        fs::read_dir(journal.0.join("awareness"))
            .expect("awareness")
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
    );
    assert!(
        journal
            .0
            .join("awareness/discovery_clusters.resolved.json")
            .exists(),
        "success must not remove the resolved sentinel"
    );
}

#[tokio::test]
async fn scan_hides_clusters_with_at_least_half_dismissed_provenance() {
    install_discovery_helper();
    let journal = Journal::new();
    journal.entity("owner", true);
    journal.owner_centroid();
    add_candidates(&journal, 5);
    journal.cache(candidate_members(5));

    let (status, dismissed) = call(
        &journal.0,
        "/app/speakers/api/discovery/dismiss",
        json!({"cluster_id":7,"disposition":"quiet"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dismissed}");

    let (status, scan) = call(&journal.0, "/app/speakers/api/discovery/scan", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{scan}");
    assert_eq!(scan["clusters"], json!([]));
    let cache: Value = serde_json::from_slice(
        &fs::read(journal.0.join("awareness/discovery_clusters.json")).expect("cache"),
    )
    .expect("cache json");
    assert!(cache["clusters"]["0"].is_array(), "{cache}");
}
