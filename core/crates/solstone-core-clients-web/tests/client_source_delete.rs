// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_clients_web::router;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_convey_shell::router as shell_router;
use solstone_core_indexer_store::db::open_index;
use solstone_core_ingest::api_router as ingest_router;
use tempfile::TempDir;
use tower::ServiceExt;

const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const JOURNAL_JID: &str = "journal-jid-fixture";

struct Bed {
    dir: TempDir,
}

impl Bed {
    fn new() -> Self {
        let bed = Self::unestablished();
        fs::create_dir_all(bed.path().join("config")).unwrap();
        fs::write(
            bed.path().join("config/journal.json"),
            format!(r#"{{"setup":{{"completed_at":1}},"jid":"{JOURNAL_JID}"}}"#),
        )
        .unwrap();
        bed
    }

    fn unestablished() -> Self {
        Self {
            dir: TempDir::new_in("/var/tmp").expect("journal"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write_file(&self, rel: &str, bytes: &[u8]) {
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn location_only(&self, day: &str, stream: &str, dir: &str) {
        self.write_file(
            &format!("chronicle/{day}/{stream}/{dir}/location.jsonl"),
            b"{\"fix\":\"loc\"}\n",
        );
        self.write_file(
            &format!("chronicle/{day}/{stream}/{dir}/stream.json"),
            b"{}",
        );
    }

    fn audio_only(&self, day: &str, stream: &str, dir: &str) {
        self.write_file(
            &format!("chronicle/{day}/{stream}/{dir}/audio.flac"),
            b"raw-audio",
        );
        self.write_file(
            &format!("chronicle/{day}/{stream}/{dir}/stream.json"),
            b"{}",
        );
    }

    fn stream_record(&self, name: &str, cid: &str, source: &str) {
        self.write_file(
            &format!("streams/{name}.json"),
            json!({
                "name": name,
                "kind": "unknown",
                "host": null,
                "platform": null,
                "created_at": 1,
                "last_day": null,
                "last_segment": null,
                "seq": 0,
                "cid": cid,
                "source": source,
            })
            .to_string()
            .as_bytes(),
        );
    }

    fn pairing_identity(&self, cid: &str) {
        self.write_file(
            "link/authorized_clients.json",
            json!([{
                "fingerprint": cid,
                "device_label": "fixture device",
                "paired_at": "2026-01-01T00:00:00Z",
                "instance_id": "fixture-instance",
                "role": "",
                "kind": "cert",
            }])
            .to_string()
            .as_bytes(),
        );
    }

    fn listing(&self, rel: &str) -> BTreeSet<String> {
        fs::read_dir(self.path().join(rel))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}

fn assert_receipt_shape(body: &Value) {
    let removed = body["removed"].as_object().expect("removed object");
    for key in [
        "originals",
        "segments",
        "mixed_segments",
        "tombstones",
        "days",
        "index_chunks",
        "stream_identity",
    ] {
        assert!(
            removed.get(key).and_then(Value::as_u64).is_some(),
            "removed.{key} must be an integer"
        );
    }
    assert!(
        !removed.contains_key("in_segment_derived"),
        "removed must not contain in_segment_derived"
    );
    for field in ["not_removed", "not_confirmed"] {
        let items = body[field].as_array().expect(field);
        for item in items {
            let what = item["what"].as_str().unwrap_or("");
            let reason = item["plain_reason"].as_str().unwrap_or("");
            assert!(!what.is_empty(), "{field} item missing what: {item}");
            assert!(
                !reason.is_empty(),
                "{field} item missing plain_reason: {item}"
            );
            for forbidden in ["entry", "reason", "staged"] {
                assert!(
                    item.get(forbidden).is_none(),
                    "{field} item must not contain {forbidden}: {item}"
                );
            }
        }
    }
}

async fn call_app(
    app: axum::Router,
    source: &str,
    basis: Option<AccessBasis>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("DELETE")
        .uri(format!("/app/devices/source/{source}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request.body(Body::empty()).unwrap();
    if let Some(basis) = basis {
        request.extensions_mut().insert(basis);
    }
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"raw": bytes.is_empty()}));
    (status, body)
}

async fn call(journal: &Path, stream: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
    call_as(journal, stream, linked_basis(CID_A), headers).await
}

async fn call_shell(journal: &Path, stream: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
    call_app(
        shell_router(journal.to_path_buf()),
        stream,
        Some(linked_basis(CID_A)),
        headers,
    )
    .await
}

async fn call_as(
    journal: &Path,
    source: &str,
    basis: AccessBasis,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    call_app(router(journal.to_path_buf()), source, Some(basis), headers).await
}

async fn call_without_identity(
    journal: &Path,
    source: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    call_app(router(journal.to_path_buf()), source, None, headers).await
}

fn linked_basis(cid: &str) -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(cid).unwrap(),
    }
}

fn owner_headers() -> Vec<(&'static str, &'static str)> {
    Vec::new()
}

fn index_path(journal: &Path, rel: &str) {
    let conn = open_index(journal).unwrap();
    conn.execute("INSERT INTO files(path, mtime) VALUES (?1, 1)", [rel])
        .unwrap();
    conn.execute(
        "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
         VALUES ('text', ?1, '20260805', '', '', 'location', 0, '')",
        [rel],
    )
    .unwrap();
}

fn indexed_paths(journal: &Path) -> BTreeSet<String> {
    let conn = open_index(journal).unwrap();
    let mut statement = conn
        .prepare("SELECT path FROM files ORDER BY path")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

fn chunk_count(journal: &Path) -> u64 {
    let conn = open_index(journal).unwrap();
    conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| {
        row.get::<_, i64>(0)
    })
    .unwrap() as u64
}

fn tombstone(journal: &Path, day: &str, stream: &str, dir: &str) -> Value {
    serde_json::from_slice(
        &fs::read(
            journal
                .join("chronicle")
                .join(day)
                .join(stream)
                .join(dir)
                .join("tombstone.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

async fn ingest_location(journal: &Path, segment: &str) -> Value {
    let server = CallosumSocketServer::bind(journal.join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let boundary = "fresh-location-ingest";
    let envelope = json!({
        "day": "20260805",
        "segment": segment,
        "source": "location",
        "files": [{"submitted": "location.jsonl"}],
    });
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n{envelope}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"location.jsonl\"\r\n\r\n{{}}\n\r\n\
         --{boundary}--\r\n"
    );
    let mut request = Request::builder()
        .method("POST")
        .uri("/app/devices/ingest")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("X-Solstone-Protocol-Version", "3")
        .body(Body::from(body))
        .expect("ingest request");
    request.extensions_mut().insert(linked_basis(CID_A));
    let response = ingest_router(journal)
        .oneshot(request)
        .await
        .expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    server.stop().await;
    let response: Value = serde_json::from_slice(&body).expect("ingest response");
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

#[tokio::test]
async fn criterion_1_location_only_spanning_days_leaves_audio() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.location_only("20260805", "location", "080000_17");
    bed.location_only("20260806", "location", "090000_17");
    bed.audio_only("20260805", "field.audio", "100000_17");
    let audio = fs::read(
        bed.path()
            .join("chronicle/20260805/field.audio/100000_17/audio.flac"),
    )
    .unwrap();
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["segments"], 3);
    assert_eq!(body["removed"]["tombstones"], 3);
    assert_eq!(body["removed"]["originals"], 3);
    assert_eq!(body["removed"]["mixed_segments"], 0);
    assert_eq!(body["removed"]["days"], 2);
    for dir in [
        "chronicle/20260805/location/070000_17",
        "chronicle/20260805/location/080000_17",
        "chronicle/20260806/location/090000_17",
    ] {
        assert_eq!(
            bed.listing(dir),
            BTreeSet::from(["tombstone.json".to_owned()])
        );
    }
    assert_eq!(
        fs::read(
            bed.path()
                .join("chronicle/20260805/field.audio/100000_17/audio.flac")
        )
        .unwrap(),
        audio
    );
}

#[tokio::test]
async fn criterion_2_two_mixed_fixtures_are_removed_whole() {
    let bed = Bed::new();
    bed.write_file(
        "chronicle/20260805/location/070000_17/location.jsonl",
        b"{}",
    );
    bed.write_file("chronicle/20260805/location/070000_17/audio.flac", b"raw");
    bed.write_file(
        "chronicle/20260805/location/080000_17/location.jsonl",
        b"{}",
    );
    bed.write_file(
        "chronicle/20260805/location/080000_17/talents/sense.json",
        b"{}",
    );
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["segments"], 2);
    assert_eq!(body["removed"]["mixed_segments"], 2);
    assert_eq!(
        bed.listing("chronicle/20260805/location/070000_17"),
        BTreeSet::from(["tombstone.json".to_owned()])
    );
    assert_eq!(
        bed.listing("chronicle/20260805/location/080000_17"),
        BTreeSet::from(["tombstone.json".to_owned()])
    );
}

#[tokio::test]
async fn criterion_3_receipt_shape_and_linked_device_identity() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["segments"], 1);
    assert_eq!(
        bed.listing("chronicle/20260805/location/070000_17"),
        BTreeSet::from(["tombstone.json".to_owned()])
    );
}

#[tokio::test]
async fn linked_device_identity_is_required_even_with_legacy_bearer_headers() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    let (status, body) = call_as(bed.path(), "location", AccessBasis::Localhost, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["reason_code"], "linked_device_required");
    let (status, body) = call_as(
        bed.path(),
        "location",
        AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        },
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["reason_code"], "linked_device_required");
    let (status, body) = call_without_identity(
        bed.path(),
        "location",
        &[("Authorization", "Bearer abcdefghijklmnop-observer-handle")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["reason_code"], "linked_device_required");
    assert!(
        bed.path()
            .join("chronicle/20260805/location/070000_17/location.jsonl")
            .is_file()
    );
}

#[tokio::test]
async fn old_observer_source_route_is_not_registered() {
    let bed = Bed::new();
    let mut request = Request::builder()
        .method("DELETE")
        .uri("/app/observer/source/location")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(linked_basis(CID_A));

    let response = router(bed.path().to_path_buf())
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let mut request = Request::builder()
        .method("DELETE")
        .uri("/app/observer/source/location")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(linked_basis(CID_A));
    let response = shell_router(bed.path().to_path_buf())
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn location_lock_failure_returns_a_failure_envelope_without_mutation() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.write_file("streams", b"not a directory");

    let (status, body) = call(bed.path(), "location", &[]).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "Location deletion temporarily unavailable");
    assert_eq!(body["reason_code"], "location_lock_unavailable");
    assert_eq!(
        body["detail"],
        "the location journal is busy; try again shortly"
    );
    assert!(
        bed.path()
            .join("chronicle/20260805/location/070000_17/location.jsonl")
            .is_file()
    );
}

#[tokio::test]
async fn source_validation_follows_linked_device_identity() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    let (status, body) = call(bed.path(), "audio", &owner_headers()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["reason_code"], "invalid_segment_or_stream");
    assert!(
        bed.path()
            .join("chronicle/20260805/location/070000_17/location.jsonl")
            .is_file()
    );
}

