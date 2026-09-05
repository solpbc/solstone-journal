// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::Local;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_shell::authorization_gate::authorized_router;
use solstone_core_convey_shell::{ConveyServeOptions, router, serve};
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::AuthorizedClientsRead;
use tokio::sync::watch;
use tower::ServiceExt;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const ESTABLISHED_DEFERRED: [&str; 0] = [];

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-convey-shell-{name}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary journal creates");
        Self(path)
    }

    fn write_config(&self, bytes: &[u8]) {
        fs::create_dir_all(self.0.join("config")).expect("config directory creates");
        fs::write(self.0.join("config/journal.json"), bytes).expect("config writes");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn corpus() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/convey_shell_corpus.json"))
        .expect("corpus parses")
}

/// Permanent documented divergence, introduced 2026-08-14, with no expiry
/// condition: the frozen corpus permanently records the deleted reference's
/// `reflections` app, which was dropped by ruling and whose Python surface no
/// longer exists. The corpus CANNOT be regenerated -- its generator needs a
/// runnable reference tree and this wave removes it -- so the fixture is a
/// frozen record and the divergence is absorbed here instead. Because this
/// cannot expire, narrowness is the safeguard: it is keyed to the one dropped
/// entry and removes exactly that element. Never generalize this into a rule
/// over app names, and never retire it.
fn apply_permanent_reflections_drop_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    assert_eq!(
        apps.iter()
            .filter(|app| app["name"] == "reflections")
            .count(),
        1,
        "frozen shell contains exactly one reflections app"
    );
    apps.retain(|app| app["name"] != "reflections");
}

/// Permanent documented divergence, introduced 2026-08-15, with no expiry
/// condition: the frozen corpus permanently records the deleted reference's
/// `tokens` app, which was removed by ruling once its usage surface moved
/// natively under `stats`. The corpus CANNOT be regenerated -- its generator
/// needs a runnable reference tree and this wave removes it -- so the
/// fixture is a frozen record and the divergence is absorbed here instead.
/// Because this cannot expire, narrowness is the safeguard: it is keyed to
/// the one removed entry and removes exactly that element. Never generalize
/// this into a rule over app names, and never retire it.
fn apply_permanent_tokens_removal_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    assert_eq!(
        apps.iter().filter(|app| app["name"] == "tokens").count(),
        1,
        "frozen shell contains exactly one tokens app"
    );
    apps.retain(|app| app["name"] != "tokens");
}

/// Permanent documented divergence, introduced 2026-08-15, with no expiry
/// condition: the frozen corpus permanently records the deleted reference's
/// `sol` app, whose identity mutations moved natively under Thinking. The
/// corpus CANNOT be regenerated -- its generator needs a runnable reference
/// tree and this wave removes it -- so the fixture is a frozen record and the
/// divergence is absorbed here instead. Because this cannot expire,
/// narrowness is the safeguard: it is keyed to the one dropped entry and
/// removes exactly that element. Never generalize this into a rule over app
/// names, and never retire it.
fn apply_permanent_sol_removal_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    assert_eq!(
        apps.iter().filter(|app| app["name"] == "sol").count(),
        1,
        "frozen shell contains exactly one sol app"
    );
    apps.retain(|app| app["name"] != "sol");
}

/// Permanent documented divergence, introduced 2026-08-20, with no expiry
/// condition: the frozen corpus permanently records the deleted reference's
/// `chat` app and `chat_bar` object, whose Convey chrome this wave removes. The
/// corpus CANNOT be regenerated -- its generator needs a runnable reference
/// tree and this wave removes it -- so the fixture is a frozen record and the
/// divergence is absorbed here instead. Because this cannot expire,
/// narrowness is the safeguard: it is keyed to the one dropped app row and
/// the chat_bar object and removes exactly those elements. Never generalize
/// this into a rule over app names, and never retire it.
fn apply_permanent_chat_removal_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    assert_eq!(
        apps.iter().filter(|app| app["name"] == "chat").count(),
        1,
        "frozen shell contains exactly one chat app"
    );
    apps.retain(|app| app["name"] != "chat");
    assert!(
        expected.get("chat_bar").is_some_and(Value::is_object),
        "frozen shell contains a chat_bar object"
    );
    expected
        .as_object_mut()
        .expect("shell payload is an object")
        .remove("chat_bar");
}

