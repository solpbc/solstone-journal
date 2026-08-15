// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mutation conformance tests driven exclusively by the captured corpus.

use std::{fs, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use solstone_core_journal_config_write::{LockOptions, hold_lock};
use tower::ServiceExt;

const MUTATION_PAIRS: [(&str, &str); 19] = [
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
    ("PUT", "/app/settings/api/storage"),
    ("POST", "/app/settings/api/storage/purge"),
    ("POST", "/app/settings/api/storage/prune-logs"),
];

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

fn raw_top_level_field(source: &[u8], key: &str) -> Vec<u8> {
    let marker = format!("  \"{key}\": ");
    let start = source
        .windows(marker.len())
        .position(|window| window == marker.as_bytes())
        .expect("top-level field")
        + marker.len();
    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in source[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'\"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'\"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            b',' | b'\n' if depth == 0 => return source[start..start + offset].to_vec(),
            _ => {}
        }
    }
    source[start..].to_vec()
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

fn mutation_cases<'a>(corpus: &'a Value, collection: &str) -> Vec<(&'a str, &'a Value)> {
    corpus[collection]
        .as_object()
        .expect("mutation collection")
        .iter()
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
    let path = root.join("config/journal.json");
    let after_bytes = fs::read(&path).expect("config after");
    let after: Value = serde_json::from_slice(&after_bytes).expect("config JSON");
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
    let before_bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(&case["config_before"]).expect("before JSON")
    );
    assert_eq!(
        raw_top_level_field(&after_bytes, "some_future_section"),
        raw_top_level_field(before_bytes.as_bytes(), "some_future_section"),
        "{case_name} future section byte preservation"
    );
}

// This replay contains `POST prune-logs.dry-run`, whose recorded 500 is the
// contract on a host *without* the retention executor. `retention_executor.rs`
// resolves `solstone-retention` off ambient PATH, and `make install` puts one at
// `.venv/bin/solstone-retention` — so without the pin below the outcome is the
// host's, not the corpus's: the suite passes 48/48 on a bare box and fails
// 46/48 on any developer machine with the venv active. The pin, not the host,
// establishes absence. Nothing else in this crate launches a process — AC 15
// holds `retention_executor.rs` as the sole exemption and forbids every launch
// API elsewhere — so narrowing PATH cannot perturb any other replayed route.
#[test]
fn ac1_mutations_replay_status_digest_config_and_key_deltas() {
    let _serialized = crate::retention_tests::executor_env_guard();
    let corpus = crate::test_support::corpus();
    let cases = mutation_cases(&corpus, "mutations");
    assert_eq!(cases.len(), 21);
    crate::retention_tests::without_executor(|| {
        for (name, case) in cases {
            let root = root_from(&case["config_before"]);
            let (status, body) = crate::retention_tests::run_async(request(
                crate::test_support::shell_router(root.path()),
                case["method"].as_str().expect("method"),
                case["path"].as_str().expect("path"),
                Some(&case["sent"]),
            ));
            assert_response(name, case, root.path(), status, body);
            assert_config_result(name, case, root.path());
        }
    });
}

