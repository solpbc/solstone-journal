// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::IntoResponse,
};
use chrono::TimeZone;
use serde_json::{Value, json};
use tower::ServiceExt;

fn corpus() -> Value {
    serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/convey_health_corpus.json"
    )))
    .expect("health corpus")
}

#[test]
fn ac3_replays_all_captured_health_cases_through_the_shell() {
    temp_env::with_var("HOSTNAME", Some("corpus-host"), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let corpus = corpus();
                let phases = corpus["phases"].as_object().expect("phases");
                let mut total = 0;
                let mut paths = std::collections::BTreeSet::new();
                for (phase_name, phase) in phases {
                    let root = crate::test_support::phase_root(phase_name);
                    let router = solstone_core_convey_shell::router(root.path().to_path_buf());
                    for case in phase["health"].as_array().expect("health cases") {
                        total += 1;
                        paths.insert(case["path"].as_str().expect("path").to_owned());
                        let response = router
                            .clone()
                            .oneshot(
                                Request::get(case["path"].as_str().expect("path"))
                                    .body(Body::empty())
                                    .expect("request"),
                            )
                            .await
                            .expect("response");
                        let expected = &case["response"];
                        assert_eq!(
                            response.status().as_u16(),
                            expected["status"].as_u64().expect("status") as u16,
                            "{phase_name} {}",
                            case["name"]
                        );
                        for (key, value) in expected["headers"].as_object().expect("headers") {
                            assert_eq!(
                                response.headers().get(key).and_then(|v| v.to_str().ok()),
                                value.as_str(),
                                "{phase_name} {} header {key}",
                                case["name"]
                            );
                        }
                        let bytes = to_bytes(response.into_body(), usize::MAX)
                            .await
                            .expect("body");
                        let mut actual = if expected["body"].is_string() {
                            Value::String(String::from_utf8(bytes.to_vec()).expect("text body"))
                        } else {
                            serde_json::from_slice(&bytes).expect("JSON body")
                        };
                        let mut wanted = expected["body"].clone();
                        if phase_name == "corrupt" {
                            replace_text(
                                &mut wanted,
                                "/var/tmp/solstone-convey-system-corpus/corrupt",
                                &root.path().display().to_string(),
                            );
                        }
                        if case["name"] == "static_health_js" {
                            replace_text(
                                &mut wanted,
                                "/app/observer/api/list",
                                "/app/network/api/observers",
                            );
                            replace_text(
                                &mut wanted,
                                "/app/tokens/api/usage",
                                "/app/stats/api/usage",
                            );
                        }
                        if case["name"] == "workspace" {
                            replace_text(&mut wanted, "/app/tokens/", "/app/stats/#cost");
                        }
                        for pattern in case["normalized"]
                            .as_array()
                            .expect("normalized")
                            .iter()
                            .filter_map(Value::as_str)
                        {
                            replace(&mut actual, "response.body", pattern);
                            replace(&mut wanted, "response.body", pattern);
                        }
                        assert_eq!(
                            actual, wanted,
                            "{phase_name} {} {}",
                            case["name"], case["path"]
                        );
                    }
                }
                assert_eq!(total, 70);
                assert_eq!(paths.len(), 10);
                assert!(
                    paths
                        .iter()
                        .filter(|path| path.contains("/api/")
                            || path.ends_with("/")
                            || path.contains("workspace")
                            || path.contains("static")
                            || path.ends_with("background"))
                        .count()
                        >= 7
                );
            });
    });
}

#[test]
fn ac16_health_static_uses_stats_usage_endpoint() {
    let asset = include_str!("../assets/static/health.js");
    assert!(asset.contains("/app/stats/api/usage?day="));
    assert!(!asset.contains("/app/tokens/api/usage?day="));
}

#[test]
fn ac16_health_workspace_cost_link_targets_stats_cost_section() {
    let asset = include_str!("../assets/workspace.html");
    assert!(asset.contains("href=\"/app/stats/#cost\""));
    assert!(!asset.contains("href=\"/app/tokens/\""));
}

fn replace(value: &mut Value, path: &str, pattern: &str) {
    if matches(path, pattern) {
        *value = Value::String("<NORMALIZED>".to_owned());
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                replace(value, &format!("{path}.{key}"), pattern)
            }
        }
        Value::Array(values) => {
            for value in values {
                replace(value, &format!("{path}.*"), pattern)
            }
        }
        _ => {}
    }
}

fn replace_text(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(text) => *text = text.replace(from, to),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| replace_text(value, from, to)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| replace_text(value, from, to)),
        _ => {}
    }
}
fn matches(path: &str, pattern: &str) -> bool {
    let path = path.split('.').collect::<Vec<_>>();
    let pattern = pattern.split('.').collect::<Vec<_>>();
    path.len() == pattern.len()
        && path
            .iter()
            .zip(pattern)
            .all(|(value, expected)| expected == "*" || value == &expected)
}

