// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::disallowed_methods,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "the process-boundary harness owns its temporary journal"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

static EXECUTOR_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn fixed_removal_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0)
        .single()
        .expect("fixed removal clock")
}

const STUB: &str = r#"#!/bin/sh
id=''
expect_mark=0
for value in "$@"; do
  if [ "$expect_mark" = 1 ]; then id="$value"; expect_mark=0; fi
  if [ "$value" = '--mark' ]; then expect_mark=1; fi
done
printf '%s\n' "$*" >> "$HOME_REMOVALS_LOG"
case "$id" in
  a*) printf '%s' "$HOME_REMOVALS_SUCCESS"; exit 0 ;;
  *) printf '%s' "$HOME_REMOVALS_RECEIPT"; exit "$HOME_REMOVALS_EXIT" ;;
esac
"#;

struct Harness {
    root: TempDir,
    stub: PathBuf,
    log: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("home-removals-")
            .tempdir()
            .expect("temporary journal");
        write_json(
            root.path(),
            "config/journal.json",
            &json!({
                "setup": {"completed_at": 1_700_000_000_000_i64},
                "retention": {"raw_media": "days", "raw_media_days": 1},
            }),
        );
        let stub = root.path().join("retention-stub");
        fs::write(&stub, STUB).expect("stub bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("stub mode");
        }
        let log = root.path().join("retention.log");
        Self { root, stub, log }
    }

    fn router(&self) -> Router {
        solstone_core_home_web::routes(
            self.root.path().to_path_buf(),
            solstone_core_home_web::Clock::fixed(fixed_removal_now()),
            |_| false,
        )
    }

    fn call<T>(&self, receipt: &Value, exit: &str, success: &Value, work: impl FnOnce() -> T) -> T {
        let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
        let binary = self.stub.display().to_string();
        let log = self.log.display().to_string();
        let receipt = receipt.to_string();
        let success = success.to_string();
        temp_env::with_vars(
            [
                ("SOLSTONE_RETENTION_BIN", Some(binary.as_str())),
                ("HOME_REMOVALS_LOG", Some(log.as_str())),
                ("HOME_REMOVALS_RECEIPT", Some(receipt.as_str())),
                ("HOME_REMOVALS_EXIT", Some(exit)),
                ("HOME_REMOVALS_SUCCESS", Some(success.as_str())),
            ],
            work,
        )
    }

    fn without_executor<T>(&self, work: impl FnOnce() -> T) -> T {
        let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
        let empty = tempfile::Builder::new()
            .prefix("home-removals-empty-")
            .tempdir()
            .expect("empty binary directory");
        let path = empty.path().display().to_string();
        temp_env::with_vars(
            [
                ("SOLSTONE_RETENTION_BIN", None),
                ("PATH", Some(path.as_str())),
                ("HOME_REMOVALS_LOG", None),
                ("HOME_REMOVALS_RECEIPT", None),
                ("HOME_REMOVALS_EXIT", None),
                ("HOME_REMOVALS_SUCCESS", None),
            ],
            work,
        )
    }

    fn invocation_count(&self) -> usize {
        self.invocation_args().len()
    }

    fn invocation_args(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(path, serde_json::to_vec(value).expect("JSON")).expect("JSON file");
}

fn request(method: &str, path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn runtime<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(work)
}

fn response(router: Router, request: Request<Body>) -> (StatusCode, Value) {
    runtime(async move {
        let response = router.oneshot(request).await.expect("response");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        (
            status,
            serde_json::from_slice(&body).expect("response JSON"),
        )
    })
}

