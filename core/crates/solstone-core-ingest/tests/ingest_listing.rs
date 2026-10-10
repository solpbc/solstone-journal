// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Route-level coverage for native durable device-ingest evidence.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_ingest::api_router;
use solstone_core_segment::with_takeover_stream_boundary;
use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
use tower::ServiceExt;

const DAY: &str = "20260804";
const TAKEOVER_DAY: &str = "20261004";
const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CID_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const CID_D: &str = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const CID_E: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn journal() -> tempfile::TempDir {
    let directory = tempfile::TempDir::new().expect("journal root");
    seed_authorized_client(directory.path(), CID_A);
    seed_authorized_client(directory.path(), CID_B);
    directory
}

fn seed_authorized_client(root: &Path, cid: &str) {
    AuthorizationLedger::new(root)
        .add(ClientEntry::new(
            cid,
            "Test device",
            "2026-01-01T00:00:00Z",
            "test-instance",
            ClientRole::Roleless,
        ))
        .unwrap();
}

fn basis(cid: &str) -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(cid).expect("valid test cid"),
        leaf_spki: vec![0x30, 0x00],
    }
}

fn multipart(envelope: Value, name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "ingest-listing-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n{envelope}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cid: &str,
    body: Vec<u8>,
    content_type: Option<String>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body))
        .expect("request");
    request.headers_mut().insert(
        "X-Solstone-Protocol-Version",
        header::HeaderValue::from_static("3"),
    );
    if let Some(content_type) = content_type {
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            content_type.parse().expect("content type"),
        );
    }
    request.extensions_mut().insert(basis(cid));
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&body).expect("JSON response"),
    )
}

async fn upload(
    app: &axum::Router,
    cid: &str,
    day: &str,
    segment: &str,
    source: &str,
    name: &str,
    bytes: &[u8],
) -> Value {
    let (content_type, body) = multipart(
        json!({
            "day": day,
            "segment": segment,
            "source": source,
            "files": [{"submitted": name}],
        }),
        name,
        bytes,
    );
    let (status, response) = request(
        app,
        "POST",
        "/app/devices/ingest",
        cid,
        body,
        Some(content_type),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

fn event_path(root: &Path, day: &str, segment: &str) -> std::path::PathBuf {
    let day_root = root.join("chronicle").join(day);
    let streams = fs::read_dir(&day_root).expect("day streams");
    for stream in streams {
        let stream = stream.expect("stream entry");
        let path = stream.path().join(segment).join("events.jsonl");
        if path.is_file() {
            return path;
        }
    }
    panic!("event path for {day}/{segment}")
}

fn overwrite_with_unparseable_row(root: &Path, day: &str, segment: &str) {
    fs::write(
        event_path(root, day, segment),
        b"{\"record_type\":\"device_ingest\"}\n",
    )
    .expect("replace durable event");
}

fn rewrite_event(root: &Path, day: &str, segment: &str, mutate: impl FnOnce(&mut Value)) {
    let path = event_path(root, day, segment);
    let contents = fs::read_to_string(&path).expect("durable event");
    let mut event: Value = serde_json::from_str(contents.lines().next().expect("event row"))
        .expect("valid device event");
    mutate(&mut event);
    fs::write(path, format!("{event}\n")).expect("rewrite durable event");
}

fn item<'a>(body: &'a Value, segment: &str) -> &'a Value {
    body["items"]
        .as_array()
        .expect("listing items")
        .iter()
        .find(|item| item["key"] == segment)
        .expect("segment item")
}

fn publish_takeover(root: &Path, adopted: &str, retired: &str) {
    with_takeover_stream_boundary(root, adopted, retired, |guard| {
        let plan = guard.plan(adopted, retired);
        guard.publish(&plan).unwrap();
        Ok(())
    })
    .expect("takeover publishes");
}

