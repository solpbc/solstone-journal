// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use solstone_core_journal_io::{LockOptions, hold_lock};
use tower::ServiceExt;

use super::{Fixture, write_json};

async fn request(path: &str, method: &str, body: Body, fixture: &Fixture) -> (StatusCode, Value) {
    let response = router(fixture.0.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8(bytes.to_vec()).expect("text")));
    (status, value)
}

async fn post(path: &str, body: &str, fixture: &Fixture) -> (StatusCode, Value) {
    request(path, "POST", Body::from(body.to_owned()), fixture).await
}

fn established() -> Fixture {
    let fixture = Fixture::new();
    fixture.established();
    fixture
}

fn journal_config(fixture: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fixture.0.join("config/journal.json")).expect("config"))
        .expect("config JSON")
}

#[cfg(unix)]
fn history_line_count(path: &std::path::Path) -> usize {
    fs::read_to_string(path)
        .map(|contents| contents.lines().count())
        .unwrap_or(0)
}

#[tokio::test]
async fn write_routes_inherit_the_unestablished_session_gate() {
    let fixture = Fixture::new();
    let response = router(fixture.0.clone())
        .oneshot(
            Request::post("/app/thinking/api/set-owner")
                .body(Body::from(r#"{"name":"Ada"}"#))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/init");
}

#[tokio::test]
async fn deleted_set_name_and_reset_routes_return_not_found() {
    let fixture = established();
    for path in ["/app/thinking/api/set-name", "/app/thinking/api/reset"] {
        let (status, _value) = post(path, r#"{"name":"Nova"}"#, &fixture).await;
        // The retired app has no route, so every method receives the unknown-app refusal.
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn set_owner_refuses_a_path_shaped_name() {
    let fixture = established();
    let before = journal_config(&fixture)["identity"]["name"].clone();
    for name in ["~/x", "a/b", "a\\b"] {
        let (status, value) = post(
            "/app/thinking/api/set-owner",
            &json!({"name": name}).to_string(),
            &fixture,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}");
        assert_eq!(value["reason_code"], json!("invalid_config_value"));
        assert_eq!(journal_config(&fixture)["identity"]["name"], before);
    }
}

#[tokio::test]
async fn set_owner_accepts_a_benign_name() {
    let fixture = established();
    let (status, value) = post("/app/thinking/api/set-owner", r#"{"name":"Ada"}"#, &fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["name"], json!("Ada"));
    assert_eq!(journal_config(&fixture)["identity"]["name"], "Ada");
}

#[tokio::test]
async fn set_owner_preserves_null_bio_and_returns_reference_shape() {
    let fixture = established();
    for (body, expected) in [
        (r#"{"name":"Ada"}"#, json!("")),
        (r#"{"name":"Ada","bio":null}"#, json!("")),
        (r#"{"name":"Ada","bio":""}"#, json!("")),
        (r#"{"name":"Ada","bio":123}"#, json!(123)),
        (r#"{"name":"Ada","bio":["x"]}"#, json!(["x"])),
    ] {
        let (status, value) = post("/app/thinking/api/set-owner", body, &fixture).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(value, json!({"name":"Ada","bio":expected}));
    }

    let (status, value) = post(
        "/app/thinking/api/set-owner",
        r#"{"name":"Ada","bio":0.0}"#,
        &fixture,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({"name":"Ada","bio":""}));
    let config: Value =
        serde_json::from_slice(&fs::read(fixture.0.join("config/journal.json")).expect("config"))
            .expect("config JSON");
    assert_eq!(config["identity"]["bio"], json!(0.0));
}

#[tokio::test]
async fn sol_init_ignores_absent_malformed_and_extra_bodies() {
    for body in [
        Body::empty(),
        Body::from("not-json"),
        Body::from(r#"{"unexpected":"value","name":"ignored"}"#),
    ] {
        let fixture = established();
        let (status, value) = request("/app/thinking/api/sol-init", "POST", body, &fixture).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["identity_dir"], json!(fixture.0.join("identity")));
        assert_eq!(value["status"], json!("ok"));
        assert!(fixture.0.join("identity/partner.md").exists());
        assert!(fixture.0.join("identity/health.md").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn sol_init_preserves_existing_seed_files_and_history() {
    let fixture = established();
    let identity_dir = fixture.0.join("identity");
    fs::create_dir_all(&identity_dir).expect("identity directory");
    let partner = identity_dir.join("partner.md");
    fs::write(&partner, b"owner partner bytes\n").expect("partner");
    let partner_bytes = fs::read(&partner).expect("partner bytes");
    let partner_inode = fs::metadata(&partner).expect("partner metadata").ino();
    let history = identity_dir.join("history.jsonl");

    let (status, value) = request(
        "/app/thinking/api/sol-init",
        "POST",
        Body::empty(),
        &fixture,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({"identity_dir": identity_dir, "status": "ok"}));
    assert_eq!(fs::read(&partner).expect("partner bytes"), partner_bytes);
    assert_eq!(
        fs::metadata(&partner).expect("partner metadata").ino(),
        partner_inode
    );
    assert!(identity_dir.join("health.md").exists());
    assert_eq!(history_line_count(&history), 1);
    assert!(
        fs::read_to_string(&history)
            .expect("history")
            .contains(r#""file":"health.md""#)
    );

    let fixture = established();
    let identity_dir = fixture.0.join("identity");
    fs::create_dir_all(&identity_dir).expect("identity directory");
    let partner = identity_dir.join("partner.md");
    let health = identity_dir.join("health.md");
    fs::write(&partner, b"owner partner bytes\n").expect("partner");
    fs::write(&health, b"owner health bytes\n").expect("health");
    let partner_bytes = fs::read(&partner).expect("partner bytes");
    let partner_inode = fs::metadata(&partner).expect("partner metadata").ino();
    let health_bytes = fs::read(&health).expect("health bytes");
    let health_inode = fs::metadata(&health).expect("health metadata").ino();
    let history = identity_dir.join("history.jsonl");

    let (status, value) = request(
        "/app/thinking/api/sol-init",
        "POST",
        Body::from(r#"{"ignored":true}"#),
        &fixture,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({"identity_dir": identity_dir, "status": "ok"}));
    assert_eq!(fs::read(&partner).expect("partner bytes"), partner_bytes);
    assert_eq!(
        fs::metadata(&partner).expect("partner metadata").ino(),
        partner_inode
    );
    assert_eq!(fs::read(&health).expect("health bytes"), health_bytes);
    assert_eq!(
        fs::metadata(&health).expect("health metadata").ino(),
        health_inode
    );
    assert_eq!(history_line_count(&history), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn sol_init_append_failure_rolls_back_new_seed_files() {
    let fixture = established();
    let identity_dir = fixture.0.join("identity");
    fs::create_dir_all(&identity_dir).expect("identity directory");
    let history = identity_dir.join("history.jsonl");
    fs::write(&history, b"").expect("history");
    fs::set_permissions(&history, fs::Permissions::from_mode(0o444)).expect("history mode");

    if OpenOptions::new().append(true).open(&history).is_ok() {
        fs::set_permissions(&history, fs::Permissions::from_mode(0o600)).expect("restore mode");
        return;
    }

    let (status, _value) = request(
        "/app/thinking/api/sol-init",
        "POST",
        Body::empty(),
        &fixture,
    )
    .await;
    fs::set_permissions(&history, fs::Permissions::from_mode(0o600)).expect("restore mode");

    assert_ne!(status, StatusCode::OK);
    assert!(!identity_dir.join("partner.md").exists());
    assert!(!identity_dir.join("health.md").exists());
    assert_eq!(history_line_count(&history), 0);
}

#[tokio::test]
async fn populated_different_mutations_preserve_siblings_and_reference_responses() {
    let fixture = Fixture::new();
    write_json(
        &fixture.0.join("config/journal.json"),
        json!({
            "setup": {"completed_at": 1},
            "root_sibling": true,
            "identity": {
                "name": "Old",
                "bio": "old bio",
                "sibling": {"keep": true},
            },
        }),
    );
    let (status, value) = post(
        "/app/thinking/api/set-owner",
        r#"{"name":"Ada","bio":"new bio"}"#,
        &fixture,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({"name":"Ada","bio":"new bio"}));
    assert_eq!(
        journal_config(&fixture)["identity"],
        json!({"name":"Ada","bio":"new bio","sibling":{"keep":true}})
    );
    assert_eq!(journal_config(&fixture)["root_sibling"], json!(true));
}

#[tokio::test]
async fn leftover_agent_object_survives_set_owner() {
    let leftover = json!({
        "name": "Ada",
        "name_status": "chosen",
        "named_date": "2026-01-02",
        "sibling": true,
    });
    let fixture = Fixture::new();
    write_json(
        &fixture.0.join("config/journal.json"),
        json!({
            "setup": {"completed_at": 1},
            "agent": leftover.clone(),
            "identity": {
                "name": "Old",
                "bio": "old bio",
            },
        }),
    );
    let (status, value) = post(
        "/app/thinking/api/set-owner",
        r#"{"name":"Ada","bio":"new bio"}"#,
        &fixture,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({"name":"Ada","bio":"new bio"}));
    assert_eq!(journal_config(&fixture)["agent"], leftover);
}

#[tokio::test]
async fn absent_owned_state_persists_only_when_the_mutation_changes_something() {
    let fixture = Fixture::new();
    write_json(
        &fixture.0.join("config/journal.json"),
        json!({"setup": {"completed_at": 1}}),
    );
    let (status, value) = post("/app/thinking/api/set-owner", r#"{"name":"Ada"}"#, &fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({"name":"Ada","bio":""}));
    assert_eq!(journal_config(&fixture)["identity"], json!({"name":"Ada"}));
}

#[tokio::test]
async fn non_object_owned_state_is_refused_without_replacement() {
    for (path, body, config, key) in [(
        "/app/thinking/api/set-owner",
        r#"{"name":"Ada"}"#,
        json!({"setup":{"completed_at":1},"identity":7}),
        "identity",
    )] {
        let fixture = Fixture::new();
        let config_path = fixture.0.join("config/journal.json");
        write_json(&config_path, config);
        let bytes = fs::read(&config_path).expect("config bytes");

        let (status, value) = post(path, body, &fixture).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        assert_eq!(value["reason_code"], json!("internal_error"), "{path}");
        assert_eq!(
            value["detail"],
            json!(format!("{key} must be a JSON object"))
        );
        assert_eq!(
            fs::read(&config_path).expect("config bytes"),
            bytes,
            "{path}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn populated_same_reset_and_set_owner_leave_bytes_inode_and_lock_mtime_unchanged() {
    for (path, body, config) in [(
        "/app/thinking/api/set-owner",
        r#"{"name":"Ada"}"#.to_owned(),
        json!({"setup":{"completed_at":1},"identity":{"name":"Ada","bio":"kept","sibling":true}}),
    )] {
        let fixture = Fixture::new();
        let config_path = fixture.0.join("config/journal.json");
        write_json(&config_path, config);
        let sentinel = fixture.0.join("config/journal.json.lock");
        fs::write(&sentinel, b"sentinel").expect("sentinel");
        let bytes = fs::read(&config_path).expect("bytes");
        let inode = fs::metadata(&config_path).expect("metadata").ino();
        let sentinel_mtime = fs::metadata(&sentinel)
            .expect("sentinel metadata")
            .modified()
            .expect("mtime");

        let (status, _value) = post(path, &body, &fixture).await;

        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(fs::read(&config_path).expect("bytes"), bytes, "{path}");
        assert_eq!(
            fs::metadata(&config_path).expect("metadata").ino(),
            inode,
            "{path}"
        );
        assert_eq!(
            fs::metadata(&sentinel)
                .expect("sentinel metadata")
                .modified()
                .expect("mtime"),
            sentinel_mtime,
            "{path}"
        );
    }
}

#[tokio::test]
async fn changed_config_write_under_lock_returns_identity_busy() {
    let fixture = established();
    let config_path = fixture.0.join("config/journal.json");
    let _lock = hold_lock(&config_path, LockOptions::default()).expect("lock");

    let (status, value) = post("/app/thinking/api/set-owner", r#"{"name":"Ada"}"#, &fixture).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["reason_code"], json!("identity_busy"));
    assert_eq!(
        value["error"],
        json!(
            "your journal's identity couldn't be updated right now because it was busy. try again in a moment."
        )
    );
    assert_eq!(value["detail"], json!("identity is busy; try again"));
}