#[tokio::test]
async fn any_linked_client_deletes_the_journal_wide_location_set() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.write_file(
        "chronicle/20260805/location/070000_17/device.json",
        json!({"cid": CID_A}).to_string().as_bytes(),
    );
    let (status, body) = call_as(
        bed.path(),
        "location",
        linked_basis("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"]["segments"], 1);
    assert_eq!(
        tombstone(bed.path(), "20260805", "location", "070000_17")["cid"],
        CID_A
    );
}

#[tokio::test]
async fn criterion_7_staged_leftover_is_a_real_door_incomplete() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    fs::create_dir_all(
        bed.path()
            .join("chronicle/20260805/location/.removing_070000_17"),
    )
    .unwrap();
    let loc = "chronicle/20260805/location/070000_17/location.jsonl";
    index_path(bed.path(), loc);
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["segments"], 0);
    let issues = body["not_removed"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert!(issues.iter().any(|item| {
        item["plain_reason"]
            .as_str()
            .unwrap()
            .contains("previous removal of this segment did not finish")
    }));
    assert!(bed.path().join(loc).is_file());
    assert!(
        !bed.path()
            .join("chronicle/20260805/location/070000_17/tombstone.json")
            .exists()
    );
    assert!(indexed_paths(bed.path()).contains(loc));
}

