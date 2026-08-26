// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Route-level fixtures for Python-era observer evidence.

use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_ingest::api_router;
use tower::ServiceExt;

const DAY: &str = "20260804";
const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("observer-listing-{label}-{nanos}"));
    fs::create_dir_all(&path).expect("test journal root");
    path
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("read fixture directory") {
        let entry = entry.expect("fixture directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("fixture entry type").is_dir() {
            fs::create_dir_all(&target).expect("fixture directory");
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("fixture file");
        }
    }
}

fn fixture(label: &str) -> PathBuf {
    let root = root(label);
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/observer_listing/python_era");
    copy_tree(&source, &root);
    // The repository ignores captured media extensions. Recreate the two held
    // payloads whose exact sizes and hashes are recorded in the golden history.
    create_media(
        &root,
        "laptop",
        "120000_60",
        "present.flac",
        b"present fixture media",
    );
    create_media(
        &root,
        "phone",
        "130000_60",
        "other.flac",
        b"other device fixture media",
    );
    root
}

async fn callosum_server(root: &Path) -> CallosumSocketServer {
    CallosumSocketServer::bind(root.join("health/callosum.sock"))
        .await
        .expect("callosum server")
}

fn files_in_tree(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).expect("read fixture tree") {
        let entry = entry.expect("fixture tree entry");
        let kind = entry.file_type().expect("fixture entry type");
        if kind.is_dir() {
            files.extend(files_in_tree(&entry.path()));
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
    files
}

fn assert_python_era_provenance(root: &Path) -> Result<(), String> {
    let streams = root.join("streams");
    match fs::read_dir(&streams) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_file()
                    && entry.file_name().to_string_lossy().ends_with(".json")
                {
                    let record =
                        fs::read_to_string(entry.path()).map_err(|error| error.to_string())?;
                    if record.contains("\"did\"") {
                        return Err(format!(
                            "stream record {} contains did",
                            entry.path().display()
                        ));
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot read streams directory: {error}")),
    }
    for path in files_in_tree(&root.join("chronicle")) {
        if path.file_name().is_some_and(|name| name == "events.jsonl") {
            let contents = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            for (line, row) in contents.lines().enumerate() {
                if row.trim().is_empty() {
                    continue;
                }
                let row: Value = serde_json::from_str(row).map_err(|error| {
                    format!(
                        "invalid fixture event {}:{}: {error}",
                        path.display(),
                        line + 1
                    )
                })?;
                if row.get("record_type").and_then(Value::as_str) == Some("device_ingest") {
                    return Err(format!(
                        "fixture event {}:{} is device_ingest",
                        path.display(),
                        line + 1
                    ));
                }
            }
        }
    }
    Ok(())
}

fn basis(cid: &str) -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(cid).expect("fixture cid"),
    }
}

async fn request_bytes(
    app: &axum::Router,
    method: &str,
    uri: &str,
    basis: AccessBasis,
    body: Vec<u8>,
    content_type: Option<String>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body))
        .expect("request");
    request
        .headers_mut()
        .insert("X-Solstone-Protocol-Version", "3".parse().expect("version"));
    if let Some(content_type) = content_type {
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            content_type.parse().expect("content type"),
        );
    }
    for (name, value) in extra_headers {
        request.headers_mut().insert(
            name.parse::<header::HeaderName>().expect("header name"),
            value.parse().expect("header value"),
        );
    }
    request.extensions_mut().insert(basis);
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
        .to_vec();
    (status, body)
}

async fn request_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cid: &str,
) -> (StatusCode, Value) {
    let (status, body) = request_bytes(app, method, uri, basis(cid), Vec::new(), None, &[]).await;
    (
        status,
        serde_json::from_slice(&body).expect("json response"),
    )
}

