// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! W2 mutation conformance tests driven exclusively by the captured corpus.

use std::{fs, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use solstone_core_journal_config_write::{LockOptions, hold_lock};
use tower::ServiceExt;

const W3_MUTATION_CASES: [&str; 4] = [
    "PUT storage.journal-logs",
    "PUT storage.per-stream",
    "PUT storage.retention",
    "POST prune-logs.dry-run",
];

const W3_STORAGE_REFUSAL_CASES: [&str; 4] = [
    "POST prune-logs.bad-days",
    "PUT storage.bad-days",
    "PUT storage.bad-mode",
    "PUT storage.logs-bad-days",
]; // W3 owns the storage/prune routes and they are deliberately unregistered in W2.

fn write_config(root: &std::path::Path, config: &Value) {
    let path = root.join("config/journal.json");
    fs::create_dir_all(path.parent().expect("config parent")).expect("config directory");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(config).expect("JSON")),
    )
    .expect("config write");
}

fn root_from(config: &Value) -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("temporary journal");
    write_config(root.path(), config);
    root
}

fn body_request(method: &str, path: &str, body: Option<&Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(path);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(body).expect("JSON")))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    }
}

async fn request(
    router: axum::Router,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let response = router
        .oneshot(body_request(method, path, body))
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

fn w2_mutations<'a>(corpus: &'a Value, collection: &str) -> Vec<(&'a str, &'a Value)> {
    corpus[collection]
        .as_object()
        .expect("mutation collection")
        .iter()
        .filter(|(name, _)| !W3_MUTATION_CASES.contains(&name.as_str()))
        .map(|(name, case)| (name.as_str(), case))
        .collect()
}

fn assert_response(
    case_name: &str,
    case: &Value,
    root: &std::path::Path,
    status: StatusCode,
    body: Value,
) {
    assert_eq!(
        status.as_u16(),
        case["status"].as_u64().expect("status") as u16,
        "{case_name}"
    );
    let (normalized, mut paths) = crate::corpus::normalize(body, "", &root.display().to_string());
    paths.sort();
    paths.dedup();
    let mut expected_paths = case["normalized_paths"]
        .as_array()
        .expect("paths")
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    expected_paths.sort();
    expected_paths.dedup();
    assert_eq!(paths, expected_paths, "{case_name} normalized paths");
    assert_eq!(
        crate::corpus::digest(&normalized),
        case["digest"].as_str().expect("digest"),
        "{case_name} digest: {normalized}"
    );
}