fn stream_for_cid(root: &Path, day: &str, cid: &str) -> String {
    for stream in fs::read_dir(root.join("chronicle").join(day)).expect("day streams") {
        let stream = stream.expect("stream");
        if !stream.path().is_dir() {
            continue;
        }
        for segment in fs::read_dir(stream.path()).expect("stream segments") {
            let segment = segment.expect("segment");
            let events = segment.path().join("events.jsonl");
            if !events.is_file() {
                continue;
            }
            let contents = fs::read_to_string(events).expect("events");
            if contents.lines().any(|line| {
                serde_json::from_str::<Value>(line).is_ok_and(|event| {
                    event["cid"] == cid
                        && event["source"] == "browser"
                        && event["segment"] == "120000_10"
                })
            }) {
                return stream.file_name().to_string_lossy().into_owned();
            }
        }
    }
    panic!("stream for {cid}")
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn collect(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("directory") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, files);
            } else if path.is_file() {
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(path).expect("file bytes"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    if root.exists() {
        collect(root, root, &mut files);
    }
    files
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn segment_items(listing: &Value) -> &Vec<Value> {
    listing["items"].as_array().expect("listing items")
}

fn stream_marker(root: &Path, day: &str, stream: &str) -> (String, Value) {
    let mut latest: Option<(u64, String, Value)> = None;
    for segment in
        fs::read_dir(root.join("chronicle").join(day).join(stream)).expect("stream segments")
    {
        let segment = segment.expect("segment");
        let path = segment.path().join("stream.json");
        if !path.is_file() {
            continue;
        }
        let marker: Value =
            serde_json::from_slice(&fs::read(path).expect("stream marker")).expect("marker JSON");
        let seq = marker["seq"].as_u64().expect("sequence");
        if latest.as_ref().is_none_or(|(prior, _, _)| seq > *prior) {
            latest = Some((
                seq,
                segment.file_name().to_string_lossy().into_owned(),
                marker,
            ));
        }
    }
    let (_, basename, marker) = latest.expect("stream marker exists");
    (basename, marker)
}

#[tokio::test]
async fn native_identity_selects_only_matching_rows_on_segments_route() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(
        &app,
        CID_A,
        DAY,
        "120000_1",
        "phone",
        "phone.flac",
        b"phone",
    )
    .await;
    upload(
        &app,
        CID_A,
        DAY,
        "120100_1",
        "laptop",
        "laptop.flac",
        b"laptop",
    )
    .await;
    upload(
        &app,
        CID_B,
        DAY,
        "120200_1",
        "phone",
        "other.flac",
        b"other",
    )
    .await;

    let (status, segments) = request(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804?source=phone",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(segments["total"], 1);
    assert_eq!(
        item(&segments, "120000_1")["files"][0]["name"],
        "phone.flac"
    );
}

#[tokio::test]
async fn native_evidence_ignores_legacy_registry_tree() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let legacy = journal.path().join("apps/observer/observers");
    fs::create_dir_all(&legacy).expect("legacy registry directory");
    let legacy_files = [
        (legacy.join("broken.json"), b"not JSON".as_slice()),
        (
            legacy.join("one.json"),
            br#"{"cid":"legacy-one"}"#.as_slice(),
        ),
        (
            legacy.join("two.json"),
            br#"{"cid":"legacy-two"}"#.as_slice(),
        ),
    ];
    for (path, contents) in &legacy_files {
        fs::write(path, contents).expect("legacy registry fixture");
    }
    let before = legacy_files
        .iter()
        .map(|(path, _)| fs::read(path).expect("legacy registry fixture"))
        .collect::<Vec<_>>();

    let app = api_router(journal.path());
    upload(&app, CID_A, DAY, "120000_1", "", "audio.flac", b"audio").await;
    for ((path, _), contents) in legacy_files.iter().zip(before) {
        assert_eq!(fs::read(path).expect("legacy registry fixture"), contents);
    }

    let (status, listing) = request(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["total"], 1);
}

#[tokio::test]
async fn native_ingest_ignores_combined_legacy_observer_artifacts() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let legacy_root = journal.path().join("apps/observer/observers");
    let legacy_files = [
        (
            legacy_root.join("aaaaaaaa.json"),
            json!({
                "key": "aaaaaaaa-test-handle",
                "name": "Desk",
                "stream": "desk",
                "created_at": 4,
                "revoked": false,
                "device_binding": {"device": CID_A, "kind": "cert"},
                "last_segment": "090000_1",
                "last_segment_day": "20260803",
                "last_segment_received_at": 1,
            })
            .to_string()
            .into_bytes(),
        ),
        (
            legacy_root.join("aaaaaaaa/hist/20260803.jsonl"),
            b"{\"type\":\"observed\",\"day\":\"20260803\",\"segment\":\"090000_1\",\"stream\":\"desk\"}\n"
                .to_vec(),
        ),
        (
            journal.path().join("streams/desk.json"),
            json!({
                "name": "desk",
                "kind": "observer",
                "host": null,
                "platform": null,
                "created_at": 4,
                "last_day": "20260803",
                "last_segment": "090000_1",
                "seq": 7,
            })
            .to_string()
            .into_bytes(),
        ),
    ];
    for (path, contents) in &legacy_files {
        fs::create_dir_all(path.parent().expect("legacy parent")).expect("legacy parent");
        fs::write(path, contents).expect("legacy artifact");
    }
    let before = legacy_files
        .iter()
        .map(|(path, _)| fs::read(path).expect("legacy artifact"))
        .collect::<Vec<_>>();

    let app = api_router(journal.path());
    let response = upload(&app, CID_A, DAY, "120000_1", "", "audio.flac", b"audio").await;
    assert_eq!(response["status"], "ok");

    let native_stream: Value = serde_json::from_slice(
        &fs::read(journal.path().join("streams/device.json")).expect("native stream record"),
    )
    .expect("native stream record JSON");
    assert_eq!(native_stream["cid"], CID_A);
    assert_eq!(native_stream["source"], "");
    assert_eq!(native_stream["seq"], 1);

    let event = fs::read_to_string(event_path(journal.path(), DAY, "120000_1"))
        .expect("native durable event");
    let event: Value = serde_json::from_str(event.lines().next().expect("event row"))
        .expect("native durable event JSON");
    assert_eq!(event["record_type"], "device_ingest");
    assert_eq!(event["cid"], CID_A);
    assert_eq!(event["source"], "");
    assert_eq!(event["stream"], "device");

    for ((path, _), contents) in legacy_files.iter().zip(before) {
        assert_eq!(fs::read(path).expect("legacy artifact"), contents);
    }
}