#[tokio::test]
async fn door_refusal_leaves_native_stream_identity_unchanged() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.stream_record("location", CID_A, "location");
    fs::create_dir_all(
        bed.path()
            .join("chronicle/20260805/location/.removing_070000_17"),
    )
    .unwrap();
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"]["segments"], 0);
    assert_eq!(body["removed"]["stream_identity"], 0);
    assert!(
        bed.path()
            .join("chronicle/20260805/location/070000_17/location.jsonl")
            .is_file()
    );
    assert!(bed.path().join("streams/location.json").is_file());
    assert!(
        body["not_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["plain_reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("previous removal"))),
        "{body}"
    );
}

#[tokio::test]
async fn criterion_8_tombstone_cid_from_device_json_not_journal_jid() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.write_file(
        "chronicle/20260805/location/070000_17/device.json",
        json!({"cid": CID_A, "jid": JOURNAL_JID, "kind": "Observed", "device": {}})
            .to_string()
            .as_bytes(),
    );
    bed.location_only("20260805", "location", "080000_17");
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"]["segments"], 2);
    let with_cid = tombstone(bed.path(), "20260805", "location", "070000_17");
    let unknown = tombstone(bed.path(), "20260805", "location", "080000_17");
    assert_eq!(with_cid["cid"], CID_A);
    assert_eq!(unknown["cid"], "unknown");
    assert_eq!(with_cid["reason"], "owner_segment_delete");
    assert_eq!(unknown["reason"], "owner_segment_delete");
    let left = with_cid.to_string();
    let right = unknown.to_string();
    assert!(!left.contains(JOURNAL_JID));
    assert!(!right.contains(JOURNAL_JID));
    assert!(!left.contains("owner_location_data_delete"));
    assert!(!right.contains("owner_location_data_delete"));
}