fn multipart(envelope: Value, name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "observer-listing-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n{}\r\n",
            envelope
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(app: &axum::Router, cid: &str, segment: &str, name: &str, bytes: &[u8]) -> Value {
    upload_on_day(app, cid, DAY, segment, name, bytes).await
}

async fn upload_on_day(
    app: &axum::Router,
    cid: &str,
    day: &str,
    segment: &str,
    name: &str,
    bytes: &[u8],
) -> Value {
    let (content_type, body) = multipart(
        json!({"day": day, "segment": segment, "files": [{"submitted": name}]}),
        name,
        bytes,
    );
    let (status, body) = request_bytes(
        app,
        "POST",
        "/app/devices/ingest",
        basis(cid),
        body,
        Some(content_type),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("upload response")
}

fn observer_path(root: &Path, prefix: &str) -> PathBuf {
    root.join("apps/observer/observers")
        .join(format!("{prefix}.json"))
}

fn history_path(root: &Path, prefix: &str) -> PathBuf {
    root.join("apps/observer/observers")
        .join(prefix)
        .join("hist")
        .join(format!("{DAY}.jsonl"))
}

fn add_observer(
    root: &Path,
    prefix: &str,
    key: &str,
    name: &str,
    binding: Value,
    stream: Value,
    revoked: bool,
) {
    fs::create_dir_all(root.join("apps/observer/observers")).expect("observer directory");
    fs::write(
        observer_path(root, prefix),
        json!({
            "key": key,
            "name": name,
            "stream": stream,
            "created_at": 4,
            "revoked": revoked,
            "device_binding": binding,
        })
        .to_string(),
    )
    .expect("observer record");
}

fn append_history(root: &Path, prefix: &str, record: Value) {
    let path = history_path(root, prefix);
    fs::create_dir_all(path.parent().expect("history parent")).expect("history parent");
    let mut contents = fs::read_to_string(&path).unwrap_or_default();
    contents.push_str(&record.to_string());
    contents.push('\n');
    fs::write(path, contents).expect("history record");
}

fn create_media(root: &Path, stream: &str, segment: &str, name: &str, bytes: &[u8]) {
    let directory = root.join("chronicle").join(DAY).join(stream).join(segment);
    fs::create_dir_all(&directory).expect("segment directory");
    fs::write(directory.join(name), bytes).expect("segment media");
}

fn item<'a>(body: &'a Value, key: &str) -> &'a Value {
    body["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["key"] == key)
        .unwrap_or_else(|| panic!("missing segment {key}"))
}

fn file<'a>(body: &'a Value, key: &str, name: &str) -> &'a Value {
    item(body, key)["files"]
        .as_array()
        .expect("files array")
        .iter()
        .find(|file| file["name"] == name)
        .unwrap_or_else(|| panic!("missing file {name} for segment {key}"))
}

fn append_device_ingest_event(
    root: &Path,
    stream: &str,
    segment: &str,
    submitted: &str,
    written: &str,
    bytes: &[u8],
) {
    create_media(root, stream, segment, written, bytes);
    let events = root
        .join("chronicle")
        .join(DAY)
        .join(stream)
        .join(segment)
        .join("events.jsonl");
    let mut contents = fs::read_to_string(&events).expect("native event log");
    contents.push_str(
        &json!({
            "record_type": "device_ingest",
            "record_version": 1,
            "outcome": "accepted",
            "protocol_version": 3,
            "did": CID_A,
            "source": "",
            "stream": stream,
            "day": DAY,
            "segment": segment,
            "files": [{
                "submitted": submitted,
                "written": written,
                "size": bytes.len(),
                "sha256": "projected-event-sha256"
            }],
            "meta": {}
        })
        .to_string(),
    );
    contents.push('\n');
    fs::write(events, contents).expect("projected native event");
}