#[tokio::test]
async fn unparseable_durable_row_refuses_segments_read() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(&app, CID_A, DAY, "120000_1", "", "audio.flac", b"audio").await;
    overwrite_with_unparseable_row(journal.path(), DAY, "120000_1");

    let (status, refusal) = request(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(refusal["reason_code"], "journal_read_failed");
}

#[tokio::test]
async fn native_events_merge_with_one_schema() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(&app, CID_A, DAY, "120000_1", "", "present.flac", b"present").await;
    upload(
        &app,
        CID_A,
        DAY,
        "120000_1",
        "",
        "processed.flac",
        b"process",
    )
    .await;
    upload(&app, CID_A, DAY, "120000_1", "", "missing.flac", b"missing").await;
    let segment = event_path(journal.path(), DAY, "120000_1")
        .parent()
        .expect("segment directory")
        .to_path_buf();
    fs::remove_file(segment.join("processed.flac")).expect("remove processed media");
    fs::write(
        segment.join("processed.jsonl"),
        concat!(
            r#"{"_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","handler":"transcribe","input_size":7}}"#,
            "\n"
        ),
    )
    .expect("terminal proof");
    fs::remove_file(segment.join("missing.flac")).expect("remove missing media");

    let (status, listing) = request(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let files = item(&listing, "120000_1")["files"]
        .as_array()
        .expect("files");
    let status_for = |name| {
        files
            .iter()
            .find(|file| file["name"] == name)
            .expect("named file")["status"]
            .clone()
    };
    assert_eq!(status_for("present.flac"), "present");
    assert_eq!(status_for("processed.flac"), "processed");
    assert_eq!(status_for("missing.flac"), "missing");
}

