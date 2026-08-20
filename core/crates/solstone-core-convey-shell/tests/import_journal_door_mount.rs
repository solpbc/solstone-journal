// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use tempfile::TempDir;
use tower::ServiceExt;

const KEY: &str = "door-key-123456789";
const PREFIX: &str = "door-key";
const BEARER: &str = "Bearer door-key-123456789";
const WRONG_PREFIX: &str = "wrong-key";
const AREAS: [&str; 3] = ["segments", "config", "entities"];
const MISSING_AUTH_HTML: &str = "<!doctype html>\n<html lang=en>\n<title>401 Unauthorized</title>\n<h1>Unauthorized</h1>\n<p>Missing or invalid authentication</p>\n";
const REVOKED_HTML: &str = "<!doctype html>\n<html lang=en>\n<title>403 Forbidden</title>\n<h1>Forbidden</h1>\n<p>API key has been revoked</p>\n";
const PREFIX_MISMATCH_HTML: &str = "<!doctype html>\n<html lang=en>\n<title>403 Forbidden</title>\n<h1>Forbidden</h1>\n<p>Key prefix mismatch</p>\n";

fn sentinel(area: &str) -> Value {
    json!({"area": area, "marker": format!("{area}-manifest-state-sentinel")})
}

fn state_path(root: &Path, area: &str) -> std::path::PathBuf {
    root.join("imports")
        .join(PREFIX)
        .join(area)
        .join("state.json")
}

fn seed(root: &Path, established: bool, revoked: bool) {
    if established {
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1767225600}}"#,
        )
        .expect("established config");
    }
    let sources = root.join("apps/import/journal_sources");
    fs::create_dir_all(&sources).expect("source directory");
    fs::write(
        sources.join("legacy-peer.json"),
        serde_json::to_vec(&json!({
            "name": "legacy-peer",
            "key": KEY,
            "enabled": true,
            "revoked": revoked,
        }))
        .expect("source JSON"),
    )
    .expect("source record");
    for area in AREAS {
        let path = state_path(root, area);
        fs::create_dir_all(path.parent().expect("state parent")).expect("state directory");
        fs::write(
            path,
            serde_json::to_vec(&sentinel(area)).expect("sentinel JSON"),
        )
        .expect("state sentinel");
    }
}

async fn request(
    app: &axum::Router,
    method: &str,
    path: &str,
    authorization: Option<&str>,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(authorization) = authorization {
        builder = builder.header(header::AUTHORIZATION, authorization);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from("{}")).expect("request"))
        .await
        .expect("router response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
        .to_vec();
    (status, content_type, body)
}

#[tokio::test]
async fn import_manifest_bearer_door_reaches_owned_state_in_each_session_phase() {
    for (phase, established) in [("established", true), ("unestablished", false)] {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        seed(root.path(), established, false);
        let app = router(root.path().to_path_buf());
        for area in AREAS {
            let (status, content_type, body) = request(
                &app,
                "GET",
                &format!("/app/import/journal/{PREFIX}/manifest/{area}"),
                Some(BEARER),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{phase} {area}");
            assert_eq!(
                content_type.as_deref(),
                Some("application/json"),
                "{phase} {area}"
            );
            assert_eq!(
                body,
                serde_json::to_vec(&sentinel(area)).expect("sentinel JSON"),
                "{phase} {area}"
            );
        }
    }
}

#[tokio::test]
async fn import_manifest_bearer_door_refusals_do_not_leak_owned_state() {
    for (case, revoked, prefix, authorization, expected_status, expected_html) in [
        (
            "missing identity",
            false,
            PREFIX,
            None,
            StatusCode::UNAUTHORIZED,
            MISSING_AUTH_HTML,
        ),
        (
            "revoked identity",
            true,
            PREFIX,
            Some(BEARER),
            StatusCode::FORBIDDEN,
            REVOKED_HTML,
        ),
        (
            "prefix mismatch",
            false,
            WRONG_PREFIX,
            Some(BEARER),
            StatusCode::FORBIDDEN,
            PREFIX_MISMATCH_HTML,
        ),
    ] {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        seed(root.path(), false, revoked);
        let app = router(root.path().to_path_buf());
        for area in AREAS {
            let (status, content_type, body) = request(
                &app,
                "GET",
                &format!("/app/import/journal/{prefix}/manifest/{area}"),
                authorization,
            )
            .await;
            let body = String::from_utf8(body).expect("authentication HTML");
            assert_eq!(status, expected_status, "{case} {area}");
            assert_eq!(
                content_type.as_deref(),
                Some("text/html; charset=utf-8"),
                "{case} {area}"
            );
            assert_eq!(body, expected_html, "{case} {area}");
            assert!(
                !body.contains(&format!("{area}-manifest-state-sentinel")),
                "{case} {area} leaked owned state: {body}"
            );
        }
    }
}

#[tokio::test]
async fn legacy_ingest_doors_refuse_missing_or_revoked_identity_without_mutating_state() {
    for (case, revoked, authorization, expected_status) in [
        ("missing identity", false, None, StatusCode::UNAUTHORIZED),
        (
            "revoked identity",
            true,
            Some(BEARER),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        seed(root.path(), false, revoked);
        let app = router(root.path().to_path_buf());
        for area in AREAS {
            let state = state_path(root.path(), area);
            let before = fs::read(&state).expect("seeded state");
            let (status, _, _) = request(
                &app,
                "POST",
                &format!("/app/import/journal/{PREFIX}/ingest/{area}"),
                authorization,
            )
            .await;
            assert_eq!(status, expected_status, "{case} {area}");
            assert_eq!(
                fs::read(&state).expect("state after refusal"),
                before,
                "{case} {area}"
            );
        }
    }
}
