// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, header},
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use crate::{
    routes,
    test_support::{corpus, fixed_clock, later_clock, phase_root, write},
};

// Mirrors sol-client json_format::{sort_json, ensure_ascii}; copied for AC3/AC19 because those helpers are private and production code must not take a client dependency. Keep sort_json(v) -> serde_json::to_string(&sorted) -> ensure_ascii(&s).
fn sort_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sort_json).collect()),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), sort_json(&object[key]));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}
fn ensure_ascii(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            let codepoint = ch as u32;
            if codepoint <= 0xFFFF {
                output.push_str(&format!("\\u{codepoint:04x}"));
            } else {
                let adjusted = codepoint - 0x1_0000;
                let high = 0xD800 + (adjusted >> 10);
                let low = 0xDC00 + (adjusted & 0x3FF);
                output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
        }
    }
    output
}
fn canonical(value: &Value) -> Vec<u8> {
    ensure_ascii(&serde_json::to_string(&sort_json(value)).expect("JSON")).into_bytes()
}
fn substitute(value: &str, root: &Path) -> String {
    let root_text = root.display().to_string();
    let canonical = root
        .canonicalize()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| root_text.clone());
    value
        .replace(&canonical, "<JOURNAL_ROOT>")
        .replace(&root_text, "<JOURNAL_ROOT>")
}
fn normalize(value: &mut Value, root: &Path) {
    match value {
        Value::String(text) => *text = substitute(text, root),
        Value::Array(values) => values.iter_mut().for_each(|value| normalize(value, root)),
        Value::Object(values) => values.values_mut().for_each(|value| normalize(value, root)),
        _ => {}
    }
}

fn gated(root: &Path) -> Router {
    solstone_core_convey_shell::session_gate::apply_layer(
        routes(root.to_path_buf(), fixed_clock()),
        root.to_path_buf(),
    )
}