#[tokio::test]
async fn criterion_5_legacy_device_json_did_key_tombstones_as_cid() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.write_file(
        "chronicle/20260805/location/070000_17/device.json",
        json!({"did": CID_A}).to_string().as_bytes(),
    );
    bed.location_only("20260805", "location", "080000_17");
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"]["segments"], 2);
    let with_cid = tombstone(bed.path(), "20260805", "location", "070000_17");
    let unknown = tombstone(bed.path(), "20260805", "location", "080000_17");
    assert_eq!(with_cid["cid"], CID_A);
    assert_eq!(unknown["cid"], "unknown");
    assert!(with_cid.get("did").is_none());
}

#[tokio::test]
async fn criterion_9_second_run_is_zero_and_keeps_tombstones() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.write_file("facets/work/entities/20260805.jsonl", b"{}\n");
    let (status, first) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["removed"]["segments"], 1);
    let before = fs::read(
        bed.path()
            .join("chronicle/20260805/location/070000_17/tombstone.json"),
    )
    .unwrap();
    let (status, second) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&second);
    assert_eq!(second["removed"]["segments"], 0);
    assert_eq!(second["removed"]["tombstones"], 0);
    assert_eq!(
        fs::read(
            bed.path()
                .join("chronicle/20260805/location/070000_17/tombstone.json"),
        )
        .unwrap(),
        before
    );
    let confirmed = second["not_confirmed"].as_array().unwrap();
    assert!(
        confirmed
            .iter()
            .any(|item| item["what"] == "work 20260805: people and topics")
    );
}