#[test]
fn permanent_chat_removal_divergence_requires_exactly_one_chat_row() {
    for (case, mut expected) in [
        ("zero", json!({"apps": [], "chat_bar": {}})),
        (
            "two",
            json!({"apps": [{"name": "chat"}, {"name": "chat"}], "chat_bar": {}}),
        ),
    ] {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply_permanent_chat_removal_divergence(&mut expected);
            }))
            .is_err(),
            "{case} chat rows must fail the narrow divergence"
        );
    }
}

#[test]
fn permanent_sol_removal_divergence_requires_exactly_one_sol_row() {
    for (case, mut expected) in [
        ("zero", json!({"apps": []})),
        ("two", json!({"apps": [{"name": "sol"}, {"name": "sol"}]})),
    ] {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply_permanent_sol_removal_divergence(&mut expected);
            }))
            .is_err(),
            "{case} sol rows must fail the narrow divergence"
        );
    }
}

/// Permanent documented divergence, introduced 2026-08-13, with no expiry
/// condition: the frozen corpus permanently records the deleted reference's
/// `observer` app, which was dropped by ruling once its capture-device
/// registry moved natively under Network. The corpus CANNOT be regenerated --
/// its generator needs a runnable reference tree and this wave removes it --
/// so the fixture is a frozen record and the divergence is absorbed here
/// instead. Because this cannot expire, narrowness is the safeguard: it is
/// keyed to the one dropped entry and removes exactly that element. Never
/// generalize this into a rule over app names, and never retire it.
fn apply_permanent_devices_shell_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    assert_eq!(
        apps.iter().filter(|app| app["name"] == "observer").count(),
        1,
        "frozen shell contains exactly one observer app"
    );
    apps.retain(|app| app["name"] != "observer");
}

fn apply_client_capture_projection_rename(expected: &mut Value) {
    let version = expected["version"]
        .as_object_mut()
        .expect("frozen system status contains version");
    for field in ["current", "latest"] {
        assert_eq!(
            version.get(field),
            Some(&Value::String("1.0.22".to_owned())),
            "frozen system status contains the v1.0.22 {field} projection"
        );
        version.insert(
            field.to_owned(),
            Value::String(env!("CARGO_PKG_VERSION").to_owned()),
        );
    }
    let capture = expected["capture"]
        .as_object_mut()
        .expect("frozen system status contains capture");
    let observers = capture
        .remove("observers")
        .expect("frozen system status contains observers");
    capture.insert("clients".to_owned(), observers);
    capture.insert("status".to_owned(), Value::String("no_clients".to_owned()));
    capture.insert("unassessed".to_owned(), Value::Array(Vec::new()));
    capture.insert(
        "registry".to_owned(),
        Value::String("registry_empty".to_owned()),
    );
}

