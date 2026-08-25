// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_callosum::CallosumSocketServer;
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

const DAY: &str = "20260804";
const DID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRESH_SEGMENT: &str = "180000_60";

fn python_era_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../solstone-core-ingest/tests/fixtures/observer_listing/python_era")
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

fn files_in_tree(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return files;
    };
    for entry in entries {
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

fn write_config(root: &Path, bytes: &[u8]) {
    fs::create_dir_all(root.join("config")).expect("config directory");
    fs::write(root.join("config/journal.json"), bytes).expect("journal config");
}

fn established_python_era() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
    copy_tree(&python_era_fixture(), dir.path());
    write_config(dir.path(), br#"{"setup":{"completed_at":1767225600}}"#);
    assert_python_era_provenance(dir.path()).expect("seeded fixture provenance");
    dir
}

fn basis() -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        did: LinkedDeviceDid::try_from(DID_A).expect("fixture did"),
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
    let journal = established_python_era();
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
    let journal = established_python_era();
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
    assert!(manifest.get("days").is_some(), "manifest has days");
    assert_eq!(manifest["days"][DAY]["segments"], 3);

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
    assert!(day["segments"].get("120000_60").is_some());

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
    assert!(listing["items"].as_array().is_some());
    assert!(listing["total"].as_u64().is_some());
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
    assert!(text.contains("Your settings were NOT changed"), "{text}");
}

#[tokio::test]
async fn python_era_fixture_posts_through_the_composed_shell_onto_laptop() {
    let journal = established_python_era();
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
        .join("laptop")
        .join(FRESH_SEGMENT);
    assert!(
        segment.is_dir(),
        "expected bytes under {}",
        segment.display()
    );
    assert!(
        files_in_tree(&segment).iter().any(|path| fs::metadata(path)
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)),
        "segment directory is empty"
    );
    for path in files_in_tree(journal.path()) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        assert_ne!(name, "device.json", "{}", path.display());
        assert!(
            !name.starts_with("device_") || !name.ends_with(".json"),
            "{}",
            path.display()
        );
    }
    callosum.stop().await;
}