#[tokio::test]
async fn criterion_10_unlinks_every_location_stream_identity_and_fresh_ingest_rebinds() {
    let bed = Bed::new();
    bed.pairing_identity(CID_A);
    bed.location_only("20260805", "location", "070000_17");
    bed.stream_record("location", CID_A, "location");
    bed.stream_record("location_2", CID_B, "location");
    bed.stream_record("workstation", CID_A, "workstation");
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["stream_identity"], 2);
    assert!(!bed.path().join("streams/location.json").exists());
    assert!(!bed.path().join("streams/location_2.json").exists());
    assert!(bed.path().join("streams/workstation.json").is_file());

    let response = ingest_location(bed.path(), "080000_17").await;
    assert_eq!(response["status"], "ok");
    let fresh: Value = serde_json::from_slice(
        &fs::read(bed.path().join("streams/device_location.json")).expect("fresh stream record"),
    )
    .expect("fresh stream record JSON");
    assert_eq!(fresh["source"], "location");
    assert_eq!(fresh["seq"], 1);
}

#[tokio::test]
async fn criterion_11_not_confirmed_equals_facet_and_occupied_stream_set() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.location_only("20260806", "location", "080000_17");
    bed.write_file("chronicle/20260805/iphone/090000_17/location.jsonl", b"{}");
    bed.write_file("chronicle/20260805/iphone/090000_17/audio.flac", b"raw");
    bed.write_file(
        "apps/observer/observers/legacy/hist/20260805.jsonl",
        b"{\"stream\":\"iphone\"}\n",
    );
    for facet in ["work", "personal"] {
        for day in ["20260805", "20260806"] {
            bed.write_file(&format!("facets/{facet}/entities/{day}.jsonl"), b"{}\n");
            bed.write_file(&format!("facets/{facet}/logs/{day}.jsonl"), b"{}\n");
            bed.write_file(&format!("facets/{facet}/news/{day}.md"), b"news\n");
        }
    }
    let mut expected = BTreeSet::new();
    for facet in ["work", "personal"] {
        for day in ["20260805", "20260806"] {
            expected.insert(format!("{facet} {day}: people and topics"));
            expected.insert(format!("{facet} {day}: activity summary"));
            expected.insert(format!("{facet} {day}: news"));
        }
    }
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["segments"], 3, "{body}");
    assert_eq!(body["removed"]["mixed_segments"], 1, "{body}");
    let got: BTreeSet<String> = body["not_confirmed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["what"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(got, expected);

    let (status, second) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    let got: BTreeSet<String> = second["not_confirmed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["what"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(got, expected);
}

#[tokio::test]
async fn criterion_11_all_failed_still_names_residue() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    fs::create_dir_all(
        bed.path()
            .join("chronicle/20260805/location/.removing_070000_17"),
    )
    .unwrap();
    bed.write_file("facets/work/entities/20260805.jsonl", b"{}\n");
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"]["segments"], 0);
    let got: BTreeSet<String> = body["not_confirmed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["what"].as_str().unwrap().to_owned())
        .collect();
    assert!(got.contains("work 20260805: people and topics"));
}

#[tokio::test]
async fn criterion_12_index_prunes_door_paths_only() {
    let bed = Bed::new();
    bed.write_file("chronicle/20260805/iphone/070000_17/location.jsonl", b"{}");
    bed.write_file("chronicle/20260805/iphone/070000_17/audio.flac", b"raw");
    bed.audio_only("20260805", "field.audio", "080000_17");
    let loc = "chronicle/20260805/iphone/070000_17/location.jsonl";
    let audio = "chronicle/20260805/iphone/070000_17/audio.flac";
    let sibling = "chronicle/20260805/field.audio/080000_17/audio.flac";
    index_path(bed.path(), loc);
    index_path(bed.path(), audio);
    index_path(bed.path(), sibling);
    let before_chunks = chunk_count(bed.path());
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&body);
    let remaining = indexed_paths(bed.path());
    assert!(!remaining.contains(loc));
    assert!(!remaining.contains(audio));
    assert!(remaining.contains(sibling));
    let deleted = before_chunks.saturating_sub(chunk_count(bed.path()));
    assert_eq!(body["removed"]["index_chunks"], deleted);
    assert_eq!(deleted, 2);
}