/// Permanent documented divergence, introduced 2026-08-24, with no expiry
/// condition: the frozen corpus preserves the deleted reference's per-app
/// `starred` preference, while the native shell has no backing route for it.
/// The corpus CANNOT be regenerated -- its generator needs a runnable
/// reference tree and this wave removes it -- so the fixture is a frozen
/// record and the divergence is absorbed here instead. Because this cannot
/// expire, narrowness is the safeguard: it removes exactly the obsolete key
/// from every frozen app row. Never generalize this into a rule over app
/// fields, and never retire it.
fn apply_permanent_starred_removal_divergence(expected: &mut Value) {
    let apps = expected["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    let starred_count = apps
        .iter()
        .filter(|app| app.get("starred").is_some_and(Value::is_boolean))
        .count();
    assert_eq!(
        starred_count,
        apps.len(),
        "frozen shell contains boolean starred on every app row"
    );
    for app in apps {
        app.as_object_mut()
            .expect("shell app is an object")
            .remove("starred")
            .expect("frozen shell app contains starred");
    }
}

/// The frozen corpus predates removal of the global shell facet contract.
/// Validate every frozen field before removing it, so the final equality check
/// proves the native shell no longer publishes the contract.
fn apply_permanent_global_facet_contract_removal_divergence(expected: &mut Value) {
    let root = expected.as_object_mut().expect("frozen shell is an object");
    assert!(
        matches!(root.remove("facets"), Some(Value::Array(_))),
        "frozen shell contains an array facets field"
    );
    assert!(
        matches!(
            root.remove("selected_facet"),
            Some(Value::Null | Value::String(_))
        ),
        "frozen shell contains a null or string selected_facet field"
    );
    let apps = root
        .get_mut("apps")
        .and_then(Value::as_array_mut)
        .expect("shell apps are an array");
    for app in apps {
        app.as_object_mut()
            .expect("shell app is an object")
            .remove("facets_enabled")
            .filter(Value::is_boolean)
            .expect("frozen shell app contains boolean facets_enabled");
    }
}

fn strip_permanent_launcher_metadata_from_actual(actual: &mut Value) {
    let apps = actual["apps"]
        .as_array_mut()
        .expect("shell apps are an array");
    for app in apps {
        let object = app.as_object_mut().expect("shell app is an object");
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>");

        let launcher_group = object
            .get("launcher_group")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("actual shell app {name} has no string launcher_group"));
        assert!(
            matches!(launcher_group, "your_journal" | "understand" | "manage"),
            "actual shell app {name} has invalid launcher_group {launcher_group:?}"
        );
        assert!(
            object.get("launcher_rank").is_some_and(Value::is_u64),
            "actual shell app {name} has no non-negative integer launcher_rank"
        );
        match object.get("rail_group") {
            Some(Value::Null) => {}
            Some(Value::String(group)) if matches!(group.as_str(), "primary" | "management") => {}
            Some(Value::String(group)) => {
                panic!("actual shell app {name} has invalid rail_group {group:?}")
            }
            Some(_) => panic!("actual shell app {name} has no string or null rail_group"),
            None => panic!("actual shell app {name} has no rail_group"),
        }
        assert!(
            object.get("rail_rank").is_some_and(Value::is_u64),
            "actual shell app {name} has no non-negative integer rail_rank"
        );

        for field in ["launcher_group", "launcher_rank", "rail_group", "rail_rank"] {
            object
                .remove(field)
                .expect("validated launcher metadata exists");
        }
    }
}

#[test]
fn permanent_starred_removal_divergence_requires_starred_on_every_app_row() {
    let mut expected = json!({
        "apps": [
            {"name": "home", "starred": false},
            {"name": "search"}
        ]
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            apply_permanent_starred_removal_divergence(&mut expected);
        }))
        .is_err(),
        "missing starred must fail the narrow divergence"
    );
}

#[test]
fn global_facet_contract_divergence_rejects_missing_or_malformed_fields_before_row_removal() {
    let fixture = || {
        json!({
            "facets": [],
            "selected_facet": null,
            "apps": [
                {"name": "home", "facets_enabled": false},
                {"name": "sol", "facets_enabled": true}
            ]
        })
    };
    for name in ["home", "sol"] {
        for value in [None, Some(Value::String("true".to_owned()))] {
            let mut expected = fixture();
            let app = expected["apps"]
                .as_array_mut()
                .expect("apps are an array")
                .iter_mut()
                .find(|app| app["name"] == name)
                .expect("named app exists")
                .as_object_mut()
                .expect("app object");
            match value {
                Some(value) => {
                    app.insert("facets_enabled".to_owned(), value);
                }
                None => {
                    app.remove("facets_enabled");
                }
            }
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    apply_permanent_global_facet_contract_removal_divergence(&mut expected);
                }))
                .is_err(),
                "{name} must be validated before later row removal"
            );
        }
    }
    for (field, malformed) in [
        ("facets", Value::Bool(false)),
        ("selected_facet", Value::Array(Vec::new())),
    ] {
        let mut expected = fixture();
        expected
            .as_object_mut()
            .expect("shell object")
            .insert(field.to_owned(), malformed);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply_permanent_global_facet_contract_removal_divergence(&mut expected);
            }))
            .is_err(),
            "malformed {field} must fail"
        );
    }
}