#[tokio::test]
async fn ac14_reprocess_response_shapes() {
    for (outcome, status, reason) in [
        (
            solstone_core_reprocess_cli::DayOutcome::Malformed,
            StatusCode::BAD_REQUEST,
            "invalid_day",
        ),
        (
            solstone_core_reprocess_cli::DayOutcome::PastOnly,
            StatusCode::BAD_REQUEST,
            "reprocess_past_only",
        ),
        (
            solstone_core_reprocess_cli::DayOutcome::Unreachable,
            StatusCode::SERVICE_UNAVAILABLE,
            "reprocess_unreachable",
        ),
        (
            solstone_core_reprocess_cli::DayOutcome::Failed("disk failed".to_owned()),
            StatusCode::INTERNAL_SERVER_ERROR,
            "reprocess_failed",
        ),
    ] {
        let response = crate::actions::response("20240101", outcome).into_response();
        assert_eq!(response.status(), status);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["reason_code"], reason);
    }
}

#[tokio::test]
async fn ac6_absent_and_unparseable_stats_share_the_unknown_backlog() {
    async fn backlog(phase: &str) -> Value {
        let root = crate::test_support::phase_root(phase);
        let response = solstone_core_convey_shell::router(root.path().to_path_buf())
            .oneshot(
                Request::get("/app/health/api/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .unwrap()["backlog"]
            .clone()
    }
    let absent = backlog("stats_absent").await;
    assert_eq!(absent, backlog("stats_unparseable").await);
    assert_ne!(absent, backlog("established_populated").await);
}

#[tokio::test]
async fn ac12_retry_import_keeps_all_three_python_forms() {
    for (body, message) in [
        (json!({}), None),
        (
            json!({"import_id":"x"}),
            Some("Import retry will be available in a future update"),
        ),
        (
            json!({"import_id":"x","stage":"decode"}),
            Some("Import retry from stage decode will be available in a future update"),
        ),
    ] {
        let response = crate::actions::retry_import(Some(axum::Json(body))).await;
        if let Some(message) = message {
            assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["message"], message);
        } else {
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }
}

#[tokio::test]
async fn ac9_brain_missing_and_unavailable_stay_distinct() {
    async fn info(root: &std::path::Path) -> Value {
        let response = solstone_core_convey_shell::router(root.to_path_buf())
            .oneshot(
                Request::get("/app/health/api/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }
    let missing = crate::test_support::root();
    assert_eq!(
        info(missing.path()).await["brain"]["reason_code"],
        "brain_record_missing"
    );
    let unavailable = crate::test_support::root();
    let path = solstone_core_brain::brain_state_path(unavailable.path());
    std::fs::create_dir_all(&path).unwrap();
    let unavailable_info = info(unavailable.path()).await;
    assert_eq!(
        unavailable_info["brain"]["reason_code"],
        "brain_record_unavailable"
    );
    assert_eq!(
        unavailable_info["brain"]["action"],
        json!({"label":"check again","refresh":true})
    );
    let response = solstone_core_convey_shell::router(unavailable.path().to_path_buf())
        .oneshot(
            Request::post("/app/health/api/brain/check")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let check: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(check["brain"]["reason_code"], "brain_record_unavailable");
}

#[tokio::test]
async fn ac13_restart_observer_injects_one_restart_request_each_time() {
    let calls = std::cell::RefCell::new(Vec::new());
    for _ in 0..2 {
        let response = crate::actions::restart_observer_with("sense", |envelope| {
            calls.borrow_mut().push(envelope.clone());
            true
        });
        assert_eq!(response.status(), StatusCode::OK);
    }
    let calls = calls.into_inner();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|call| call.tract == "supervisor"
        && call.event == "restart"
        && call.extra["service"] == "sense"));
}

#[test]
fn ac15_brain_refresh_injects_a_request_on_every_call() {
    let root = crate::test_support::root();
    let calls = std::cell::RefCell::new(Vec::new());
    for _ in 0..2 {
        assert!(crate::brain::refresh_with(root.path(), |envelope| {
            calls.borrow_mut().push(envelope.clone());
            true
        }));
    }
    let calls = calls.into_inner();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|call| call.tract == "supervisor"
        && call.event == "request"
        && call.extra["cmd"] == json!(["journal", "brain", "refresh"])));
}

#[tokio::test]
async fn ac17_devices_payload_has_the_health_observer_fields() {
    let root = crate::test_support::root();
    let directory = root.path().join("apps/observer/observers");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("failing0.json"), json!({"key":"failing0-key","name":"failing","created_at":1,"last_seen":1,"enabled":true,"health":{"ingest_rejection":{"reason_code":"ingest_rejected","active_count":1,"first_ts":10,"latest_ts":20,"summary":"bad segment","stream":"screen","version":"2"}}}).to_string()).unwrap();
    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/network/api/observers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    let row = &body["observers"][0];
    for key in [
        "state",
        "label",
        "failing",
        "name",
        "prefix",
        "clock_skew",
        "last_seen",
    ] {
        assert!(row.get(key).is_some(), "missing {key}");
    }
    for key in ["active_count", "first_ts", "version"] {
        assert!(
            row["ingest_rejection"].get(key).is_some(),
            "missing ingest_rejection.{key}"
        );
    }
}

#[tokio::test]
async fn ac18_background_uses_shell_catch_all() {
    let root = crate::test_support::root();
    let response = solstone_core_convey_shell::router(root.path().to_path_buf())
        .oneshot(
            Request::get("/app/health/background")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
#[test]
fn ac8_errors_today_label_checks_both_count_and_scan_success() {
    assert_eq!(crate::errors_today_label(1, true), "error today");
    for count in [0, 2, 5] {
        assert_eq!(crate::errors_today_label(count, true), "errors today");
    }
    // A one-item failed scan must take the explicit non-ok branch, rather than
    // accidentally inheriting the singular result from its count.
    assert_eq!(crate::errors_today_label(1, false), "errors today");
    assert_eq!(crate::errors_today_label(0, false), "errors today");
}

#[tokio::test]
async fn ac10_and_ac11_log_reads_are_whole_and_safely_classified() {
    async fn get(root: &std::path::Path, path: &str) -> (StatusCode, Value) {
        let response = solstone_core_convey_shell::router(root.to_path_buf())
            .oneshot(
                Request::get(format!("/app/health/api/log?path={path}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        (status, body)
    }

    let root = crate::test_support::root();
    let health = root.path().join("20240101/health");
    std::fs::create_dir_all(&health).unwrap();
    let content = "whole-file-log\n".repeat(10_000);
    assert!(content.len() > 100_000);
    std::fs::write(health.join("large.log"), &content).unwrap();
    let (status, body) = get(root.path(), "20240101/health/large.log").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], content);

    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.log");
    std::fs::write(&outside_file, "outside").unwrap();
    std::os::unix::fs::symlink(&outside_file, health.join("escape.log")).unwrap();
    let (status, body) = get(root.path(), "20240101/health/escape.log").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["reason_code"], "invalid_path");

    let (status, body) = get(root.path(), "20240101/health/missing.log").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["reason_code"], "file_not_found");

    std::fs::create_dir(health.join("unreadable.log")).unwrap();
    let (status, body) = get(root.path(), "20240101/health/unreadable.log").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["reason_code"], "file_read_failed");
}

#[tokio::test]
async fn ac14_and_ac15_reprocess_handler_seam_submits_despite_backoff() {
    let root = crate::test_support::root();
    let day = "20260101";
    let now = chrono::Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap();
    let segment = root.path().join(format!("chronicle/{day}/090000_60"));
    std::fs::create_dir_all(&segment).unwrap();
    std::fs::write(segment.join("audio.jsonl"), "raw\n").unwrap();
    let health = root.path().join(format!("chronicle/{day}/health"));
    std::fs::create_dir_all(&health).unwrap();
    std::fs::write(health.join("stream.updated"), "").unwrap();

    let calls = std::cell::RefCell::new(Vec::new());
    let response = crate::actions::reprocess_with(
        root.path(),
        day,
        solstone_core_reprocess_cli::Flavor::FromScratch,
        now,
        chrono_tz::UTC,
        |envelope| {
            calls.borrow_mut().push(envelope.clone());
            true
        },
    );
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, json!({"status":"queued","day":day}));
    let calls = calls.into_inner();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tract, "supervisor");
    assert_eq!(calls[0].event, "request");
    assert_eq!(calls[0].extra["day"], day);

    let fingerprint =
        solstone_core_system::catchup::read_raw_input_fingerprint(root.path(), day).unwrap();
    let retry_at = now.timestamp() as f64 + 3_600.0;
    let daily_key = format!("{day}:daily-catchup");
    let state = json!({
        "version": 1,
        "entries": {
            daily_key: {
                "active": null,
                "next_retry_at": retry_at,
                "fingerprint": fingerprint,
            }
        }
    });
    let state_path = root.path().join("health/catchup-state.json");
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let calls = std::cell::Cell::new(0);
    let response = crate::actions::reprocess_with(
        root.path(),
        day,
        solstone_core_reprocess_cli::Flavor::ProcessNow,
        now,
        chrono_tz::UTC,
        |_| {
            calls.set(calls.get() + 1);
            true
        },
    );
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, json!({"status":"queued","day":day}));
    assert_eq!(calls.get(), 1);
}