#[test]
fn provenance_gate_walks_every_event_log() {
    let root = fixture("provenance-recursion");
    let event_log = root.join("chronicle/20260804/legacy/235959_60/events.jsonl");
    fs::create_dir_all(event_log.parent().expect("event directory")).expect("event directory");
    fs::write(&event_log, "{\"record_type\":\"device_ingest\"}\n").expect("event row");
    assert!(assert_python_era_provenance(&root).is_err());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac1_ac2_ac11_ac12_certificate_selects_fixture_material_on_all_routes() {
    let root = fixture("identity");
    assert_python_era_provenance(&root).expect("seeded fixture provenance");
    let app = api_router(&root);
    let headers = [
        ("Authorization", "Bearer bbbbbbbb-observer-handle"),
        ("X-Solstone-Observer", "bbbbbbbb-observer-handle"),
    ];
    let (status, with_headers) = request_bytes(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        basis(CID_A),
        Vec::new(),
        None,
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, without_headers) = request_bytes(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        basis(CID_A),
        Vec::new(),
        None,
        &[],
    )
    .await;
    assert_eq!(
        with_headers, without_headers,
        "headers cannot select an observer"
    );
    let a: Value = serde_json::from_slice(&with_headers).expect("A listing");
    assert!(a["total"].as_u64().expect("total") > 0);
    assert!(
        a["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["key"] == "120000_60")
    );
    assert!(
        a["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["key"] != "130000_60")
    );
    assert_eq!(file(&a, "120000_60", "present.flac")["status"], "present");
    assert_eq!(
        file(&a, "120100_60", "processed.flac")["status"],
        "processed"
    );
    assert_eq!(file(&a, "120200_60", "gone.flac")["status"], "missing");

    let (status, manifest) = request_json(&app, "GET", "/app/devices/ingest/manifest", CID_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(manifest["days"][DAY]["segments"], 3);
    let (status, day) =
        request_json(&app, "GET", "/app/devices/ingest/manifest/20260804", CID_A).await;
    assert_eq!(status, StatusCode::OK);
    assert!(day["segments"].get("120000_60").is_some());

    let (_, b) = request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_B).await;
    assert!(b["total"].as_u64().expect("B total") > 0);
    assert!(
        b["items"]
            .as_array()
            .expect("B items")
            .iter()
            .any(|item| item["key"] == "130000_60")
    );
    assert!(
        b["items"]
            .as_array()
            .expect("B items")
            .iter()
            .all(|item| item["key"] != "120000_60")
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac3_rejecting_observer_shapes_are_omitted() {
    let root = fixture("rejecting-shapes");
    add_observer(
        &root,
        "cccccccc",
        "cccccccc-extra",
        "Wrong",
        json!({"device": CID_B, "kind":"cert"}),
        json!("wrong"),
        false,
    );
    add_observer(
        &root,
        "dddddddd",
        "dddddddd-extra",
        "Revoked",
        json!({"device": CID_A, "kind":"cert"}),
        json!("revoked"),
        true,
    );
    fs::write(
        observer_path(&root, "eeeeeeee"),
        json!({"key":"eeeeeeee-extra","name":"None","stream":"none","created_at":5}).to_string(),
    )
    .expect("no binding record");
    fs::write(observer_path(&root, "ffffffff"), json!({"key":"ffffffff-extra","name":"Null","stream":"null","created_at":6,"device_binding":null}).to_string()).expect("null binding record");
    for (prefix, stream, segment) in [
        ("cccccccc", "wrong", "140000_60"),
        ("dddddddd", "revoked", "140100_60"),
        ("eeeeeeee", "none", "140200_60"),
        ("ffffffff", "null", "140300_60"),
    ] {
        create_media(&root, stream, segment, "wrong.flac", b"held");
        append_history(
            &root,
            prefix,
            json!({"segment":segment,"stream":stream,"files":[{"submitted":"wrong.flac","written":"wrong.flac","size":4,"sha256":"x","disposition":"written"}]}),
        );
    }
    let app = api_router(&root);
    let (_, body) = request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    for key in ["140000_60", "140100_60", "140200_60", "140300_60"] {
        assert!(
            body["items"]
                .as_array()
                .expect("items")
                .iter()
                .all(|item| item["key"] != key)
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac4_zero_one_and_many_observers_have_distinct_outcomes() {
    let root = root("zero-observer");
    let server = callosum_server(&root).await;
    let app = api_router(&root);
    upload(&app, CID_A, "150000_60", "native.flac", b"native").await;
    let (status, zero) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "zero observers serves native evidence"
    );
    assert_eq!(file(&zero, "150000_60", "native.flac")["status"], "present");

    add_observer(
        &root,
        "aaaaaaaa",
        "aaaaaaaa-extra",
        "One",
        json!({"device": CID_A, "kind":"cert"}),
        json!("one"),
        false,
    );
    create_media(&root, "one", "150100_60", "one.flac", b"one");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"150100_60","stream":"one","files":[{"submitted":"one.flac","written":"one.flac","size":3,"sha256":"one","disposition":"written"}]}),
    );
    let (_, one) = request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert!(
        one["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["key"] == "150100_60")
    );

    add_observer(
        &root,
        "cccccccc",
        "cccccccc-extra",
        "Many",
        json!({"device": CID_A, "kind":"cert"}),
        json!("many"),
        false,
    );
    let (status, many) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(many["reason_code"], "ambiguous_device_observer");
    let detail = many["detail"].as_str().expect("detail");
    assert!(detail.contains("observer revoke <prefix>"));
    assert!(detail.contains("aaaaaaaa") && detail.contains("cccccccc"));
    server.stop().await;
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac11_named_refusal_reaches_every_read_route() {
    let root = fixture("all-route-refusal");
    add_observer(
        &root,
        "cccccccc",
        "cccccccc-extra",
        "Ambiguous",
        json!({"device": CID_A, "kind":"cert"}),
        json!("other"),
        false,
    );
    let app = api_router(&root);
    for route in [
        "/app/devices/ingest/manifest",
        "/app/devices/ingest/manifest/20260804",
        "/app/devices/ingest/segments/20260804",
    ] {
        let (status, body) = request_json(&app, "GET", route, CID_A).await;
        assert_eq!(status, StatusCode::CONFLICT, "{route}");
        assert_eq!(body["reason_code"], "ambiguous_device_observer", "{route}");
    }
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac5_history_tears_refuse_and_remain_device_scoped() {
    let root = fixture("history-tear");
    let torn = history_path(&root, "aaaaaaaa");
    let mut contents = fs::read_to_string(&torn).expect("fixture history");
    contents.push_str("{\n");
    fs::write(&torn, contents).expect("torn history");
    let app = api_router(&root);
    let (status, torn) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(torn["reason_code"], "observer_history_torn");
    let (status, intact) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_B).await;
    assert_eq!(status, StatusCode::OK);
    assert!(intact["total"].as_u64().expect("total") > 0);
    assert!(
        intact["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["key"] == "130000_60")
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac1_all_days_manifest_degrades_a_torn_day() {
    let root = fixture("all-days-degrade");
    let server = callosum_server(&root).await;
    let app = api_router(&root);
    upload_on_day(&app, CID_A, "20260805", "120000_60", "later.flac", b"later").await;
    let torn = history_path(&root, "aaaaaaaa");
    let mut contents = fs::read_to_string(&torn).expect("fixture history");
    contents.push_str("{\n");
    fs::write(&torn, contents).expect("torn history");
    let (status, manifest) = request_json(&app, "GET", "/app/devices/ingest/manifest", CID_A).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        manifest["days"]["20260804"]["error"],
        "observer_history_torn"
    );
    assert!(
        manifest["days"]["20260805"]["segments"]
            .as_u64()
            .expect("second day")
            >= 1
    );
    let (status, day) =
        request_json(&app, "GET", "/app/devices/ingest/manifest/20260804", CID_A).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(day["reason_code"], "observer_history_torn");
    server.stop().await;
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn ac5_existing_unreadable_history_refuses_loudly() {
    let root = fixture("history-unreadable");
    let history = history_path(&root, "aaaaaaaa");
    fs::set_permissions(&history, fs::Permissions::from_mode(0o000)).expect("chmod history");
    if std::fs::File::open(&history).is_ok() {
        panic!("requires a non-root runner");
    }
    let app = api_router(&root);
    let (status, body) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["reason_code"], "observer_history_unreadable");
    fs::set_permissions(&history, fs::Permissions::from_mode(0o600)).expect("restore history");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac6_unions_history_and_all_native_events_with_one_schema() {
    let root = fixture("merge");
    let server = callosum_server(&root).await;
    let app = api_router(&root);
    let first = upload(&app, CID_A, "160000_60", "native.flac", b"native").await;
    let landed = first["segment"]
        .as_str()
        .expect("landed segment")
        .to_owned();
    upload(&app, CID_A, "160000_60", "added.json", b"added").await;
    append_history(
        &root,
        "aaaaaaaa",
        json!({"type":"observed","segment":landed}),
    );
    append_device_ingest_event(
        &root,
        "laptop",
        &landed,
        "client-name.flac",
        "stored-name.flac",
        b"projected event",
    );
    let (_, body) = request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(
        file(&body, "120000_60", "present.flac")["status"],
        "present",
        "history half survives native writes"
    );
    let native = item(&body, &landed);
    assert_eq!(
        native["observed"], true,
        "history observed markers apply to event-only segment files"
    );
    let names = native["files"]
        .as_array()
        .expect("native files")
        .iter()
        .map(|file| file["name"].as_str().expect("name"))
        .collect::<HashSet<_>>();
    assert_eq!(
        names,
        HashSet::from(["native.flac", "added.json", "stored-name.flac"]),
        "all event rows contribute"
    );
    assert!(
        !file(&body, "120000_60", "present.flac")
            .as_object()
            .expect("history file")
            .contains_key("submitted_name"),
        "submitted_name is absent when submitted equals written"
    );
    let projected = file(&body, &landed, "stored-name.flac")
        .as_object()
        .expect("projected event file");
    assert_eq!(projected["submitted_name"], "client-name.flac");
    assert!(!projected.contains_key("written") && !projected.contains_key("submitted"));
    for file in body["items"]
        .as_array()
        .expect("items")
        .iter()
        .flat_map(|item| item["files"].as_array().expect("files"))
    {
        let object = file.as_object().expect("file object");
        assert!(
            object.contains_key("name")
                && object.contains_key("size")
                && object.contains_key("sha256")
                && object.contains_key("status")
        );
        assert!(!object.contains_key("written") && !object.contains_key("submitted"));
    }
    assert_eq!(
        file(&body, "120200_60", "gone.flac")["status"],
        "missing",
        "history attestation is missing before the device re-sends it"
    );
    upload(&app, CID_A, "120200_60", "gone.flac", b"replacement").await;
    let (_, converged) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_ne!(
        file(&converged, "120200_60", "gone.flac")["status"],
        "missing"
    );

    let disk = root
        .join("chronicle")
        .join(DAY)
        .join("laptop")
        .join(&landed)
        .join("native.flac");
    fs::write(disk, b"drifted native bytes").expect("mutate event media");
    let (_, drifted) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(
        file(&drifted, &landed, "native.flac")["status"],
        "present",
        "event status is stat-only"
    );
    server.stop().await;
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn native_write_conflicting_with_history_attestation_refuses_ambiguous() {
    let root = fixture("history-sha-conflict");
    let server = callosum_server(&root).await;
    let app = api_router(&root);
    upload(&app, CID_A, "120200_60", "gone.flac", b"conflicting-bytes").await;
    let (status, body) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason_code"], "ambiguous_segment_file_name");
    server.stop().await;
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac6d_and_ac7_history_selection_rules_are_route_visible() {
    let root = fixture("selection");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"type":"observed","segment":"120000_60"}),
    );
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120300_60","stream":"laptop","files":[{"submitted":"quiet.flac","written":"quiet.flac","size":4,"sha256":"quiet","disposition":"written"}]}),
    );
    create_media(&root, "laptop", "120300_60", "quiet.flac", b"okay");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120400_60","stream":"laptop","files":[{"submitted":"pruned.flac","written":"pruned.flac","size":4,"sha256":"p","disposition":"written"}]}),
    );
    append_history(
        &root,
        "aaaaaaaa",
        json!({"type":"pruned","segment":"120400_60"}),
    );
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120500_60","stream":"laptop","files":[{"submitted":"live.flac","written":"live.flac","size":4,"sha256":"l","disposition":"written"}]}),
    );
    append_history(
        &root,
        "aaaaaaaa",
        json!({"type":"pruned","segment":"120500_60"}),
    );
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120500_60","stream":"laptop","files":[{"submitted":"live.flac","written":"live.flac","size":4,"sha256":"l","disposition":"written"}]}),
    );
    create_media(&root, "laptop", "120500_60", "live.flac", b"live");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120600_60","stream":"laptop","segment_original":"120599_60","files":[
            {"submitted":"same.flac","written":"same.flac","size":4,"sha256":"same","disposition":"written"},
            {"submitted":"same.flac","written":"same.flac","size":4,"sha256":"same","disposition":"written"},
            {"submitted":"left.flac","written":"left.flac","size":4,"sha256":"bytes","disposition":"written"},
            {"submitted":"right.flac","written":"right.flac","size":4,"sha256":"bytes","disposition":"written"},
            {"submitted":"audit.flac","written":"audit.flac","size":4,"sha256":"audit","disposition":"received_not_written"},
            {"submitted":"real.flac","written":"real.flac","size":4,"sha256":"real","disposition":"written"},
            {"submitted":"real.flac","written":"real.flac","size":4,"sha256":"audit-sha","disposition":"received_not_written"}
        ]}),
    );
    for name in ["same.flac", "left.flac", "right.flac", "real.flac"] {
        create_media(&root, "laptop", "120600_60", name, b"held");
    }
    let app = api_router(&root);
    let (_, body) = request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(item(&body, "120000_60")["observed"], true);
    assert_eq!(item(&body, "120300_60")["observed"], false);
    assert!(
        body["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["key"] != "120400_60")
    );
    assert!(
        body["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["key"] == "120500_60")
    );
    let files = item(&body, "120600_60")["files"].as_array().expect("files");
    assert_eq!(
        files
            .iter()
            .filter(|file| file["name"] == "same.flac")
            .count(),
        1
    );
    assert!(
        files.iter().any(|file| file["name"] == "left.flac")
            && files.iter().any(|file| file["name"] == "right.flac")
    );
    assert!(files.iter().any(|file| file["name"] == "real.flac"));
    assert!(files.iter().all(|file| file["name"] != "audit.flac"));
    assert_eq!(item(&body, "120600_60")["original_key"], "120599_60");
    let _ = fs::remove_dir_all(root);

    let root = fixture("ambiguous-name");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120700_60","stream":"laptop","files":[
            {"submitted":"same.flac","written":"same.flac","size":4,"sha256":"one","disposition":"written"},
            {"submitted":"same.flac","written":"same.flac","size":4,"sha256":"two","disposition":"written"}
        ]}),
    );
    let app = api_router(&root);
    let (status, body) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason_code"], "ambiguous_segment_file_name");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ac7v_fallback_streams_and_ac8_statuses_refuse_bad_evidence() {
    let root = fixture("fallback-status");
    fs::write(
        observer_path(&root, "aaaaaaaa"),
        json!({"key":"aaaaaaaa-observer-handle","name":"Laptop.local","stream":"locked-stream","created_at":1,"revoked":false,"device_binding":{"device":CID_A,"kind":"cert"}}).to_string(),
    )
    .expect("lock observer to distinct stream");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120800_60","files":[{"submitted":"locked.flac","written":"locked.flac","size":4,"sha256":"locked","disposition":"written"}]}),
    );
    create_media(&root, "locked-stream", "120800_60", "locked.flac", b"held");
    assert!(
        !root
            .join("chronicle")
            .join(DAY)
            .join("laptop/120800_60/locked.flac")
            .exists(),
        "only the locked fallback path holds this file"
    );
    let app = api_router(&root);
    let (_, locked) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(
        file(&locked, "120800_60", "locked.flac")["status"],
        "present",
        "locked observer stream is the fallback"
    );
    fs::write(observer_path(&root, "aaaaaaaa"), json!({"key":"aaaaaaaa-observer-handle","name":"Laptop.local","created_at":1,"revoked":false,"device_binding":{"device":CID_A,"kind":"cert"}}).to_string()).expect("unlock observer");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120900_60","files":[{"submitted":"derived.flac","written":"derived.flac","size":4,"sha256":"derived","disposition":"written"}]}),
    );
    create_media(&root, "laptop", "120900_60", "derived.flac", b"held");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"120950_60","files":[{"submitted":"client.flac","written":"stored.flac","size":4,"sha256":"stored","disposition":"written"}]}),
    );
    create_media(&root, "laptop", "120950_60", "stored.flac", b"held");
    fs::write(
        root.join("chronicle")
            .join(DAY)
            .join("laptop/120000_60/present.jsonl"),
        json!({"_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","handler":"transcribe","input_size":21}}).to_string() + "\n",
    )
    .expect("terminal sidecar beside present media");
    let (_, body) = request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(
        file(&body, "120900_60", "derived.flac")["status"],
        "present",
        "Python stream_name(Laptop.local) derives laptop"
    );
    assert_eq!(
        file(&body, "120950_60", "stored.flac")["name"],
        "stored.flac"
    );
    assert_eq!(
        file(&body, "120950_60", "stored.flac")["submitted_name"],
        "client.flac"
    );
    assert_eq!(
        file(&body, "120100_60", "processed.flac")["status"],
        "processed"
    );
    assert_eq!(
        file(&body, "120000_60", "present.flac")["status"],
        "present",
        "present is checked before terminal proof"
    );
    fs::write(
        root.join("chronicle")
            .join(DAY)
            .join("laptop/120000_60/present.flac"),
        b"drifted bytes",
    )
    .expect("history drift");
    let (_, drifted) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(
        file(&drifted, "120000_60", "present.flac")["status"],
        "present"
    );
    let _ = fs::remove_dir_all(root);

    for (label, row) in [
        (
            "bad-written",
            json!({"segment":"121000_60","stream":"laptop","files":[{"submitted":"bad","written":"../bad.flac","size":4,"sha256":"bad","disposition":"written"}]}),
        ),
        (
            "bad-stream",
            json!({"segment":"121000_60","stream":"../laptop","files":[{"submitted":"bad.flac","written":"bad.flac","size":4,"sha256":"bad","disposition":"written"}]}),
        ),
    ] {
        let root = fixture(label);
        append_history(&root, "aaaaaaaa", row);
        let app = api_router(&root);
        let (status, body) =
            request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "malformed_evidence_row");
        let _ = fs::remove_dir_all(root);
    }

    let root = fixture("non-string-stream");
    fs::write(observer_path(&root, "aaaaaaaa"), json!({"key":"aaaaaaaa-observer-handle","name":"Laptop.local","stream":{},"created_at":1,"revoked":false,"device_binding":{"device":CID_A,"kind":"cert"}}).to_string()).expect("malformed observer");
    let app = api_router(&root);
    let (status, body) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["reason_code"], "malformed_evidence_row");
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn ac9_registry_failures_and_denominator_are_distinct() {
    let root = fixture("registry-unreadable");
    let registry = root.join("apps/observer/observers");
    fs::set_permissions(&registry, fs::Permissions::from_mode(0o000)).expect("chmod registry");
    if fs::read_dir(&registry).is_ok() {
        panic!("requires a non-root runner");
    }
    let app = api_router(&root);
    let (status, unreadable) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(unreadable["reason_code"], "observer_registry_unreadable");
    fs::set_permissions(&registry, fs::Permissions::from_mode(0o700)).expect("restore registry");
    let _ = fs::remove_dir_all(root);

    let root = fixture("record-unreadable");
    fs::write(observer_path(&root, "brokenxx"), "{").expect("broken regular json");
    let app = api_router(&root);
    let (status, skipped) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(skipped["reason_code"], "observer_record_unreadable");
    let _ = fs::remove_dir_all(root);

    let fixture_root = fixture("json-directory");
    fs::create_dir(fixture_root.join("apps/observer/observers/x.json"))
        .expect("json-named directory");
    let app = api_router(&fixture_root);
    let (status, normal) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "directories are outside the registry denominator"
    );
    assert!(normal["total"].as_u64().expect("total") > 0);
    let _ = fs::remove_dir_all(fixture_root);

    let zero_root = self::root("registry-zero");
    let zero_app = api_router(&zero_root);
    let (_, zero) = request_json(
        &zero_app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        CID_A,
    )
    .await;
    let many_root = fixture("registry-many");
    add_observer(
        &many_root,
        "cccccccc",
        "cccccccc-extra",
        "Many",
        json!({"device": CID_A, "kind":"cert"}),
        json!("many"),
        false,
    );
    let many_app = api_router(&many_root);
    let (_, many) = request_json(
        &many_app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        CID_A,
    )
    .await;
    assert_ne!(unreadable["reason_code"], zero["reason_code"]);
    assert_ne!(unreadable["reason_code"], many["reason_code"]);
    let _ = fs::remove_dir_all(zero_root);
    let _ = fs::remove_dir_all(many_root);
}