#[test]
fn permanent_launcher_metadata_divergence_validates_and_strips_only_metadata() {
    let app = || {
        json!({
            "name": "home",
            "starred": false,
            "launcher_group": "your_journal",
            "launcher_rank": 0,
            "rail_group": "primary",
            "rail_rank": 0
        })
    };

    let mut actual = json!({
        "apps": [
            app(),
            {
                "name": "body",
                "other_key": "unchanged",
                "launcher_group": "your_journal",
                "launcher_rank": 4,
                "rail_group": null,
                "rail_rank": 0
            }
        ]
    });
    strip_permanent_launcher_metadata_from_actual(&mut actual);
    assert_eq!(
        actual,
        json!({
            "apps": [
                {"name": "home", "starred": false},
                {"name": "body", "other_key": "unchanged"}
            ]
        }),
        "metadata stripping preserves all other app keys"
    );

    for (case, mut actual) in [
        (
            "missing field",
            json!({"apps": [{
                "name": "home",
                "launcher_group": "your_journal",
                "launcher_rank": 0,
                "rail_group": "primary"
            }]}),
        ),
        (
            "wrong type",
            json!({"apps": [{
                "name": "home",
                "launcher_group": "your_journal",
                "launcher_rank": "0",
                "rail_group": "primary",
                "rail_rank": 0
            }]}),
        ),
        (
            "unknown group",
            json!({"apps": [{
                "name": "home",
                "launcher_group": "other",
                "launcher_rank": 0,
                "rail_group": "primary",
                "rail_rank": 0
            }]}),
        ),
    ] {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                strip_permanent_launcher_metadata_from_actual(&mut actual);
            }))
            .is_err(),
            "{case} launcher metadata must fail validation"
        );
    }
}