fn action_records(root: &Path) -> Vec<Value> {
    let directory = root.join("config/actions");
    let entries = fs::read_dir(directory).expect("action directory");
    entries
        .flat_map(|entry| {
            fs::read_to_string(entry.expect("action entry").path())
                .expect("action file")
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .map(|line| serde_json::from_str(&line).expect("action JSON"))
        .collect()
}

#[test]
fn removal_card_escapes_journal_values_before_rendering_markup() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/removals_escape.js"))
        .arg(manifest_dir)
        .output()
        .expect("removal card escape harness");
    assert!(
        output.status.success(),
        "removal card escape harness: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn waiting_list_rows_follow_their_sources() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/needs_you_render.js"))
        .arg(manifest_dir)
        .output()
        .expect("needs-you render harness");
    assert!(
        output.status.success(),
        "needs-you render harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn connections_scheduled_shelf_is_names_only() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/connections_scheduled_render.js"))
        .arg(manifest_dir)
        .output()
        .expect("connections-scheduled render harness");
    assert!(
        output.status.success(),
        "connections-scheduled render harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn vitals_render_calm_neutral_without_attention_chip() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/vitals_render.js"))
        .arg(manifest_dir)
        .output()
        .expect("vitals render harness");
    assert!(
        output.status.success(),
        "vitals render harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn narrative_and_briefing_placeholder_states_stay_honest() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/narrative_briefing_render.js"))
        .arg(manifest_dir)
        .output()
        .expect("narrative and briefing render harness");
    assert!(
        output.status.success(),
        "narrative and briefing render harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reconnect_rereads_what_a_lost_completion_event_would_have_refreshed() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/reconnect_refresh.js"))
        .arg(manifest_dir)
        .output()
        .expect("reconnect refresh harness");
    assert!(
        output.status.success(),
        "reconnect refresh harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_failed_night_is_at_most_two_waiting_rows() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/yesterday_processing_render.js"))
        .arg(manifest_dir)
        .output()
        .expect("yesterday-processing render harness");
    assert!(
        output.status.success(),
        "yesterday-processing render harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn today_rows_carry_the_servers_source_label_only_where_it_is_set() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/today_activity_source_render.js"))
        .arg(manifest_dir)
        .output()
        .expect("today activity source render harness");
    assert!(
        output.status.success(),
        "today activity source render harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn recover_receipt(targets: Value, halted: Value) -> Value {
    json!({
        "ok": true,
        "verb": "recover",
        "outcome": {"targets": targets, "halted": halted},
        "index": {"ok": true, "chunks": 0, "files": 0},
        "detail": {"verb": "recover"},
    })
}

fn recover_target(removed: &[&str], leftover: bool) -> Value {
    json!({
        "target": {"day": "20260101", "stream": "_default", "dir": "070000_17"},
        "removed": removed,
        "not_removed": if leftover {
            json!([{
                "entry": "chronicle/20260101/.removing_070000_17",
                "reason": "r",
                "staged": "chronicle/20260101/.removing_070000_17",
            }])
        } else {
            json!([])
        },
    })
}

fn post_recover(harness: &Harness, receipt: &Value, exit: &str) -> (StatusCode, Value) {
    harness.call(receipt, exit, &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/recover", json!({})),
        )
    })
}

fn empty_recover_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/app/home/api/recover")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(""))
        .expect("request")
}

#[test]
fn recover_invokes_the_executor_with_the_hardcoded_actor_tokens() {
    let harness = Harness::new();
    let response = post_recover(&harness, &recover_receipt(json!([]), Value::Null), "0");
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(response.1["state"], "recover.none");
    assert_eq!(response.1["finished_count"], 0);
    assert!(response.1.get("requested_count").is_none());
    let line = &harness.invocation_args()[0];
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    assert_eq!(tokens[0], "recover", "recover argv verb: {line}");
    assert_eq!(tokens[1], "--journal");
    assert_eq!(tokens[2], harness.root.path().display().to_string());
    assert_eq!(tokens[3], "--at");
    assert!(
        tokens[4].contains('T') && tokens[4].ends_with('Z'),
        "recover --at is RFC 3339: {}",
        tokens[4]
    );
    assert_eq!(&tokens[5..], ["--did", "owner", "--reason", "owner"]);
}

#[test]
fn recover_maps_a_missing_binary_to_tool_unavailable() {
    let harness = Harness::new();
    let response = harness.without_executor(|| {
        response(
            harness.router(),
            request("POST", "/app/home/api/recover", json!({})),
        )
    });
    assert_eq!(response.1["state"], "tool.unavailable");
    assert_eq!(response.1["finished_count"], 0);
    assert!(!harness.log.exists());
}