#[tokio::test]
async fn native_statuses_refuse_conflicting_durable_evidence() {
    for (field, value) in [
        ("cid", Value::String(CID_B.to_owned())),
        ("source", Value::String("other".to_owned())),
        ("stream", Value::String("foreign".to_owned())),
        ("day", Value::String("20260805".to_owned())),
    ] {
        let journal = journal();
        let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
            .await
            .expect("Callosum server");
        let app = api_router(journal.path());
        upload(&app, CID_A, DAY, "120000_1", "", "audio.flac", b"audio").await;
        rewrite_event(journal.path(), DAY, "120000_1", |event| {
            event[field] = value;
        });
        let (status, refusal) = request(
            &app,
            "GET",
            "/app/devices/ingest/segments/20260804",
            CID_A,
            Vec::new(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{field}");
        assert_eq!(refusal["reason_code"], "journal_read_failed", "{field}");
    }
}

#[tokio::test]
async fn well_formed_unknown_durable_rows_are_ignored() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(&app, CID_A, DAY, "120000_1", "", "audio.flac", b"audio").await;
    let path = event_path(journal.path(), DAY, "120000_1");
    let mut rows = fs::read_to_string(&path).expect("durable rows");
    rows.push_str("{\"record_type\":\"future_event\",\"version\":1}\n");
    fs::write(path, rows).expect("unknown durable row");

    let (status, listing) = request(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["total"], 1);
}

#[tokio::test]
async fn non_collision_listing_wire_shape_is_unchanged() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(
        &app,
        CID_A,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        b"{\"t\":\"segment_start\",\"ts\":1700000000,\"blocks\":[{\"text\":\"noncollision\"}]}\n",
    )
    .await;

    let (status, listing) = request(
        &app,
        "GET",
        "/app/devices/ingest/segments/20261004?source=browser",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let item = &segment_items(&listing)[0];
    let mut keys = item
        .as_object()
        .expect("item object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, ["files", "key"]);
    assert_eq!(item["key"], "120000_10");
    let mut file_keys = item["files"][0]
        .as_object()
        .expect("file object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    file_keys.sort_unstable();
    assert_eq!(file_keys, ["name", "sha256", "size", "status"]);
}

#[tokio::test]
async fn takeover_collision_listing_preserves_both_histories_on_segments_route() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    let bytes_a =
        b"{\"t\":\"segment_start\",\"ts\":1700000000,\"blocks\":[{\"text\":\"pages from A\"}]}\n";
    let bytes_b = b"{\"t\":\"segment_start\",\"ts\":1700000000,\"blocks\":[{\"text\":\"different pages from B\"}]}\n";
    let upload_a = upload(
        &app,
        CID_A,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_a,
    )
    .await;
    let upload_b = upload(
        &app,
        CID_B,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_b,
    )
    .await;
    let stream_a = stream_for_cid(journal.path(), TAKEOVER_DAY, CID_A);
    let stream_b = stream_for_cid(journal.path(), TAKEOVER_DAY, CID_B);
    assert_ne!(stream_a, stream_b);
    publish_takeover(journal.path(), CID_B, CID_A);
    let before = snapshot_tree(&journal.path().join("chronicle").join(TAKEOVER_DAY));

    seed_authorized_client(journal.path(), CID_C);
    seed_authorized_client(journal.path(), CID_E);
    let mut previous_cid = CID_B;
    for reader_cid in [CID_B, CID_C, CID_E] {
        if reader_cid != previous_cid {
            publish_takeover(journal.path(), reader_cid, previous_cid);
        }
        previous_cid = reader_cid;
        let (status, listing) = request(
            &app,
            "GET",
            "/app/devices/ingest/segments/20261004?source=browser",
            reader_cid,
            Vec::new(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listing["protocol_version"], 3);
        assert_eq!(listing["total"], 2);
        for (stream, bytes, upload_response) in [
            (stream_a.as_str(), bytes_a.as_slice(), &upload_a),
            (stream_b.as_str(), bytes_b.as_slice(), &upload_b),
        ] {
            let item = segment_items(&listing)
                .iter()
                .find(|item| item["stream"] == stream)
                .expect("stream item");
            let key = format!("120000_10~{stream}");
            assert_eq!(item["key"], key);
            assert_eq!(item["segment"], "120000_10");
            let file = &item["files"][0];
            assert_eq!(file["name"], "browser_pages.jsonl");
            assert_eq!(file["size"], bytes.len());
            assert_eq!(file["sha256"], digest(bytes));
            assert_eq!(file["status"], "present");
            assert_eq!(
                upload_response["file_descriptors"][0]["sha256"],
                digest(bytes)
            );
        }
    }

    assert_eq!(
        snapshot_tree(&journal.path().join("chronicle").join(TAKEOVER_DAY)),
        before
    );
}

#[tokio::test]
async fn takeover_continuation_upload_advances_only_the_new_tail() {
    let journal = journal();
    for cid in [CID_C, CID_D, CID_E] {
        seed_authorized_client(journal.path(), cid);
    }
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());

    let bytes_a =
        b"{\"t\":\"segment_start\",\"ts\":1700000000,\"blocks\":[{\"text\":\"A history\"}]}\n";
    let bytes_b_pre = b"{\"t\":\"segment_start\",\"ts\":1700000001,\"blocks\":[{\"text\":\"B independent pre-choice\"}]}\n";
    let bytes_b_tail = b"{\"t\":\"segment_start\",\"ts\":1700000002,\"blocks\":[{\"text\":\"B transferred tail\"}]}\n";
    let bytes_c_pre = b"{\"t\":\"segment_start\",\"ts\":1700000003,\"blocks\":[{\"text\":\"C independent pre-choice\"}]}\n";
    let bytes_c_tail = b"{\"t\":\"segment_start\",\"ts\":1700000004,\"blocks\":[{\"text\":\"C transferred tail\"}]}\n";
    let bytes_d_pre = b"{\"t\":\"segment_start\",\"ts\":1700000005,\"blocks\":[{\"text\":\"D independent pre-choice\"}]}\n";
    let bytes_e = b"{\"t\":\"segment_start\",\"ts\":1700000006,\"blocks\":[{\"text\":\"unrelated device\"}]}\n";
    let bytes_other_source =
        b"{\"t\":\"segment_start\",\"ts\":1700000007,\"blocks\":[{\"text\":\"other source\"}]}\n";
    upload(
        &app,
        CID_A,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_a,
    )
    .await;
    upload(
        &app,
        CID_B,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_b_pre,
    )
    .await;
    upload(
        &app,
        CID_C,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_c_pre,
    )
    .await;
    upload(
        &app,
        CID_D,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_d_pre,
    )
    .await;
    upload(
        &app,
        CID_E,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_e,
    )
    .await;
    upload(
        &app,
        CID_A,
        TAKEOVER_DAY,
        "130000_5",
        "other",
        "other.jsonl",
        bytes_other_source,
    )
    .await;

    let tail_stream = stream_for_cid(journal.path(), TAKEOVER_DAY, CID_A);
    let b_stream = stream_for_cid(journal.path(), TAKEOVER_DAY, CID_B);
    let c_stream = stream_for_cid(journal.path(), TAKEOVER_DAY, CID_C);
    let d_stream = stream_for_cid(journal.path(), TAKEOVER_DAY, CID_D);
    let e_stream = stream_for_cid(journal.path(), TAKEOVER_DAY, CID_E);
    assert_ne!(tail_stream, b_stream);
    assert_ne!(tail_stream, c_stream);
    assert_ne!(tail_stream, d_stream);
    assert_ne!(tail_stream, e_stream);

    publish_takeover(journal.path(), CID_B, CID_A);
    upload(
        &app,
        CID_B,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_b_tail,
    )
    .await;
    publish_takeover(journal.path(), CID_C, CID_B);
    upload(
        &app,
        CID_C,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_c_tail,
    )
    .await;
    publish_takeover(journal.path(), CID_D, CID_C);

    let before = snapshot_tree(&journal.path().join("chronicle").join(TAKEOVER_DAY));
    let (previous_segment, before_marker) =
        stream_marker(journal.path(), TAKEOVER_DAY, &tail_stream);
    let seq_before = before_marker["seq"].as_u64().expect("tail sequence");
    let unrelated_stream_snapshots = [
        b_stream.as_str(),
        c_stream.as_str(),
        d_stream.as_str(),
        e_stream.as_str(),
    ]
    .into_iter()
    .map(|stream| {
        (
            stream.to_owned(),
            before
                .iter()
                .filter(|(path, _)| path.starts_with(&format!("{stream}/")))
                .map(|(path, bytes)| (path.clone(), bytes.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    })
    .collect::<BTreeMap<_, _>>();

    let bytes_d_tail = b"{\"t\":\"segment_start\",\"ts\":1700000008,\"blocks\":[{\"text\":\"D transferred tail\"}]}\n";
    let upload_d = upload(
        &app,
        CID_D,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        bytes_d_tail,
    )
    .await;
    let landed = upload_d["segment"].as_str().expect("landed segment");
    let tail_path = journal
        .path()
        .join("chronicle")
        .join(TAKEOVER_DAY)
        .join(&tail_stream)
        .join(landed);
    assert!(tail_path.join("stream.json").is_file());
    let new_marker: Value =
        serde_json::from_slice(&fs::read(tail_path.join("stream.json")).expect("new tail marker"))
            .expect("new marker JSON");
    assert_eq!(new_marker["seq"], seq_before + 1);
    assert_eq!(new_marker["prev_day"], TAKEOVER_DAY);
    assert_eq!(new_marker["prev_segment"], previous_segment);
    for (stream, expected) in unrelated_stream_snapshots {
        let actual = snapshot_tree(&journal.path().join("chronicle").join(TAKEOVER_DAY))
            .into_iter()
            .filter(|(path, _)| path.starts_with(&format!("{stream}/")))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(actual, expected, "unrelated stream {stream} changed");
    }
    assert!(before.contains_key(&format!("{tail_stream}/{previous_segment}/stream.json")));

    let (status, listing) = request(
        &app,
        "GET",
        "/app/devices/ingest/segments/20261004?source=browser",
        CID_D,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed_streams = segment_items(&listing)
        .iter()
        .filter_map(|item| item["stream"].as_str().or_else(|| item["key"].as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(listed_streams.contains(tail_stream.as_str()));
    assert!(listed_streams.contains(d_stream.as_str()));
    assert!(!listed_streams.contains(e_stream.as_str()));
    let listed_hashes = segment_items(&listing)
        .iter()
        .flat_map(|item| item["files"].as_array().into_iter().flatten())
        .filter_map(|file| file["sha256"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for bytes in [
        bytes_a.as_slice(),
        bytes_b_tail.as_slice(),
        bytes_c_tail.as_slice(),
        bytes_d_pre.as_slice(),
        bytes_d_tail.as_slice(),
    ] {
        assert!(listed_hashes.contains(digest(bytes).as_str()));
    }
    assert!(!listed_hashes.contains(digest(bytes_e).as_str()));
    assert!(!listed_hashes.contains(digest(bytes_other_source).as_str()));
}

#[tokio::test]
async fn retired_cid_ingest_is_rejected_after_takeover() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(
        &app,
        CID_A,
        TAKEOVER_DAY,
        "120000_10",
        "browser",
        "browser_pages.jsonl",
        b"{\"t\":\"segment_start\",\"ts\":1700000000,\"blocks\":[{\"text\":\"A bytes\"}]}\n",
    )
    .await;
    publish_takeover(journal.path(), CID_B, CID_A);

    let (content_type, body) = multipart(
        json!({
            "day": TAKEOVER_DAY,
            "segment": "120000_10",
            "source": "browser",
            "files": [{"submitted": "browser_pages.jsonl"}],
        }),
        "browser_pages.jsonl",
        b"{\"t\":\"segment_start\",\"ts\":1700000001,\"blocks\":[{\"text\":\"retired write\"}]}\n",
    );
    let (status, refusal) = request(
        &app,
        "POST",
        "/app/devices/ingest",
        CID_A,
        body,
        Some(content_type),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refusal["reason_code"], "foreign_stream_binding");
}