fn assert_config_result(case_name: &str, case: &Value, root: &std::path::Path) {
    let after: Value =
        serde_json::from_slice(&fs::read(root.join("config/journal.json")).expect("config after"))
            .expect("config JSON");
    assert_eq!(after, case["config_after"], "{case_name} config_after");
    let before_keys = case["config_before"]
        .as_object()
        .expect("before object")
        .keys()
        .collect::<std::collections::BTreeSet<_>>();
    let after_keys = after
        .as_object()
        .expect("after object")
        .keys()
        .collect::<std::collections::BTreeSet<_>>();
    let added = after_keys
        .difference(&before_keys)
        .map(|key| Value::String((*key).to_owned()))
        .collect::<Vec<_>>();
    let removed = before_keys
        .difference(&after_keys)
        .map(|key| Value::String((*key).to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(
        added,
        case["config_keys_added"].as_array().expect("added").clone(),
        "{case_name} added keys"
    );
    assert_eq!(
        removed,
        case["config_keys_removed"]
            .as_array()
            .expect("removed")
            .clone(),
        "{case_name} removed keys"
    );
    if case["config_keys_added"]
        .as_array()
        .expect("added")
        .is_empty()
        && case["config_keys_removed"]
            .as_array()
            .expect("removed")
            .is_empty()
    {
        assert_eq!(
            after_keys, before_keys,
            "{case_name} full root key preservation"
        );
    }
    assert_eq!(
        after["some_future_section"], case["config_before"]["some_future_section"],
        "{case_name} future section preservation"
    );
}

#[tokio::test]
async fn ac1_mutations_replay_status_digest_config_and_key_deltas() {
    let corpus = crate::test_support::corpus();
    let cases = w2_mutations(&corpus, "mutations");
    assert_eq!(cases.len(), 16);
    for (name, case) in cases {
        let root = root_from(&case["config_before"]);
        let (status, body) = request(
            crate::test_support::shell_router(root.path()),
            case["method"].as_str().expect("method"),
            case["path"].as_str().expect("path"),
            Some(&case["sent"]),
        )
        .await;
        assert_response(name, case, root.path(), status, body);
        assert_config_result(name, case, root.path());
    }
}

#[tokio::test]
async fn ac5_malformed_mutations_replay_and_keep_malformed_sections_byte_equal() {
    let corpus = crate::test_support::corpus();
    let cases = w2_mutations(&corpus, "mutations_malformed");
    assert_eq!(cases.len(), 16);
    for (name, case) in cases {
        let root = root_from(&case["config_before"]);
        let before = case["config_before"].as_object().expect("before object");
        let malformed_before = ["retention", "observe", "describe", "identity"]
            .iter()
            .filter_map(|key| {
                before
                    .get(*key)
                    .map(|value| ((*key).to_owned(), crate::corpus::python_json(value)))
            })
            .collect::<Vec<_>>();
        let (status, body) = request(
            crate::test_support::shell_router(root.path()),
            case["method"].as_str().expect("method"),
            case["path"].as_str().expect("path"),
            Some(&case["sent"]),
        )
        .await;
        assert_response(name, case, root.path(), status, body);
        assert_config_result(name, case, root.path());
        let after: Value = serde_json::from_slice(
            &fs::read(root.path().join("config/journal.json")).expect("config after"),
        )
        .expect("config JSON");
        for (section, bytes) in malformed_before {
            if case["config_before"][&section] == case["config_after"][&section] {
                assert_eq!(
                    crate::corpus::python_json(&after[&section]),
                    bytes,
                    "{name} malformed {section} bytes"
                );
            }
        }
    }
}

fn refusal_route(name: &str) -> (&'static str, &'static str) {
    match name {
        "POST config.no-body"
        | "POST config.no-section"
        | "POST config.unknown-section"
        | "POST config.empty-journal-name"
        | "POST config.bad-backend"
        | "POST config.non-bool-preserve" => ("POST", "/app/settings/api/config"),
        "POST observe.non-object-tmux"
        | "POST observe.interval-out-of-range"
        | "POST observe.no-body" => ("POST", "/app/settings/api/observe"),
        "PUT vision.max-extractions-low"
        | "PUT vision.redact-not-list"
        | "PUT vision.unknown-category"
        | "PUT vision.bad-importance" => ("PUT", "/app/settings/api/vision"),
        "PUT sync.non-object" | "PUT sync.non-bool" => ("PUT", "/app/settings/api/sync"),
        "POST facet.no-title" | "POST facet.numeric-title" => ("POST", "/app/settings/api/facet"),
        "PUT facet.absent-update" => ("PUT", "/app/settings/api/facet/no-such"),
        "DELETE facet.delete-no-consent" | "DELETE facet.delete-false-consent" => {
            ("DELETE", "/app/settings/api/facet/no-such")
        }
        "POST facet.rename-no-name" => ("POST", "/app/settings/api/facet/no-such/rename"),
        "PUT chat.bad-thinking-surfaces" => ("PUT", "/app/settings/api/chat"),
        "PUT sol_voice.not-object" => ("PUT", "/app/settings/api/sol_voice"),
        other => panic!("unknown W2 refusal route: {other}"),
    }
}

#[tokio::test]
async fn ac3_w2_refusals_replay_across_all_non_corrupt_phases() {
    let corpus = crate::test_support::corpus();
    let phases = ["established", "rich", "populated", "tokened", "malformed"];
    let mut total = 0;
    for phase in phases {
        let cases = corpus["phases"][phase].as_object().expect("phase");
        for (name, case) in cases.iter().filter(|(name, _)| {
            (name.starts_with("POST ") || name.starts_with("PUT ") || name.starts_with("DELETE "))
                && !W3_STORAGE_REFUSAL_CASES.contains(&name.as_str())
        }) {
            let root = crate::test_support::phase_root(phase);
            let before = fs::read(root.path().join("config/journal.json")).expect("config before");
            let (method, path) = refusal_route(name);
            let sent = if matches!(
                name.as_str(),
                "POST config.no-body" | "POST observe.no-body"
            ) {
                None
            } else {
                case.get("sent")
            };
            let (status, body) = request(
                crate::test_support::shell_router(root.path()),
                method,
                path,
                sent,
            )
            .await;
            if matches!(
                name.as_str(),
                "POST config.no-body" | "POST observe.no-body"
            ) {
                // Recorded native deviation: the handler's 400 is the published
                // contract, while Flask body extraction turns this into a 500.
                assert_eq!(status, StatusCode::BAD_REQUEST, "{phase} {name}");
                assert_eq!(
                    body["reason_code"], "missing_request_body",
                    "{phase} {name}"
                );
            } else {
                assert_response(&format!("{phase} {name}"), case, root.path(), status, body);
            }
            assert_eq!(
                fs::read(root.path().join("config/journal.json")).expect("config after"),
                before,
                "{phase} {name} config bytes"
            );
            total += 1;
        }
    }
    assert_eq!(total, 23 * 5);
}

#[tokio::test]
async fn ac4_config_request_shapes() {
    for body in [
        json!({"section":"identity","data":{"preferred":"Countess"}}),
        json!({"section":"identity","key":"preferred","value":"Countess"}),
        json!({"identity":{"preferred":"Countess"}}),
    ] {
        let root = crate::test_support::phase_root("rich");
        let (status, _) = request(
            crate::test_support::shell_router(root.path()),
            "POST",
            "/app/settings/api/config",
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn ac6_facet_delete_requires_consent_for_a_real_facet() {
    let root = crate::test_support::populated_root();
    let path = root.path().join("facets/work-life");
    // Derived from get_json(silent=True) is None; this branch is not recorded.
    for (body, reason) in [
        (None, "missing_request_body"),
        (Some(json!({})), "missing_required_field"),
        (Some(json!({"consent": false})), "invalid_request_value"),
    ] {
        let (_, response) = request(
            crate::test_support::shell_router(root.path()),
            "DELETE",
            "/app/settings/api/facet/work-life",
            body.as_ref(),
        )
        .await;
        assert_eq!(response["reason_code"], reason);
        assert!(path.exists(), "{reason} preserves facet");
    }
    let (status, _) = request(
        crate::test_support::shell_router(root.path()),
        "DELETE",
        "/app/settings/api/facet/work-life",
        Some(&json!({"consent": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!path.exists());
}

#[tokio::test]
async fn ac7_always_on_activity_is_protected() {
    let root = crate::test_support::populated_root();
    let (_, body) = request(
        crate::test_support::shell_router(root.path()),
        "DELETE",
        "/app/settings/api/facet/work-life/activities/meeting",
        None,
    )
    .await;
    assert_eq!(body["reason_code"], "activity_protected");
}

#[tokio::test]
async fn ac8_bodyless_posts_preserve_the_corrupt_session_gate_envelope() {
    for path in ["/app/settings/api/config", "/app/settings/api/observe"] {
        let root = crate::test_support::corrupt_root();
        let (status, body) = request(
            crate::test_support::shell_router(root.path()),
            "POST",
            path,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "corrupt_config");
    }
}

#[tokio::test]
async fn ac9_off_journal_writes_are_directly_observable() {
    let root = crate::test_support::phase_root("rich");
    let (status, _) = request(
        crate::test_support::shell_router(root.path()),
        "PUT",
        "/app/settings/api/chat",
        Some(&json!({"thinking_surfaces":"always"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(root.path().join("config/chat.json")).expect("chat")
        )
        .expect("JSON")["thinking_surfaces"],
        "always"
    );
    let (status, _) = request(
        crate::test_support::shell_router(root.path()),
        "PUT",
        "/app/settings/api/sync",
        Some(&json!({"plaud":{"enabled":true}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(root.path().join("config/schedules.json")).expect("schedules")
        )
        .expect("JSON")["sync:plaud"]["enabled"],
        true
    );
}

#[tokio::test]
async fn ac10_config_busy_routes_and_chat_timeout_twin() {
    let options = LockOptions {
        timeout: Duration::from_millis(1),
        poll_interval: Duration::from_millis(1),
        mode: None,
    };
    for (path, body) in [
        (
            "/app/settings/api/config",
            json!({"section":"identity","data":{"bio":"busy"}}),
        ),
        ("/app/settings/api/validate-keys", json!({})),
        ("/app/settings/api/vision", json!({"max_extractions":10})),
        (
            "/app/settings/api/observe",
            json!({"tmux":{"enabled":true}}),
        ),
    ] {
        let root = crate::test_support::phase_root("rich");
        let _lock = hold_lock(
            root.path().join("config/journal.json"),
            LockOptions::default(),
        )
        .expect("held journal lock");
        let (status, response) = request(
            crate::routes_with_lock_options(root.path().to_owned(), options),
            if path.ends_with("validate-keys") {
                "POST"
            } else {
                "PUT"
            },
            path,
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(response["reason_code"], "config_busy", "{path}");
    }
    let root = crate::test_support::phase_root("rich");
    fs::create_dir_all(root.path().join("config")).expect("config directory");
    fs::write(root.path().join("config/chat.json"), "{}\n").expect("chat config");
    let _lock = hold_lock(root.path().join("config/chat.json"), LockOptions::default())
        .expect("held chat lock");
    let (status, response) = request(
        crate::routes_with_lock_options(root.path().to_owned(), options),
        "PUT",
        "/app/settings/api/chat",
        Some(&json!({"thinking_surfaces":"always"})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response["reason_code"], "settings_operation_failed");
}

#[tokio::test]
async fn ac11_env_write_persists_masks_and_clears_stale_validation() {
    let root = crate::test_support::phase_root("rich");
    let (status, _) = request(
        crate::test_support::shell_router(root.path()),
        "POST",
        "/app/settings/api/config",
        Some(&json!({"section":"env","data":{"PLAUD_ACCESS_TOKEN":"fresh-token"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let config: Value =
        serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).expect("config"))
            .expect("JSON");
    assert_eq!(config["env"]["PLAUD_ACCESS_TOKEN"], "fresh-token");
    assert!(config["service_key_validation"].get("plaud").is_none());
    let day = chrono::Local::now().format("%Y%m%d").to_string();
    let line: Value = serde_json::from_str(
        &fs::read_to_string(
            root.path()
                .join("config/actions")
                .join(format!("{day}.jsonl")),
        )
        .expect("action log"),
    )
    .expect("action JSON");
    assert_eq!(
        line["params"]["changed_fields"]["PLAUD_ACCESS_TOKEN"],
        json!({"old":"***","new":"***"})
    );
}

#[tokio::test]
async fn ac12_explicit_sixteen_pair_inventory() {
    let pairs = [
        ("PUT", "/app/settings/api/config"),
        ("POST", "/app/settings/api/config"),
        ("PUT", "/app/settings/api/sol_voice"),
        ("PUT", "/app/settings/api/chat"),
        ("POST", "/app/settings/api/validate-keys"),
        ("PUT", "/app/settings/api/vision"),
        ("PUT", "/app/settings/api/observe"),
        ("POST", "/app/settings/api/observe"),
        ("POST", "/app/settings/api/facet"),
        ("PUT", "/app/settings/api/facet/work-life"),
        ("DELETE", "/app/settings/api/facet/work-life"),
        ("POST", "/app/settings/api/facet/work-life/rename"),
        ("POST", "/app/settings/api/facet/work-life/activities"),
        (
            "PUT",
            "/app/settings/api/facet/work-life/activities/meeting",
        ),
        (
            "DELETE",
            "/app/settings/api/facet/work-life/activities/meeting",
        ),
        ("PUT", "/app/settings/api/sync"),
    ];
    assert_eq!(pairs.len(), 16);
    let root = crate::test_support::populated_root();
    for (method, path) in pairs {
        let response = crate::test_support::shell_router(root.path())
            .oneshot(body_request(method, path, Some(&json!({}))))
            .await
            .expect("response");
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
    }
}