#[cfg(unix)]
#[tokio::test]
async fn ac10_new_reasons_are_distinct_and_journal_read_remains_reachable() {
    async fn code(root: &Path) -> String {
        let app = api_router(root);
        let (_, body) =
            request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
        body["reason_code"]
            .as_str()
            .expect("refusal code")
            .to_owned()
    }

    let mut codes = HashSet::new();
    let root = fixture("codes-malformed");
    fs::write(observer_path(&root, "aaaaaaaa"), json!({"key":"aaaaaaaa-observer-handle","name":"Laptop.local","stream":[],"created_at":1,"revoked":false,"device_binding":{"device":CID_A,"kind":"cert"}}).to_string()).expect("malformed observer");
    codes.insert(code(&root).await);
    let _ = fs::remove_dir_all(root);

    let root = fixture("codes-ambiguous-observer");
    add_observer(
        &root,
        "cccccccc",
        "cccccccc-extra",
        "Many",
        json!({"device":CID_A,"kind":"cert"}),
        json!("many"),
        false,
    );
    codes.insert(code(&root).await);
    let _ = fs::remove_dir_all(root);

    let root = fixture("codes-torn");
    let path = history_path(&root, "aaaaaaaa");
    fs::write(&path, fs::read_to_string(&path).expect("history") + "{\n").expect("tear history");
    codes.insert(code(&root).await);
    let _ = fs::remove_dir_all(root);

    let root = fixture("codes-unreadable-history");
    let path = history_path(&root, "aaaaaaaa");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod history");
    if std::fs::File::open(&path).is_ok() {
        panic!("requires a non-root runner");
    }
    codes.insert(code(&root).await);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("restore history");
    let _ = fs::remove_dir_all(root);

    let root = fixture("codes-unreadable-registry");
    let path = root.join("apps/observer/observers");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod registry");
    if fs::read_dir(&path).is_ok() {
        panic!("requires a non-root runner");
    }
    codes.insert(code(&root).await);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("restore registry");
    let _ = fs::remove_dir_all(root);

    let root = fixture("codes-skipped-record");
    fs::write(observer_path(&root, "brokenxx"), "{").expect("broken record");
    codes.insert(code(&root).await);
    let _ = fs::remove_dir_all(root);

    let root = fixture("codes-ambiguous-name");
    append_history(
        &root,
        "aaaaaaaa",
        json!({"segment":"122000_60","stream":"laptop","files":[
            {"submitted":"same.flac","written":"same.flac","size":4,"sha256":"one","disposition":"written"},
            {"submitted":"same.flac","written":"same.flac","size":4,"sha256":"two","disposition":"written"}
        ]}),
    );
    codes.insert(code(&root).await);
    let _ = fs::remove_dir_all(root);

    assert_eq!(
        codes,
        HashSet::from([
            "malformed_evidence_row".to_owned(),
            "ambiguous_device_observer".to_owned(),
            "observer_history_torn".to_owned(),
            "observer_history_unreadable".to_owned(),
            "observer_registry_unreadable".to_owned(),
            "observer_record_unreadable".to_owned(),
            "ambiguous_segment_file_name".to_owned(),
        ])
    );

    let root = fixture("journal-read-failure");
    let chronicle = root.join("chronicle");
    fs::set_permissions(&chronicle, fs::Permissions::from_mode(0o000)).expect("chmod chronicle");
    if fs::read_dir(&chronicle).is_ok() {
        panic!("requires a non-root runner");
    }
    let app = api_router(&root);
    let (status, body) =
        request_json(&app, "GET", "/app/devices/ingest/segments/20260804", CID_A).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["reason_code"], "journal_read_failed");
    fs::set_permissions(&chronicle, fs::Permissions::from_mode(0o700)).expect("restore chronicle");
    let _ = fs::remove_dir_all(root);
}
