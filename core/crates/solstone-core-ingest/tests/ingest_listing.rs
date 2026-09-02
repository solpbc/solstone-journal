// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Route-level coverage for native durable device-ingest evidence.

use std::fs;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_ingest::api_router;
use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
use tower::ServiceExt;

const DAY: &str = "20260804";
const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn journal() -> tempfile::TempDir {
    let directory = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
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

#[tokio::test]
async fn native_identity_selects_only_matching_rows_on_all_routes() {
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

    let (status, manifest) = request(
        &app,
        "GET",
        "/app/devices/ingest/manifest?source=phone",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(manifest["days"][DAY]["segments"], 1);

    let (status, day) = request(
        &app,
        "GET",
        "/app/devices/ingest/manifest/20260804?source=phone",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(day["segments"].get("120000_1").is_some());
    assert!(day["segments"].get("120100_1").is_none());

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
async fn unparseable_durable_row_refuses_every_read_route() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(&app, CID_A, DAY, "120000_1", "", "audio.flac", b"audio").await;
    overwrite_with_unparseable_row(journal.path(), DAY, "120000_1");

    let (status, manifest) = request(
        &app,
        "GET",
        "/app/devices/ingest/manifest",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(manifest["days"][DAY]["error"], "journal_read_failed");
    for path in [
        "/app/devices/ingest/manifest/20260804",
        "/app/devices/ingest/segments/20260804",
    ] {
        let (status, refusal) = request(&app, "GET", path, CID_A, Vec::new(), None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        assert_eq!(refusal["reason_code"], "journal_read_failed", "{path}");
    }
}

#[tokio::test]
async fn all_days_manifest_degrades_an_unparseable_durable_day() {
    let journal = journal();
    let _callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = api_router(journal.path());
    upload(
        &app,
        CID_A,
        "20260803",
        "120000_1",
        "",
        "good.flac",
        b"good",
    )
    .await;
    upload(&app, CID_A, DAY, "120000_1", "", "bad.flac", b"bad").await;
    overwrite_with_unparseable_row(journal.path(), DAY, "120000_1");

    let (status, manifest) = request(
        &app,
        "GET",
        "/app/devices/ingest/manifest",
        CID_A,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(manifest["days"]["20260803"]["segments"], 1);
    assert_eq!(manifest["days"][DAY]["error"], "journal_read_failed");
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