#[tokio::test]
async fn ac2_partial_processing_write_preserves_unknown_nested_bytes() {
    let root = crate::test_support::phase_root("rich");
    let config_path = root.path().join("config/journal.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("config")).expect("config JSON");
    config["processing"] = json!({
        "mode": "batch",
        "gate": {"min_text_chars": 25, "max_line_density": 0.3},
        "some_future_key": {"preserve": ["these", "bytes"]},
    });
    write_config(root.path(), &config);
    let before = fs::read(&config_path).expect("before");
    let (status, _) = request(
        crate::test_support::shell_router(root.path()),
        "POST",
        "/app/settings/api/config",
        Some(&json!({"section":"processing","data":{"mode":"realtime"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after = fs::read(&config_path).expect("after");
    let after_config: Value = serde_json::from_slice(&after).expect("after JSON");
    assert_eq!(
        after_config["processing"]["some_future_key"],
        config["processing"]["some_future_key"]
    );
    assert_eq!(
        raw_top_level_field(&after, "some_future_section"),
        raw_top_level_field(&before, "some_future_section")
    );
}

fn fixture_tree_file<'a>(corpus: &'a Value, path: &str) -> &'a str {
    corpus["phases"]["populated"]["_journal_tree"]["files"][path]
        .as_str()
        .expect("fixture tree file")
}

fn normalized_log_bytes(text: &str) -> String {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut row: Value = serde_json::from_str(line).expect("action log JSON");
            row["timestamp"] = json!("<VOLATILE:log_timestamp>");
            String::from_utf8(crate::corpus::python_json(&row)).expect("canonical action log")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if text.ends_with('\n') { "\n" } else { "" }
}

#[tokio::test]
async fn ac13_populated_journal_tree_mutation_bytes_and_runtime_day_logs() {
    let corpus = crate::test_support::corpus();
    let root = crate::test_support::phase_root("rich");
    let router = crate::test_support::shell_router(root.path());
    let calls = [
        (
            "POST",
            "/app/settings/api/config",
            json!({"section":"identity","data":{"preferred":"Countess"}}),
        ),
        (
            "POST",
            "/app/settings/api/facet",
            json!({"title":"Muted Thing","description":"Muted Thing desc","color":"#334455","emoji":"🔇","consent":true}),
        ),
        (
            "PUT",
            "/app/settings/api/facet/muted-thing",
            json!({"muted":true}),
        ),
        (
            "POST",
            "/app/settings/api/facet",
            json!({"title":"Work Life","description":"Work Life desc","color":"#334455","emoji":"💼","icon":"briefcase","consent":true}),
        ),
        (
            "POST",
            "/app/settings/api/facet/work-life/activities",
            json!({"name":"Deep Work","description":"focus block","priority":"high","icon":"target"}),
        ),
        (
            "POST",
            "/app/settings/api/facet/work-life/activities",
            json!({"name":"Standup","description":"","priority":"low","emoji":"🗣"}),
        ),
        (
            "POST",
            "/app/settings/api/facet",
            json!({"title":"Zeta Project","description":"Zeta Project desc","color":"#334455","emoji":"🧪","icon":"flask-conical","consent":true}),
        ),
    ];
    for (method, path, body) in calls {
        let (status, _) = request(router.clone(), method, path, Some(&body)).await;
        assert!(status.is_success(), "{method} {path}: {status}");
    }
    for path in [
        "facets/muted-thing/facet.json",
        "facets/work-life/facet.json",
        "facets/work-life/activities/activities.jsonl",
        "facets/zeta-project/facet.json",
    ] {
        assert_eq!(
            fs::read_to_string(root.path().join(path)).expect("produced tree file"),
            fixture_tree_file(&corpus, path),
            "{path} fixture bytes"
        );
    }
    let day = chrono::Local::now().format("%Y%m%d").to_string();
    for path in [
        "config/actions/20260813.jsonl",
        "facets/muted-thing/logs/20260813.jsonl",
        "facets/work-life/logs/20260813.jsonl",
        "facets/zeta-project/logs/20260813.jsonl",
    ] {
        let runtime_path = path.replace("20260813", &day);
        assert_eq!(
            normalized_log_bytes(
                &fs::read_to_string(root.path().join(runtime_path)).expect("runtime log")
            ),
            normalized_log_bytes(fixture_tree_file(&corpus, path)),
            "{path} log records"
        );
    }
    let lifecycle = crate::test_support::phase_root("rich");
    let lifecycle_router = crate::test_support::shell_router(lifecycle.path());
    for (method, path, body) in [
        (
            "POST",
            "/app/settings/api/facet",
            json!({"title":"Temporary","consent":true}),
        ),
        (
            "POST",
            "/app/settings/api/facet/temporary/activities",
            json!({"name":"Temporary Activity"}),
        ),
        (
            "PUT",
            "/app/settings/api/facet/temporary/activities/temporary_activity",
            json!({"description":"updated"}),
        ),
        (
            "DELETE",
            "/app/settings/api/facet/temporary/activities/temporary_activity",
            json!({}),
        ),
        (
            "POST",
            "/app/settings/api/facet/temporary/rename",
            json!({"new_name":"renamed"}),
        ),
        (
            "DELETE",
            "/app/settings/api/facet/renamed",
            json!({"consent":true}),
        ),
    ] {
        let (status, _) = request(lifecycle_router.clone(), method, path, Some(&body)).await;
        assert!(status.is_success(), "{method} {path}: {status}");
    }
    assert!(!lifecycle.path().join("facets/renamed").exists());
}

// Same executor-absence pin as `ac1`, for the same reason: this collection
// replays `POST prune-logs.dry-run` too.
#[test]
fn ac5_malformed_mutations_replay_and_keep_malformed_sections_byte_equal() {
    let _serialized = crate::retention_tests::executor_env_guard();
    let corpus = crate::test_support::corpus();
    let cases = mutation_cases(&corpus, "mutations_malformed");
    assert_eq!(cases.len(), 21);
    crate::retention_tests::without_executor(|| {
        for (name, case) in cases {
            let root = root_from(&case["config_before"]);
            let before_bytes =
                fs::read(root.path().join("config/journal.json")).expect("before bytes");
            let before = case["config_before"].as_object().expect("before object");
            let malformed_before = ["retention", "observe", "describe", "identity"]
                .iter()
                .filter_map(|key| {
                    before
                        .get(*key)
                        .map(|_| ((*key).to_owned(), raw_top_level_field(&before_bytes, key)))
                })
                .collect::<Vec<_>>();
            let (status, body) = crate::retention_tests::run_async(request(
                crate::test_support::shell_router(root.path()),
                case["method"].as_str().expect("method"),
                case["path"].as_str().expect("path"),
                Some(&case["sent"]),
            ));
            assert_response(name, case, root.path(), status, body);
            assert_config_result(name, case, root.path());
            let after_bytes =
                fs::read(root.path().join("config/journal.json")).expect("config after");
            for (section, bytes) in malformed_before {
                if case["config_before"][&section] == case["config_after"][&section] {
                    assert_eq!(
                        raw_top_level_field(&after_bytes, &section),
                        bytes,
                        "{name} malformed {section} bytes"
                    );
                }
            }
        }
    });
}

// Negative twin for the two pins above. It stages the exact host state that
// broke the replay — a resolvable `solstone-retention` on PATH *and* in
// SOLSTONE_RETENTION_BIN, i.e. any machine that has run `make install` — and
// asserts the pinned replay still reproduces the recorded 500 without ever
// spawning it. Without this, a future change that drops the pin stays green on
// a bare CI box and only turns red on a developer's machine.
#[test]
fn prune_logs_replay_refuses_even_when_a_host_executor_is_resolvable() {
    let _serialized = crate::retention_tests::executor_env_guard();
    let host = tempfile::Builder::new()
        .prefix("host-executor-")
        .tempdir()
        .expect("space-free host directory");
    assert!(!host.path().display().to_string().contains(' '));
    let planted = host.path().join(crate::retention_executor::BINARY);
    let marker = host.path().join("spawned.marker");
    fs::write(
        &planted,
        format!(
            "#!/bin/sh\nprintf 'spawned\\n' >> {}\nprintf '%s' '{{\"marks\":{{}}}}'\n",
            marker.display()
        ),
    )
    .expect("planted executor bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&planted, fs::Permissions::from_mode(0o755)).expect("planted mode");
    }

    let corpus = crate::test_support::corpus();
    let name = "POST prune-logs.dry-run";
    let case = &corpus["mutations"][name];
    assert_eq!(case["status"].as_u64(), Some(500), "recorded contract");
    let root = root_from(&case["config_before"]);

    let host_path = host.path().display().to_string();
    let planted_path = planted.display().to_string();
    let (status, body) = temp_env::with_vars(
        [
            ("PATH", Some(host_path.as_str())),
            ("SOLSTONE_RETENTION_BIN", Some(planted_path.as_str())),
        ],
        || {
            // The staged host is genuinely hostile: the production resolver
            // finds the planted executor before the pin is applied.
            assert!(
                crate::retention_executor::binary().is_ok(),
                "staged host must resolve an executor, or this twin proves nothing"
            );
            crate::retention_tests::without_executor(|| {
                assert!(
                    crate::retention_executor::binary().is_err(),
                    "the pin must make the executor unresolvable"
                );
                crate::retention_tests::run_async(request(
                    crate::test_support::shell_router(root.path()),
                    case["method"].as_str().expect("method"),
                    case["path"].as_str().expect("path"),
                    Some(&case["sent"]),
                ))
            })
        },
    );

    assert_response(name, case, root.path(), status, body);
    assert_config_result(name, case, root.path());
    assert!(
        !marker.exists(),
        "the pinned replay must never spawn the host executor"
    );
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
        "PUT storage.bad-mode" | "PUT storage.bad-days" | "PUT storage.logs-bad-days" => {
            ("PUT", "/app/settings/api/storage")
        }
        "POST prune-logs.bad-days" => ("POST", "/app/settings/api/storage/prune-logs"),
        other => panic!("unknown refusal route: {other}"),
    }
}

fn inventory_path(path: &str) -> &str {
    match path {
        "/app/settings/api/facet/no-such" => "/app/settings/api/facet/work-life",
        "/app/settings/api/facet/no-such/rename" => "/app/settings/api/facet/work-life/rename",
        other => other,
    }
}

#[tokio::test]
async fn ac3_refusals_replay_across_all_non_corrupt_phases() {
    let corpus = crate::test_support::corpus();
    let phases = ["established", "rich", "populated", "tokened", "malformed"];
    let mut total = 0;
    let mut per_phase = None;
    for phase in phases {
        let cases = corpus["phases"][phase].as_object().expect("phase");
        let mutation_cases = cases
            .iter()
            .filter(|(name, _)| {
                name.starts_with("POST ") || name.starts_with("PUT ") || name.starts_with("DELETE ")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            per_phase.get_or_insert(mutation_cases.len()),
            &mutation_cases.len()
        );
        for (name, case) in mutation_cases {
            let root = crate::test_support::phase_root(phase);
            let before = fs::read(root.path().join("config/journal.json")).expect("config before");
            let (method, path) = refusal_route(name);
            assert!(
                MUTATION_PAIRS.contains(&(method, inventory_path(path))),
                "{phase} {name} maps to an inventory route"
            );
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
    assert_eq!(per_phase, Some(27));
    assert_eq!(total, 27 * 5);
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
async fn ac12_explicit_nineteen_pair_inventory() {
    assert_eq!(MUTATION_PAIRS.len(), 19);
    let root = crate::test_support::populated_root();
    for (method, path) in MUTATION_PAIRS {
        let response = crate::test_support::shell_router(root.path())
            .oneshot(body_request(method, path, Some(&json!({}))))
            .await
            .expect("response");
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
    }
    let corpus = crate::test_support::corpus();
    for collection in ["mutations", "mutations_malformed"] {
        for (name, case) in corpus[collection].as_object().expect("mutation collection") {
            assert!(
                MUTATION_PAIRS.contains(&(
                    case["method"].as_str().expect("method"),
                    inventory_path(case["path"].as_str().expect("path"))
                )),
                "{collection} {name}"
            );
        }
    }
    for (phase, cases) in corpus["phases"].as_object().expect("phases") {
        for (name, _) in cases.as_object().expect("phase") {
            if name.starts_with("POST ") || name.starts_with("PUT ") || name.starts_with("DELETE ")
            {
                let (method, path) = refusal_route(name);
                assert!(
                    MUTATION_PAIRS.contains(&(method, inventory_path(path))),
                    "{phase} {name}"
                );
            }
        }
    }
}