#[tokio::test]
async fn ac2_ac3_replay_all_108_timeline_records() {
    let fixture = corpus();
    let mut executed = 0;
    for (phase, records) in fixture["phases"].as_object().expect("phases") {
        let root = phase_root(phase);
        let router = gated(root.path());
        for expected in records
            .as_array()
            .expect("records")
            .iter()
            .filter(|record| {
                record["path"]
                    .as_str()
                    .is_some_and(|path| path.starts_with("/app/timeline/"))
            })
        {
            executed += 1;
            let response = router
                .clone()
                .oneshot(
                    Request::get(expected["path"].as_str().expect("path"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(
                response.status().as_u16(),
                expected["status"].as_u64().expect("status") as u16,
                "{phase} {}",
                expected["path"]
            );
            // Deliberately exclude Content-Disposition: Flask's static sender records it on
            // 16 cases, while converted native assets set Content-Type only, as do
            // convey-shell and settings-web.
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                expected["content_type"].as_str(),
                "{phase} {}",
                expected["path"]
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok()),
                expected.get("location").and_then(Value::as_str),
                "{phase} {}",
                expected["path"]
            );
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let digest = if expected["body_sha256_basis"] == "raw-body" {
                Sha256::digest(
                    substitute(std::str::from_utf8(&body).expect("UTF-8"), root.path()).as_bytes(),
                )
            } else {
                let mut value: Value = serde_json::from_slice(&body).expect("JSON body");
                normalize(&mut value, root.path());
                Sha256::digest(canonical(&value))
            };
            assert_eq!(
                format!("{digest:x}"),
                expected["body_sha256"].as_str().expect("digest"),
                "{phase} {}",
                expected["path"]
            );
            if expected["body_sha256_basis"] == "normalized-json" {
                assert_eq!(expected["normalized_fields"], serde_json::json!([]));
            } else {
                assert!(expected.get("normalized_fields").is_none());
            }
        }
    }
    assert_eq!(executed, 108);
}

#[tokio::test]
async fn ac4_clock_grows_populated_coverage_by_one_month() {
    let root = phase_root("populated");
    let first = routes(root.path().to_path_buf(), fixed_clock())
        .oneshot(
            Request::get("/app/timeline/api/overview")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let first: Value =
        serde_json::from_slice(&to_bytes(first.into_body(), usize::MAX).await.expect("body"))
            .expect("JSON");
    assert_eq!(first["now"], "2026-05-15T12:00:00");
    let second = routes(root.path().to_path_buf(), later_clock())
        .oneshot(
            Request::get("/app/timeline/api/overview")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let second: Value = serde_json::from_slice(
        &to_bytes(second.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("JSON");
    assert_eq!(
        first["months"].as_array().expect("months").len() + 1,
        second["months"].as_array().expect("months").len()
    );
    assert_eq!(second["months"][2]["ym"], "202606");
}

#[tokio::test]
async fn ac17_unparseable_rollup_is_internal_error() {
    let root = phase_root("established_empty");
    write(&root.path().join("timeline.json"), "{");
    let router = routes(root.path().to_path_buf(), fixed_clock());
    for path in [
        "/app/timeline/api/overview",
        "/app/timeline/api/grid",
        "/app/timeline/api/month/202605",
        "/app/timeline/api/day/20260510",
    ] {
        let response = router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
            r#"{"detail":"","error":"I couldn't complete that request.","reason_code":"internal_error"}"#
        );
    }
}

#[tokio::test]
async fn ac17_semantically_invalid_rollup_month_is_internal_error() {
    let root = phase_root("established_empty");
    write(
        &root.path().join("timeline.json"),
        r#"{"months":{"202699":{}}}"#,
    );
    let response = routes(root.path().to_path_buf(), fixed_clock())
        .oneshot(
            Request::get("/app/timeline/api/overview")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
        r#"{"detail":"","error":"I couldn't complete that request.","reason_code":"internal_error"}"#
    );
}

#[test]
fn ac19_canonicalizer_matches_python_compact_ascii_and_surrogate_pairs() {
    let value = serde_json::json!({"z": "é", "a": "😀"});
    assert_eq!(
        String::from_utf8(canonical(&value)).expect("UTF-8"),
        r#"{"a":"\ud83d\ude00","z":"\u00e9"}"#
    );
}

#[tokio::test]
async fn ac5_ac6_ac11_shell_gate_and_fallback_contracts() {
    let established = phase_root("established_empty");
    let shell = solstone_core_convey_shell::router(established.path().to_path_buf());
    let timeline_404 = shell
        .clone()
        .oneshot(
            Request::get("/app/timeline/notaday")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(timeline_404.status(), axum::http::StatusCode::NOT_FOUND);
    assert_eq!(
        to_bytes(timeline_404.into_body(), usize::MAX)
            .await
            .expect("body")
            .len(),
        0
    );
    // Unknown apps are exempt because known_app == None, unlike the gated timeline path.
    let unknown = solstone_core_convey_shell::router(established.path().to_path_buf())
        .oneshot(
            Request::get("/app/nonexistent/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unknown.status(), axum::http::StatusCode::NOT_FOUND);
    assert!(
        !to_bytes(unknown.into_body(), usize::MAX)
            .await
            .expect("body")
            .is_empty()
    );
    let nested = shell
        .oneshot(
            Request::get("/app/timeline/nosuch/deep/path")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(nested.status(), axum::http::StatusCode::NOT_FOUND);
    assert!(
        to_bytes(nested.into_body(), usize::MAX)
            .await
            .expect("body")
            .starts_with(b"<!doctype html>")
    );
    for path in [
        "/app/timeline/",
        "/app/timeline/workspace",
        "/app/timeline/api/grid",
    ] {
        for phase in ["unestablished", "corrupt", "established_empty"] {
            let root = phase_root(phase);
            let response = gated(root.path())
                .oneshot(Request::get(path).body(Body::empty()).expect("request"))
                .await
                .expect("response");
            let status = response.status();
            let location = response.headers().get(header::LOCATION).cloned();
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            if phase == "unestablished" {
                assert_eq!(status, axum::http::StatusCode::FOUND);
                assert_eq!(body.len(), 197);
                assert_eq!(
                    location.as_ref().and_then(|value| value.to_str().ok()),
                    Some("/init")
                );
            } else if phase == "corrupt" {
                assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
                let is_api = path.contains("/api/");
                assert_eq!(
                    content_type,
                    if is_api {
                        "application/json"
                    } else {
                        "text/plain; charset=utf-8"
                    }
                );
                let actual = substitute(std::str::from_utf8(&body).expect("UTF-8"), root.path());
                let detail = "I couldn't read your settings file at <JOURNAL_ROOT>/config/journal.json. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.";
                let expected = if is_api {
                    format!(
                        r#"{{"error":"I couldn't read your settings.","reason_code":"corrupt_config","detail":"{detail}"}}"#
                    )
                } else {
                    detail.to_owned()
                };
                assert_eq!(actual, expected);
            } else if path == "/app/timeline/" {
                assert_eq!(status, axum::http::StatusCode::FOUND);
                assert_eq!(body.len(), 231);
            } else {
                assert_eq!(status, axum::http::StatusCode::OK);
            }
        }
    }
}