#[test]
fn recover_accepts_empty_and_empty_object_bodies_and_rejects_the_rest() {
    let harness = Harness::new();
    let receipt = recover_receipt(json!([]), Value::Null);
    let empty = harness.call(&receipt, "0", &json!({}), || {
        response(harness.router(), empty_recover_request())
    });
    assert_eq!(empty.1["state"], "recover.none");

    let object = post_recover(&harness, &receipt, "0");
    assert_eq!(object.1["state"], "recover.none");

    for body in [
        json!({"mark_ids": ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}),
        json!({"did": "owner"}),
        json!([]),
        Value::Null,
        json!("x"),
    ] {
        let refused = response(
            harness.router(),
            request("POST", "/app/home/api/recover", body),
        );
        assert_eq!(refused.0, StatusCode::BAD_REQUEST);
        assert_eq!(refused.1["state"], "request.invalid");
    }
    assert_eq!(harness.invocation_count(), 2);
}

#[test]
fn recover_maps_receipt_shapes_without_consulting_policy() {
    let harness = Harness::new();
    write_json(
        harness.root.path(),
        "config/journal.json",
        &json!({
            "setup": {"completed_at": 1_700_000_000_000_i64},
            "retention": {"raw_media": "keep", "empty_audio": "keep"},
        }),
    );

    let leftover = post_recover(
        &harness,
        &recover_receipt(json!([recover_target(&[], true)]), Value::Null),
        "3",
    );
    assert_eq!(leftover.1["state"], "recover.failed");
    assert_eq!(leftover.1["finished_count"], 0);
    assert_eq!(leftover.1["not_finished_count"], 1);
    assert_eq!(leftover.1["refusals"][0]["name"], ".removing_070000_17");

    let mixed = post_recover(
        &harness,
        &json!({
            "ok": false,
            "verb": "recover",
            "outcome": {
                "targets": [
                    recover_target(&["chronicle/20260101/_default/070000_17/a.flac"], false),
                    {
                        "target": {"day": "20260102", "stream": "_default", "dir": "080000_17"},
                        "removed": [],
                        "not_removed": [{
                            "entry": "chronicle/20260102/.removing_080000_17",
                            "reason": "r",
                            "staged": "chronicle/20260102/.removing_080000_17",
                        }],
                    }
                ],
                "halted": null,
            },
            "index": {"ok": true, "chunks": 0, "files": 0},
            "detail": {"verb": "recover"},
        }),
        "3",
    );
    assert_eq!(mixed.1["state"], "recover.failed");
    assert_eq!(mixed.1["finished_count"], 1);
    assert_eq!(mixed.1["not_finished_count"], 1);

    let done = post_recover(
        &harness,
        &recover_receipt(json!([recover_target(&[], false)]), Value::Null),
        "0",
    );
    assert_eq!(done.1["state"], "recover.done");
    assert_eq!(done.1["finished_count"], 1);
    assert_eq!(done.1["not_finished_count"], 0);

    let halted = post_recover(
        &harness,
        &recover_receipt(json!([]), json!({"reason": "h"})),
        "4",
    );
    assert_eq!(halted.1["state"], "recover.failed");
    assert!(halted.1["halted"].as_bool().expect("halted"));

    let no_outcome = harness.call(
        &json!({"ok": false, "verb": "recover", "error": "e"}),
        "3",
        &json!({}),
        || {
            response(
                harness.router(),
                request("POST", "/app/home/api/recover", json!({})),
            )
        },
    );
    assert_eq!(no_outcome.1["state"], "outcome.unknown");
    assert_eq!(no_outcome.1["finished_count"], 0);

    let unknown = harness.call(
        &recover_receipt(json!([]), Value::Null),
        "2",
        &json!({}),
        || {
            response(
                harness.router(),
                request("POST", "/app/home/api/recover", json!({})),
            )
        },
    );
    assert_eq!(unknown.1["state"], "outcome.unknown");

    let records = action_records(harness.root.path());
    assert_eq!(records.len(), 6);
    assert!(
        records
            .iter()
            .all(|record| record["action"] == "removal_recover")
    );
    assert!(
        records
            .iter()
            .all(|record| record.get("requested_count").is_none())
    );
}

#[test]
fn getting_started_render_card_structure() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // without_executor empties PATH for the whole process; hold its lock so node resolves.
    let _env = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let output = Command::new("node")
        .arg(manifest_dir.join("tests/getting_started_render.js"))
        .arg(manifest_dir)
        .output()
        .expect("getting-started render harness");
    assert!(
        output.status.success(),
        "getting-started render harness: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
