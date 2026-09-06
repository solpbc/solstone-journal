// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

const DAY: &str = "20260804";
const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRESH_SEGMENT: &str = "180000_60";

fn write_config(root: &std::path::Path, bytes: &[u8]) {
    fs::create_dir_all(root.join("config")).expect("config directory");
    fs::write(root.join("config/journal.json"), bytes).expect("journal config");
}

fn established_journal() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
    write_config(dir.path(), br#"{"setup":{"completed_at":1767225600}}"#);
    fs::create_dir_all(dir.path().join("link")).expect("link directory");
    fs::write(
        dir.path().join("link/authorized_clients.json"),
        json!([{
            "fingerprint": CID_A,
            "device_label": "fixture device",
            "paired_at": "2026-01-01T00:00:00Z",
            "instance_id": "fixture-instance",
            "role": "",
            "kind": "cert",
        }])
        .to_string(),
    )
    .expect("pairing identity");
    dir
}

fn basis() -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(CID_A).expect("fixture cid"),
    }
}

fn multipart(envelope: Value, name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "devices-ingest-mount-boundary";
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

fn fresh_upload() -> (String, Vec<u8>) {
    multipart(
        json!({"day": DAY, "segment": FRESH_SEGMENT, "files": [{"submitted": "fresh.flac"}]}),
        "fresh.flac",
        b"fresh-mount-bytes",
    )
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Vec<u8>,
    content_type: Option<String>,
    protocol: Option<&str>,
) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body))
        .expect("request");
    if let Some(version) = protocol {
        request.headers_mut().insert(
            "X-Solstone-Protocol-Version",
            version.parse().expect("version"),
        );
    }
    if let Some(content_type) = content_type {
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            content_type.parse().expect("content type"),
        );
    }
    request.extensions_mut().insert(basis());
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
        .to_vec();
    (status, headers, body)
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("json response")
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[tokio::test]
async fn omit_protocol_header_is_400_on_all_four_mounted_paths() {
    let journal = established_journal();
    let app = router(journal.path().to_path_buf());
    let (content_type, body) = fresh_upload();
    for (method, path, payload, content_type) in [
        ("POST", "/app/devices/ingest", body, Some(content_type)),
        ("GET", "/app/devices/ingest/manifest", Vec::new(), None),
        (
            "GET",
            "/app/devices/ingest/manifest/20260804",
            Vec::new(),
            None,
        ),
        (
            "GET",
            "/app/devices/ingest/segments/20260804",
            Vec::new(),
            None,
        ),
    ] {
        let (status, _, bytes) = call(&app, method, path, payload, content_type, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {path}");
        assert_eq!(
            json_body(&bytes)["reason_code"],
            "protocol_version_required",
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn protocol_3_gets_return_ingest_listing_json() {
    let journal = established_journal();
    let app = router(journal.path().to_path_buf());

    let (status, _, bytes) = call(
        &app,
        "GET",
        "/app/devices/ingest/manifest",
        Vec::new(),
        None,
        Some("3"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let manifest = json_body(&bytes);
    assert_eq!(manifest["days"], json!({}));

    let (status, _, bytes) = call(
        &app,
        "GET",
        "/app/devices/ingest/manifest/20260804",
        Vec::new(),
        None,
        Some("3"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let day = json_body(&bytes);
    assert_eq!(day["day"], DAY);
    assert_eq!(day["segments"], json!({}));

    let (status, _, bytes) = call(
        &app,
        "GET",
        "/app/devices/ingest/segments/20260804",
        Vec::new(),
        None,
        Some("3"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listing = json_body(&bytes);
    assert_eq!(listing["protocol_version"], 3);
    assert_eq!(listing["items"], json!([]));
    assert_eq!(listing["total"], 0);
}

#[tokio::test]
async fn unestablished_post_ingest_redirects_to_init() {
    let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
    let app = router(journal.path().to_path_buf());
    let (content_type, body) = fresh_upload();
    let (status, headers, _) = call(
        &app,
        "POST",
        "/app/devices/ingest",
        body,
        Some(content_type),
        Some("3"),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(header(&headers, "location"), Some("/init"));
}

#[tokio::test]
async fn corrupt_post_ingest_is_settings_repair_plain_text() {
    let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
    write_config(journal.path(), b"[]");
    let app = router(journal.path().to_path_buf());
    let (content_type, body) = fresh_upload();
    let (status, headers, bytes) = call(
        &app,
        "POST",
        "/app/devices/ingest",
        body,
        Some(content_type),
        Some("3"),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/plain; charset=utf-8")
    );
    let text = String::from_utf8(bytes).expect("plain text");
    assert!(text.contains("your settings were not changed"), "{text}");
}

#[tokio::test]
async fn native_ingest_posts_through_the_composed_shell() {
    let journal = established_journal();
    let callosum = CallosumSocketServer::bind(journal.path().join("health/callosum.sock"))
        .await
        .expect("Callosum server");
    let app = router(journal.path().to_path_buf());
    let (content_type, body) = fresh_upload();
    let (status, _, bytes) = call(
        &app,
        "POST",
        "/app/devices/ingest",
        body,
        Some(content_type),
        Some("3"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join("device")
        .join(FRESH_SEGMENT);
    assert!(
        segment.is_dir(),
        "expected bytes under {}",
        segment.display()
    );
    assert!(
        fs::metadata(segment.join("fresh.flac"))
            .expect("ingested media")
            .len()
            > 0,
        "segment media is empty"
    );
    callosum.stop().await;
}