#[tokio::test]
async fn criterion_13_item_json_is_not_mixed_talents_is() {
    let bed = Bed::new();
    bed.write_file(
        "chronicle/20260805/location/070000_17/location.jsonl",
        b"{}",
    );
    bed.write_file("chronicle/20260805/location/070000_17/stream.json", b"{}");
    bed.write_file("chronicle/20260805/location/070000_17/item.json", b"{}");
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"]["mixed_segments"], 0);
    assert_eq!(body["removed"]["segments"], 1);

    let bed = Bed::new();
    bed.write_file(
        "chronicle/20260805/location/070000_17/location.jsonl",
        b"{}",
    );
    bed.write_file(
        "chronicle/20260805/location/070000_17/talents/sense.json",
        b"{}",
    );
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"]["mixed_segments"], 1);
    assert_eq!(body["removed"]["segments"], 1);
}

#[tokio::test]
async fn criterion_14_post_chronicle_failures_do_not_claim_the_work() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    fs::create_dir_all(bed.path().join("indexer/journal.sqlite")).unwrap();
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["segments"], 1);
    assert_eq!(body["removed"]["index_chunks"], 0);
    assert!(
        body["not_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["what"] == "search index")
    );

    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    bed.stream_record("location", CID_A, "location");
    bed.write_file("streams/broken.json", b"{not valid JSON");
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"]["segments"], 1);
    assert_eq!(body["removed"]["stream_identity"], 0);
    assert!(bed.path().join("streams/location.json").is_file());
    assert!(body["not_removed"].as_array().unwrap().iter().any(|item| {
        item["plain_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("inventory could not be read completely"))
    }));
}

#[tokio::test]
async fn default_stream_uses_directory_name() {
    let bed = Bed::new();
    bed.write_file(
        "chronicle/20260805/093000_300_summary/location.jsonl",
        b"{}",
    );
    let (status, body) = call(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["removed"]["segments"], 1);
    assert_eq!(
        bed.listing("chronicle/20260805/093000_300_summary"),
        BTreeSet::from(["tombstone.json".to_owned()])
    );
}

#[tokio::test]
async fn merged_shell_router_tombstones_at_the_devices_route() {
    let bed = Bed::new();
    bed.location_only("20260805", "location", "070000_17");
    let (status, body) = call_shell(bed.path(), "location", &owner_headers()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_receipt_shape(&body);
    assert_eq!(body["removed"]["segments"], 1);
    assert_eq!(
        bed.listing("chronicle/20260805/location/070000_17"),
        BTreeSet::from(["tombstone.json".to_owned()])
    );
}

#[tokio::test]
async fn merged_shell_router_keeps_devices_routes_session_gated() {
    let bed = Bed::unestablished();
    bed.location_only("20260805", "location", "070000_17");
    let (status, _body) = call_shell(bed.path(), "location", &[]).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_ne!(status, StatusCode::NOT_FOUND);
    assert!(
        bed.path()
            .join("chronicle/20260805/location/070000_17/location.jsonl")
            .is_file()
    );
}