fn journal_for_phase(phase: &str) -> TempDir {
    let journal = TempDir::new(phase);
    match phase {
        "unestablished" => {}
        "established" => {
            journal.write_config(br#"{"setup":{"completed_at":1767225600}}"#);
            let principal = journal.0.join("entities/principal");
            fs::create_dir_all(&principal).expect("principal directory creates");
            fs::write(
                principal.join("entity.json"),
                br#"{"id":"principal","name":"Corpus Owner","type":"Person","is_principal":true}"#,
            )
            .expect("principal identity writes");
        }
        "corrupt" => journal.write_config(br#"{"setup":{"completed_at":17672256"#),
        _ => panic!("unknown corpus phase {phase}"),
    }
    journal
}

async fn get(app: axum::Router, path: &str) -> (StatusCode, String, Option<String>, Vec<u8>) {
    let response = app
        .oneshot(
            Request::get(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .expect("response content type")
        .to_str()
        .expect("content type is text")
        .to_owned();
    let location = response
        .headers()
        .get("location")
        .map(|value| value.to_str().expect("location is text").to_owned());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads")
        .to_vec();
    (status, content_type, location, body)
}

fn normalize(value: &mut Value, journal_root: &str, path: &str) {
    match value {
        Value::Object(object) => {
            for (key, item) in object {
                let next = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                normalize(item, journal_root, &next);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item, journal_root, path);
            }
        }
        Value::String(text) => {
            if text.contains(journal_root) {
                *text = text.replace(journal_root, "<JOURNAL_ROOT>");
            } else if text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_digit()) {
                *text = "<TODAY>".to_owned();
            } else if path == "version" && text.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                *text = "<VERSION>".to_owned();
            }
        }
        _ => {}
    }
}

fn normalize_body_root(body: &[u8], journal_root: &str) -> Vec<u8> {
    let needle = journal_root.as_bytes();
    let replacement = b"<JOURNAL_ROOT>";
    let mut normalized = Vec::with_capacity(body.len());
    let mut remaining = body;
    while let Some(index) = remaining
        .windows(needle.len())
        .position(|window| window == needle)
    {
        normalized.extend_from_slice(&remaining[..index]);
        normalized.extend_from_slice(replacement);
        remaining = &remaining[index + needle.len()..];
    }
    normalized.extend_from_slice(remaining);
    normalized
}

#[tokio::test]
async fn corpus_gate_and_converted_surface_match_all_non_deferred_cases() {
    let corpus = corpus();
    let phases = corpus["phases"].as_object().expect("phases are object");
    let mut established_asserted = 0;
    let mut established_deferred = 0;

    for (phase, cases) in phases {
        let journal = journal_for_phase(phase);
        let (_, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let app = authorized_router(journal.0.clone(), authorization)
            .into_inner()
            .layer(Extension(AccessBasis::Localhost));
        for case in cases.as_array().expect("phase cases are array") {
            let path = case["path"].as_str().expect("case path");
            if phase == "established" && ESTABLISHED_DEFERRED.contains(&path) {
                established_deferred += 1;
                continue;
            }
            if phase == "established" {
                established_asserted += 1;
            }
            let (status, content_type, location, body) = get(app.clone(), path).await;
            assert_eq!(
                status.as_u16(),
                case["status"].as_u64().unwrap() as u16,
                "{phase} {path}"
            );
            assert_eq!(
                content_type,
                case["content_type"].as_str().unwrap(),
                "{phase} {path}"
            );
            assert_eq!(
                location.as_deref(),
                case.get("location").and_then(Value::as_str),
                "{phase} {path}"
            );

            if let Some(expected_json) = case.get("json") {
                let mut actual: Value =
                    serde_json::from_slice(&body).expect("JSON response parses");
                let mut expected = expected_json.clone();
                if phase == "established" && path == "/api/shell" {
                    apply_permanent_global_facet_contract_removal_divergence(&mut expected);
                    apply_permanent_devices_shell_divergence(&mut expected);
                    apply_permanent_reflections_drop_divergence(&mut expected);
                    apply_permanent_tokens_removal_divergence(&mut expected);
                    apply_permanent_sol_removal_divergence(&mut expected);
                    apply_permanent_chat_removal_divergence(&mut expected);
                    apply_permanent_starred_removal_divergence(&mut expected);
                    strip_permanent_launcher_metadata_from_actual(&mut actual);
                }
                if phase == "established" && path == "/api/system/status" {
                    apply_client_capture_projection_rename(&mut expected);
                }
                normalize(&mut actual, &journal.0.display().to_string(), "");
                normalize(&mut expected, &journal.0.display().to_string(), "");
                assert_eq!(actual, expected, "{phase} {path}");
            } else {
                let body = if case.get("body_normalized").is_some() {
                    normalize_body_root(&body, &journal.0.display().to_string())
                } else {
                    body
                };
                // A record with no `body_sha256` is deliberately not body-asserted: served
                // frontend assets (html/js/css) carry status, content type and disposition
                // only. See the corpus_maintenance note in the fixture.
                if let Some(recorded) = case.get("body_sha256").and_then(Value::as_str) {
                    let digest = format!("{:x}", Sha256::digest(&body));
                    assert_eq!(digest, recorded, "{phase} {path}");
                }
            }
        }
    }

    assert_eq!(
        established_asserted, 18,
        "all 18 established probes are asserted"
    );
    assert_eq!(established_deferred, 0);
}

#[tokio::test]
async fn speakers_state_uses_the_python_local_date_semantics() {
    let journal = journal_for_phase("established");
    let (_, _, _, body) = get(router(journal.0.clone()), "/app/speakers/api/state").await;
    let state: Value = serde_json::from_slice(&body).expect("speakers state parses");
    assert_eq!(
        state["today"],
        Value::String(Local::now().format("%Y%m%d").to_string())
    );
    assert_eq!(state["speaker_copy"].as_object().unwrap().len(), 120);
}

#[tokio::test]
async fn registry_and_unconverted_refusal_contract_are_stable() {
    let journal = journal_for_phase("established");
    let (_, _, _, shell_body) = get(router(journal.0.clone()), "/api/shell").await;
    let shell: Value = serde_json::from_slice(&shell_body).expect("shell parses");
    let apps = shell["apps"].as_array().expect("apps array");
    assert!(shell.get("chat_bar").is_none());
    assert_eq!(apps.len(), 18);
    for app in apps {
        // `starred` and the global facet capability were removed; launcher and rail metadata add four fields.
        assert_eq!(app.as_object().unwrap().len(), 12);
        assert!(app["icon_svg"].is_string());
    }
    let backgrounds: Vec<_> = apps
        .iter()
        .filter_map(|app| app["background_url"].as_str())
        .collect();
    assert_eq!(
        backgrounds,
        ["/app/support/background", "/app/timeline/background"]
    );

    let (status, content_type, _, body) =
        get(router(journal.0.clone()), "/app/activities/workspace").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(content_type, "application/json");
    let refusal: Value = serde_json::from_slice(&body).expect("refusal parses");
    assert_eq!(refusal["reason_code"], "app_not_converted");
    assert_eq!(refusal["app"], "activities");

    let shell = include_bytes!("../assets/static/shell.html");
    for path in ["/app/activities/", "/app/activities"] {
        let (status, content_type, _, body) = get(router(journal.0.clone()), path).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}");
        assert_eq!(content_type, "text/html; charset=utf-8", "{path}");
        assert_eq!(body, shell, "{path}");
    }
}

#[tokio::test]
async fn sol_cut_uses_unknown_app_404_across_methods_without_session_gate_change() {
    let journal = journal_for_phase("established");
    let app = router(journal.0.clone());
    let mut first_404_body = None;
    for path in ["/app/sol", "/app/sol/", "/app/sol/background"] {
        let (status, content_type, location, body) = get(app.clone(), path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(content_type, "text/html; charset=utf-8", "{path}");
        assert!(location.is_none(), "{path}");
        if let Some(expected) = &first_404_body {
            assert_eq!(&body, expected, "{path}");
        } else {
            first_404_body = Some(body);
        }
    }

    for path in ["/app/sol/api/set-owner", "/app/sol/api/sol-init"] {
        let response = app
            .clone()
            .oneshot(
                Request::post(path)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(response.headers().get("location").is_none(), "{path}");
    }

    for phase in ["unestablished", "corrupt"] {
        let journal = journal_for_phase(phase);
        let app = router(journal.0.clone());
        let (status, _content_type, location, _body) = get(app.clone(), "/app/sol/").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{phase}");
        assert!(location.is_none(), "{phase}");

        let response = app
            .oneshot(
                Request::post("/app/sol/api/set-owner")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{phase}");
        assert!(response.headers().get("location").is_none(), "{phase}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unestablished_loopback_serves_the_init_wizard() {
    let journal = journal_for_phase("unestablished");
    let handle = serve(ConveyServeOptions {
        journal_root: journal.0.clone(),
        loopback_port: 0,
        door_port: 0,
        handshake_timeout: Duration::from_secs(2),
        stream_stall_timeout: Duration::from_secs(2),
        router: router(journal.0.clone()),
        carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
        handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
    })
    .await
    .expect("loopback serve");
    let port = handle.loopback_ipv4_addr().port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .write_all(b"GET /init HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .expect("write");
    let mut body = String::new();
    stream.read_to_string(&mut body).expect("read");
    handle.shutdown();
    assert!(
        body.starts_with("HTTP/1.1 200"),
        "expected 200 from /init, got:\n{body}"
    );
    assert!(
        body.contains("create your journal"),
        "expected wizard HTML, got:\n{body}"
    );
}

#[tokio::test]
async fn sse_is_gated_and_exposes_a_heartbeat_after_establishment() {
    let unestablished = journal_for_phase("unestablished");
    let (status, _, location, _) = get(router(unestablished.0.clone()), "/sse/events").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(location.as_deref(), Some("/init"));

    let established = journal_for_phase("established");
    let response = router(established.0.clone())
        .oneshot(Request::get("/sse/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-cache");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
}

#[tokio::test]
async fn an_unconverted_app_refusal_is_never_a_success_status() {
    // Found in a browser, not a test: shell_boot.js only evaluates a response
    // when `response.ok`, so a refusal at 200 was parsed as JavaScript. Every
    // client branches on that bit -- pin it rather than the exact code.
    let journal = journal_for_phase("established");
    for path in [
        // `/app/home/` and `/app/home/workspace` were here until 2026-08-14, when
        // the home page's shell and workspace became native routes. The INVARIANT is
        // unchanged -- an unconverted app's refusal is never 2xx -- only these
        // examples went stale. Replaced with a still-unconverted app rather than
        // deleted, so the assertion keeps covering shell and workspace paths.
        "/app/activities/",
        "/app/activities/workspace",
        // `/app/tokens/background` was here until 2026-08-15, when the tokens
        // registry entry was removed and its usage surface moved natively under
        // stats; `/app/support/background` replaced it and went stale the same
        // day, when the support conversion landed and made that path serve the
        // app's real backdrop -- support is the only app in its plate whose
        // background is real content rather than a 404. The INVARIANT is
        // unchanged -- an unconverted app's refusal is never 2xx -- only these
        // examples went stale. Replaced with a still-unconverted app rather than
        // deleted, so the assertion keeps covering a background path.
        "/app/activities/background",
        // `/app/sol/background` was here until 2026-08-15, when Sol's
        // registry entry was removed after its identity mutations moved
        // natively under Thinking. The INVARIANT is unchanged -- an
        // unconverted app's refusal is never 2xx -- only this example went
        // stale. Replaced with a still-unconverted app rather than deleted,
        // so the assertion keeps covering a background path.
        // `/app/network/background` was here until 2026-08-18, when Network
        // flipped converted: its mint, ceremony, devices, and write routes
        // were already native, and the 501 "not ported" body on unmounted
        // paths was the lie. activities remains the unconverted example.
    ] {
        let (status, _content_type, _location, body) = get(router(journal.0.clone()), path).await;
        assert!(
            !status.is_success(),
            "{path} returned a success status for an unconverted app: {status}"
        );
        // On 2026-08-24, the `/` tail moved to the shell HTML navigation response
        // while the workspace tail retained its JSON fragment refusal.
        if path.ends_with("/workspace") || path.ends_with("/background") {
            let refusal: Value = serde_json::from_slice(&body)
                .unwrap_or_else(|_| panic!("{path} refusal parses as JSON"));
            assert_eq!(refusal["reason_code"], "app_not_converted", "{path}");
        }
    }
}

#[cfg(unix)]
mod live_sse {
    use super::*;
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::time::{Duration, timeout};

    async fn frame(body: &mut axum::body::BodyDataStream) -> String {
        let bytes = timeout(Duration::from_secs(3), body.next())
            .await
            .expect("SSE must make progress")
            .expect("SSE remains open")
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn until(body: &mut axum::body::BodyDataStream, needle: &str) -> String {
        timeout(Duration::from_secs(4), async {
            loop {
                let frame = frame(body).await;
                if frame.contains(needle) {
                    return frame;
                }
            }
        })
        .await
        .expect("expected SSE frame")
    }

    async fn open() -> (
        TempDir,
        UnixListener,
        UnixStream,
        axum::body::BodyDataStream,
    ) {
        let journal = journal_for_phase("established");
        std::fs::create_dir_all(journal.0.join("health")).unwrap();
        let listener = UnixListener::bind(journal.0.join("health/callosum.sock")).unwrap();
        let response = router(journal.0.clone())
            .oneshot(Request::get("/sse/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        let (socket, _) = timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert!(
            until(&mut body, "\"connected\"")
                .await
                .starts_with("event: continuity\n")
        );
        (journal, listener, socket, body)
    }

    #[tokio::test]
    async fn envelopes_reach_the_browser_and_body_drop_closes_the_socket() {
        let (_journal, _listener, mut socket, mut body) = open().await;
        let envelope = serde_json::json!({"tract":"supervisor", "event":"status", "ts":123,
            "services":[{"name":"sense"}], "extension":{"line":"one\ntwo"}});
        socket
            .write_all(format!("{envelope}\n").as_bytes())
            .await
            .unwrap();
        let data = until(&mut body, "\"supervisor\"").await;
        assert!(!data.contains("event: continuity"));
        let payload = data.strip_prefix("data: ").unwrap().trim();
        assert_eq!(serde_json::from_str::<Value>(payload).unwrap(), envelope);
        drop(body);
        let mut byte = [0];
        assert_eq!(
            timeout(Duration::from_secs(3), socket.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn broker_disconnect_is_visible_before_fresh_reconnected_data() {
        let (_journal, listener, socket, mut body) = open().await;
        drop(socket);
        let gap = until(&mut body, "\"gapped\"").await;
        assert!(gap.starts_with("event: continuity\n"));
        let (mut replacement, _) = timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        until(&mut body, "\"connected\"").await;
        replacement
            .write_all(b"{\"tract\":\"cortex\",\"event\":\"status\",\"running_uses\":1}\n")
            .await
            .unwrap();
        assert!(
            until(&mut body, "\"cortex\"")
                .await
                .contains("\"running_uses\":1")
        );
    }

    #[tokio::test]
    async fn malformed_frame_reports_gap_and_never_becomes_browser_data() {
        let (_journal, _listener, mut socket, mut body) = open().await;
        socket.write_all(b"this is not json\n").await.unwrap();
        until(&mut body, "\"gapped\"").await;
        until(&mut body, "\"connected\"").await;
        socket
            .write_all(b"{\"tract\":\"observe\",\"event\":\"status\"}\n")
            .await
            .unwrap();
        assert!(until(&mut body, "\"observe\"").await.starts_with("data: "));
    }
}
