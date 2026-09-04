// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "the process-boundary harness owns its temporary journal"
)]

#[path = "removals/support.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Days, Local, SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_retention_client::{RemovalClass, policy_from_retention};
use tempfile::TempDir;
use tower::ServiceExt;

static EXECUTOR_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
            solstone_core_home_web::Clock::system(),
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

fn class(removal_class: RemovalClass) -> Value {
    serde_json::to_value(removal_class).expect("class JSON")
}

fn mark(removal_class: RemovalClass, state: Value, id: &str, names: &[&str]) -> Value {
    json!({
        "id": id,
        "class": class(removal_class),
        "target": {"day": "20260101", "stream": "_default", "dir": "070000_17"},
        "marked_at": "2026-01-01T00:00:00Z",
        "proposal": {"bytes": 12, "reason": "r", "names": names},
        "state": state,
    })
}

fn marks_receipt(marks: Value) -> Value {
    json!({"ok": true, "verb": "marks", "marks": {"version": 1, "marks": marks}})
}

fn target(removed: &[&str], entries: &[&str], halted: bool) -> Value {
    let entries = entries
        .iter()
        .map(|entry| json!({"entry": entry, "reason": "r", "staged": null}))
        .collect::<Vec<_>>();
    json!({
        "ok": true,
        "verb": "remove-marked",
        "outcome": {
            "targets": [{"target": {}, "removed": removed, "not_removed": entries}],
            "halted": halted.then(|| json!({"reason": "h"})),
        },
        "index": {},
        "detail": {},
    })
}

fn decline_receipt() -> Value {
    json!({"ok": true, "verb": "decline", "marks": {"version": 1, "marks": {}}})
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
fn list_projects_only_pending_approval_and_all_failures() {
    let harness = Harness::new();
    let receipt = marks_receipt(json!({
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": mark(RemovalClass::PolicyRawRelease, json!("marked"), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &["a", "b"]),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb": mark(RemovalClass::OwnerRawRelease, json!("marked"), "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &["c"]),
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc": mark(RemovalClass::PolicyRawRelease, json!({"failed": {"at": "2026-01-01T00:00:01Z", "reason": "r", "staged": "chronicle/20260101/.removing_070000_17"}}), "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc", &[]),
    }));
    let response = harness.call(&receipt, "0", &json!({}), || {
        response(
            harness.router(),
            request("GET", "/app/home/api/removals", Value::Null),
        )
    });

    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(response.1["state"], "list.ready");
    let rows = response.1["removals"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    let marked = rows
        .iter()
        .find(|row| row["state"] == "marked")
        .expect("marked row");
    assert_eq!(
        marked.as_object().expect("marked fields").len(),
        11,
        "marked rows contain only their projection fields"
    );
    assert_eq!(marked["count"], 2);
    assert_eq!(marked["bytes"], 12);
    assert_eq!(marked["size"], "12 B");
    assert_eq!(marked["stream"], "_default");
    assert!(marked.get("reason").is_none());
    let failed = rows
        .iter()
        .find(|row| row["state"] == "failed")
        .expect("failed row");
    let failed_fields = failed.as_object().expect("failed fields");
    assert_eq!(
        failed_fields.len(),
        14,
        "failed rows contain their exact fields"
    );
    assert!(
        [
            "id",
            "class",
            "origin",
            "day",
            "stream",
            "dir",
            "marked_at",
            "count",
            "bytes",
            "size",
            "state",
            "at",
            "reason",
            "staged",
        ]
        .iter()
        .all(|field| failed_fields.contains_key(*field)),
        "failed rows contain only their declared fields"
    );
    assert_eq!(failed["staged"], "chronicle/20260101/.removing_070000_17");
    assert_eq!(failed["reason"], "r");
}

#[test]
fn policy_that_cannot_release_skips_the_client_and_its_action_log() {
    let harness = Harness::new();
    write_json(
        harness.root.path(),
        "config/journal.json",
        &json!({
            "setup": {"completed_at": 1_700_000_000_000_i64},
            "retention": {"raw_media": "keep", "empty_audio": "keep"},
        }),
    );
    let response = response(
        harness.router(),
        request(
            "POST",
            "/app/home/api/approve",
            json!({"mark_ids": ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]}),
        ),
    );
    assert_eq!(response.1["state"], "approve.policy_keeps");
    assert!(!harness.log.exists());
    assert!(!harness.root.path().join("config/actions").exists());
}

#[test]
fn list_maps_the_marks_refusal_shape_to_register_unavailable() {
    let harness = Harness::new();
    let receipt = json!({"ok": false, "verb": "marks", "error": "e"});
    let response = harness.call(&receipt, "3", &json!({}), || {
        response(
            harness.router(),
            request("GET", "/app/home/api/removals", Value::Null),
        )
    });
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(
        response.1,
        json!({"state": "list.register_unavailable", "removals": []})
    );
}

#[test]
fn write_requests_reject_invalid_and_over_cap_bodies_before_an_invocation() {
    let harness = Harness::new();
    let invalid = response(
        harness.router(),
        request("POST", "/app/home/api/approve", json!({})),
    );
    assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.1["state"], "request.invalid");

    let ids = (0..33)
        .map(|number| json!(format!("{number:064x}")))
        .collect::<Vec<_>>();
    let over_cap = harness.without_executor(|| {
        response(
            harness.router(),
            request("POST", "/app/home/api/decline", json!({"mark_ids": ids})),
        )
    });
    assert_eq!(over_cap.0, StatusCode::BAD_REQUEST);
    assert_eq!(over_cap.1["state"], "request.too_large");
    assert!(!harness.log.exists());
    assert!(!harness.root.path().join("config/actions").exists());
}

#[test]
fn approve_maps_preflight_partial_and_halted_receipts_and_logs_the_attempt() {
    let harness = Harness::new();
    let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let preflight = json!({"ok": false, "verb": "remove-marked", "error": "e"});
    let preflight = harness.call(&preflight, "3", &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(preflight.1["state"], "approve.refused_before_start");

    let partial_receipt = target(&["a"], &["chronicle/20260101/070000_17/b.flac"], false);
    let partial = harness.call(&partial_receipt, "3", &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(partial.1["state"], "approve.partial");
    assert_eq!(partial.1["removed_count"], 1);
    assert_eq!(partial.1["not_removed_count"], 1);
    assert_eq!(
        partial.1["refusals"][0],
        json!({"state": "refusal.item_named", "name": "b.flac", "reason": "r"})
    );
    assert_ne!(partial.1["state"], "approve.refused_after_start");

    let halted_receipt = target(&["a"], &[], true);
    let halted = harness.call(&halted_receipt, "4", &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(halted.1["state"], "approve.halted");
    assert_eq!(halted.1["removed_count"], 1);
    assert!(halted.1["halted"].as_bool().expect("halted"));

    let records = action_records(harness.root.path());
    assert_eq!(records.len(), 3);
    assert!(
        records
            .iter()
            .all(|record| record["action"] == "removal_approve")
    );
}

#[test]
fn segment_refusal_names_only_the_directory_basename() {
    let harness = Harness::new();
    let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let receipt = target(&[], &["chronicle/20260101/070000_17"], false);
    let named = harness.call(&receipt, "3", &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    let name = named.1["refusals"][0]["name"].as_str().expect("item name");
    assert_eq!(name, "070000_17");
    assert!(!name.contains('/'));

    let unnamed_receipt = target(&[], &[""], false);
    let unnamed = harness.call(&unnamed_receipt, "3", &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(unnamed.1["refusals"][0]["state"], "refusal.item_unnamed");
    assert!(unnamed.1["refusals"][0].get("name").is_none());
    assert_eq!(unnamed.1["refusals"][0]["reason"], "r");
}

#[test]
fn decline_runs_each_id_and_aggregates_the_result() {
    let harness = Harness::new();
    let success = decline_receipt();
    let refusal = json!({"ok": false, "verb": "decline", "error": "e"});
    let response = harness.call(&refusal, "3", &success, || {
        response(
            harness.router(),
            request(
                "POST",
                "/app/home/api/decline",
                json!({"mark_ids": [
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ]}),
            ),
        )
    });
    assert_eq!(response.1["state"], "declined.partial");
    assert_eq!(response.1["declined_count"], 1);
    assert_eq!(response.1["refused_count"], 1);
    assert_eq!(harness.invocation_count(), 2);
    assert_eq!(
        action_records(harness.root.path())[0]["action"],
        "removal_decline"
    );
}

#[test]
fn decline_maps_client_unknown_and_unavailable_to_distinct_non_delete_states() {
    let harness = Harness::new();
    let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let unknown = harness.call(&decline_receipt(), "2", &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/decline", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(unknown.1["state"], "declined.unknown");
    assert_eq!(unknown.1["unknown_count"], 1);

    let unavailable = harness.without_executor(|| {
        response(
            harness.router(),
            request("POST", "/app/home/api/decline", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(unavailable.1["state"], "tool.unavailable");
    assert_eq!(unavailable.1["unavailable_count"], 1);
}

#[test]
fn list_maps_an_unknown_marks_outcome_without_claiming_a_write_outcome() {
    let harness = Harness::new();
    let response = harness.call(&marks_receipt(json!({})), "2", &json!({}), || {
        response(
            harness.router(),
            request("GET", "/app/home/api/removals", Value::Null),
        )
    });
    assert_eq!(
        response.1,
        json!({"state": "outcome.unknown", "removals": []})
    );
}

#[test]
fn unavailable_and_unknown_attempts_keep_distinct_state_codes_and_are_logged() {
    let harness = Harness::new();
    let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let unavailable = harness.without_executor(|| {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(unavailable.1["state"], "tool.unavailable");

    let unknown = harness.call(&target(&["a"], &[], false), "2", &json!({}), || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(unknown.1["state"], "outcome.unknown");
    assert_eq!(action_records(harness.root.path()).len(), 2);
}

#[test]
fn action_log_failure_does_not_change_the_client_outcome() {
    let harness = Harness::new();
    fs::write(harness.root.path().join("config/actions"), b"x").expect("action obstacle");
    let receipt = target(&["a"], &[], false);
    let response = harness.call(&receipt, "0", &json!({}), || {
        response(
            harness.router(),
            request(
                "POST",
                "/app/home/api/approve",
                json!({"mark_ids": ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]}),
            ),
        )
    });
    assert_eq!(response.1["state"], "approve.deleted");
}

#[test]
fn removal_card_escapes_journal_values_before_rendering_markup() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
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
fn needs_you_items_render_informational_without_affordances() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
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
fn vitals_render_calm_neutral_without_attention_chip() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
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
fn yesterday_processing_splits_failures_from_neutral_summary() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
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

fn run_retention(binary: &Path, args: &[&str]) -> Value {
    let output = Command::new(binary)
        .args(args)
        .output()
        .expect("retention command");
    assert!(output.status.success(), "retention command: {output:?}");
    serde_json::from_slice(&output.stdout).expect("retention receipt")
}

fn seed_segment(root: &Path) -> PathBuf {
    seed_segment_on(root, "20260701")
}

fn seed_segment_on(root: &Path, day: &str) -> PathBuf {
    let segment = root.join(format!("chronicle/{day}/field.audio/070000_17"));
    fs::create_dir_all(&segment).expect("segment");
    let raw = b"raw";
    fs::write(segment.join("audio.flac"), raw).expect("raw");
    fs::write(segment.join("notes.txt"), b"notes").expect("other file");
    let header = json!({
        "segment": "070000_17",
        "_solstone_processing": {
            "schema": "solstone.processing.v1",
            "state": "analyzed",
            "reason_code": "ok",
            "handler": "transcribe",
            "attempted_at": "2026-07-01T00:00:00Z",
            "input_size": raw.len(),
        },
    });
    fs::write(
        segment.join("audio.jsonl"),
        format!("{header}\n{{\"start\":0.0,\"text\":\"x\"}}\n"),
    )
    .expect("sidecar");
    segment
}

fn seed_empty_terminal_on(root: &Path, day: &str) -> PathBuf {
    let segment = root.join(format!("chronicle/{day}/field.audio/070000_17"));
    fs::create_dir_all(&segment).expect("segment");
    let raw = b"raw";
    fs::write(segment.join("audio.flac"), raw).expect("raw");
    let header = json!({
        "segment": "070000_17",
        "_solstone_processing": {
            "schema": "solstone.processing.v1",
            "state": "empty",
            "reason_code": "no_decodable_audio",
            "handler": "transcribe",
            "attempted_at": "2026-07-01T00:00:00Z",
            "input_size": raw.len(),
        },
    });
    fs::write(segment.join("audio.jsonl"), format!("{header}\n")).expect("sidecar");
    segment
}

fn keep_journal_product_policy() -> String {
    serde_json::to_string(&policy_from_retention(
        json!({"raw_media": "keep"}).as_object().expect("retention"),
    ))
    .expect("policy JSON")
}

#[test]
fn real_executor_approval_releases_only_named_files_and_drops_the_mark() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    {
        let segment = seed_segment(harness.root.path());
        let policy =
            r#"{"default_rule":{"anchor":"captured","period":1,"priority":0},"enabled":true}"#;
        let root = harness.root.path().display().to_string();
        let marked = run_retention(
            &binary,
            &[
                "mark",
                "--journal",
                &root,
                "--today",
                "2026-08-06",
                "--now",
                "2026-08-06T00:00:00Z",
                "--policy",
                policy,
            ],
        );
        let id = marked["marks"]["marks"]
            .as_object()
            .expect("marks")
            .keys()
            .next()
            .expect("mark id")
            .to_owned();
        let notes = fs::read(segment.join("notes.txt")).expect("other bytes");
        let sidecar = fs::read(segment.join("audio.jsonl")).expect("sidecar bytes");

        let binary = binary.display().to_string();
        let approved =
            temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary.as_str()))], || {
                response(
                    harness.router(),
                    request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
                )
            });
        assert_eq!(approved.1["state"], "approve.deleted");
        assert!(!segment.join("audio.flac").exists());
        assert_eq!(
            fs::read(segment.join("notes.txt")).expect("other bytes"),
            notes
        );
        assert_eq!(
            fs::read(segment.join("audio.jsonl")).expect("sidecar bytes"),
            sidecar
        );
        let repeated =
            temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary.as_str()))], || {
                response(
                    harness.router(),
                    request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
                )
            });
        assert_eq!(repeated.1["state"], "approve.refused_before_start");
        assert_eq!(repeated.1["removed_count"], 0);
        assert!(!segment.join("audio.flac").exists());
        let listed = run_retention(&PathBuf::from(&binary), &["marks", "--journal", &root]);
        assert_eq!(listed["marks"]["marks"], json!({}));
    }
}

#[test]
fn real_executor_refuses_when_the_current_policy_no_longer_releases_the_mark() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    let today = Local::now().date_naive();
    let segment_day = today
        .checked_sub_days(Days::new(2))
        .expect("segment date")
        .format("%Y%m%d")
        .to_string();
    let segment = seed_segment_on(harness.root.path(), &segment_day);
    let policy = r#"{"default_rule":{"anchor":"captured","period":1,"priority":0},"enabled":true}"#;
    let root = harness.root.path().display().to_string();
    let today = today.format("%Y-%m-%d").to_string();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let marked = run_retention(
        &binary,
        &[
            "mark",
            "--journal",
            &root,
            "--today",
            &today,
            "--now",
            &now,
            "--policy",
            policy,
        ],
    );
    let id = marked["marks"]["marks"]
        .as_object()
        .expect("marks")
        .keys()
        .next()
        .expect("mark id")
        .to_owned();
    write_json(
        harness.root.path(),
        "config/journal.json",
        &json!({
            "setup": {"completed_at": 1_700_000_000_000_i64},
            "retention": {"raw_media": "days", "raw_media_days": 4},
        }),
    );

    let binary = binary.display().to_string();
    let response = temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary.as_str()))], || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(response.1["state"], "approve.refused_after_start");
    assert_eq!(response.1["removed_count"], 0);
    assert_eq!(response.1["not_removed_count"], 1);
    assert!(segment.join("audio.flac").exists());
}

#[test]
fn real_executor_decline_drops_the_mark_without_releasing_files() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    {
        let segment = seed_segment(harness.root.path());
        let policy =
            r#"{"default_rule":{"anchor":"captured","period":1,"priority":0},"enabled":true}"#;
        let root = harness.root.path().display().to_string();
        let marked = run_retention(
            &binary,
            &[
                "mark",
                "--journal",
                &root,
                "--today",
                "2026-08-06",
                "--now",
                "2026-08-06T00:00:00Z",
                "--policy",
                policy,
            ],
        );
        let id = marked["marks"]["marks"]
            .as_object()
            .expect("marks")
            .keys()
            .next()
            .expect("mark id")
            .to_owned();
        let binary = binary.display().to_string();
        let declined =
            temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary.as_str()))], || {
                response(
                    harness.router(),
                    request("POST", "/app/home/api/decline", json!({"mark_ids": [id]})),
                )
            });
        assert_eq!(declined.1["state"], "declined.done");
        assert!(segment.join("audio.flac").exists());
        let listed = run_retention(&PathBuf::from(&binary), &["marks", "--journal", &root]);
        assert_eq!(listed["marks"]["marks"], json!({}));
        let empty =
            temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary.as_str()))], || {
                response(
                    harness.router(),
                    request("GET", "/app/home/api/removals", Value::Null),
                )
            });
        assert_eq!(empty.1["state"], "list.empty");
        assert_ne!(empty.1["state"], "list.register_unavailable");
        assert_eq!(empty.1["removals"], json!([]));
    }
}

fn write_keep_journal(root: &Path, empty_audio: Option<&str>) {
    let retention = match empty_audio {
        Some(mode) => json!({"raw_media": "keep", "empty_audio": mode}),
        None => json!({"raw_media": "keep"}),
    };
    write_json(
        root,
        "config/journal.json",
        &json!({
            "setup": {"completed_at": 1_700_000_000_000_i64},
            "retention": retention,
        }),
    );
}

fn mark_journal(binary: &Path, root: &Path, policy: &str) -> String {
    let root = root.display().to_string();
    let marked = run_retention(
        binary,
        &[
            "mark",
            "--journal",
            &root,
            "--today",
            "2026-08-06",
            "--now",
            "2026-08-06T00:00:00Z",
            "--policy",
            policy,
        ],
    );
    marked["marks"]["marks"]
        .as_object()
        .expect("marks")
        .keys()
        .next()
        .expect("mark id")
        .to_owned()
}

fn mark_empty_audio(binary: &Path, root: &Path) -> String {
    mark_journal(binary, root, &keep_journal_product_policy())
}

fn processed_policy(retention: Value) -> String {
    serde_json::to_string(&policy_from_retention(
        retention.as_object().expect("retention"),
    ))
    .expect("policy JSON")
}

fn seed_analyzed_sibling(segment: &Path, name: &str) {
    let extra = b"sibling";
    fs::write(segment.join(name), extra).expect("sibling raw");
    let stem = name.rsplit_once('.').expect("extension").0;
    let header = json!({
        "segment": "070000_17",
        "_solstone_processing": {
            "schema": "solstone.processing.v1",
            "state": "analyzed",
            "reason_code": "ok",
            "handler": "transcribe",
            "attempted_at": "2026-07-01T00:00:00Z",
            "input_size": extra.len(),
        },
    });
    fs::write(
        segment.join(format!("{stem}.jsonl")),
        format!("{header}\n{{\"start\":0.0,\"text\":\"x\"}}\n"),
    )
    .expect("sibling sidecar");
}

fn approve(harness: &Harness, binary: &str, id: &str) -> (StatusCode, Value) {
    temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary))], || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    })
}

#[test]
fn product_keep_journal_approves_empty_audio_release() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    write_keep_journal(harness.root.path(), None);
    let segment = seed_empty_terminal_on(harness.root.path(), "20260701");
    let id = mark_empty_audio(&binary, harness.root.path());
    let binary = binary.display().to_string();
    let approved = temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary.as_str()))], || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(approved.1["state"], "approve.deleted");
    assert!(!segment.join("audio.flac").exists());
}

#[test]
fn product_keep_journal_approves_empty_audio_when_a_sibling_is_no_longer_empty() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    write_keep_journal(harness.root.path(), None);
    let segment = seed_empty_terminal_on(harness.root.path(), "20260701");
    let id = mark_empty_audio(&binary, harness.root.path());
    seed_analyzed_sibling(&segment, "extra.flac");
    let binary = binary.display().to_string();
    let approved = approve(&harness, &binary, &id);
    assert_eq!(approved.1["state"], "approve.deleted");
    assert_eq!(approved.1["removed_count"], 1);
    assert!(!segment.join("audio.flac").exists());
    assert!(segment.join("extra.flac").exists());
}

#[test]
fn product_keep_journal_refuses_when_empty_audio_is_keep() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    write_keep_journal(harness.root.path(), None);
    let segment = seed_empty_terminal_on(harness.root.path(), "20260701");
    let id = mark_empty_audio(&binary, harness.root.path());
    write_keep_journal(harness.root.path(), Some("keep"));
    let binary = binary.display().to_string();
    let refused = temp_env::with_vars([("SOLSTONE_RETENTION_BIN", Some(binary.as_str()))], || {
        response(
            harness.router(),
            request("POST", "/app/home/api/approve", json!({"mark_ids": [id]})),
        )
    });
    assert_eq!(refused.1["state"], "approve.policy_keeps");
    assert!(segment.join("audio.flac").exists());
}

fn write_retention(root: &Path, retention: Value) {
    write_json(
        root,
        "config/journal.json",
        &json!({
            "setup": {"completed_at": 1_700_000_000_000_i64},
            "retention": retention,
        }),
    );
}

#[test]
fn product_processed_journal_releases_ordinary_while_empty_sibling_is_too_young() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    let retention = json!({
        "raw_media": "processed",
        "empty_audio": "days",
        "empty_audio_days": 7,
    });
    write_retention(harness.root.path(), retention.clone());
    let segment = seed_empty_terminal_on(harness.root.path(), "20260815");
    seed_analyzed_sibling(&segment, "extra.flac");
    let id = mark_journal(&binary, harness.root.path(), &processed_policy(retention));
    let binary = binary.display().to_string();
    let approved = approve(&harness, &binary, &id);
    assert_eq!(approved.1["state"], "approve.partial");
    assert_eq!(approved.1["removed_count"], 1);
    assert_eq!(approved.1["not_removed_count"], 1);
    assert!(segment.join("audio.flac").exists());
    assert!(!segment.join("extra.flac").exists());
}

#[test]
fn product_processed_journal_releases_empty_audio_while_ordinary_hits_the_floor() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    let retention = json!({
        "raw_media": "processed",
        "raw_media_minimum_days": 30,
    });
    write_retention(harness.root.path(), retention.clone());
    let segment = harness
        .root
        .path()
        .join("chronicle/20260805/field.audio/070000_17");
    fs::create_dir_all(&segment).expect("segment");
    let empty = b"raw";
    fs::write(segment.join("audio.flac"), empty).expect("raw");
    let empty_header = json!({
        "segment": "070000_17",
        "_solstone_processing": {
            "schema": "solstone.processing.v1",
            "state": "empty",
            "reason_code": "no_decodable_audio",
            "handler": "transcribe",
            "attempted_at": "2026-08-05T00:00:00Z",
            "input_size": empty.len(),
        },
    });
    fs::write(segment.join("audio.jsonl"), format!("{empty_header}\n")).expect("sidecar");
    let extra = b"sibling";
    fs::write(segment.join("extra.flac"), extra).expect("sibling raw");
    let extra_header = json!({
        "segment": "070000_17",
        "_solstone_processing": {
            "schema": "solstone.processing.v1",
            "state": "analyzed",
            "reason_code": "ok",
            "handler": "transcribe",
            "attempted_at": "2026-08-05T00:00:00Z",
            "input_size": extra.len(),
        },
    });
    fs::write(
        segment.join("extra.jsonl"),
        format!("{extra_header}\n{{\"start\":0.0,\"text\":\"x\"}}\n"),
    )
    .expect("sibling sidecar");
    let id = mark_journal(&binary, harness.root.path(), &processed_policy(retention));
    let binary = binary.display().to_string();
    let approved = approve(&harness, &binary, &id);
    assert_eq!(approved.1["state"], "approve.partial");
    assert_eq!(approved.1["removed_count"], 1);
    assert_eq!(approved.1["not_removed_count"], 1);
    assert!(!segment.join("audio.flac").exists());
    assert!(segment.join("extra.flac").exists());
}

#[test]
fn product_processed_journal_releases_only_the_eligible_named_file_from_a_mixed_mark() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    write_retention(harness.root.path(), json!({"raw_media": "processed"}));
    let segment = seed_empty_terminal_on(harness.root.path(), "20260701");
    seed_analyzed_sibling(&segment, "extra.flac");
    let id = mark_journal(
        &binary,
        harness.root.path(),
        &processed_policy(json!({"raw_media": "processed"})),
    );
    write_retention(
        harness.root.path(),
        json!({"raw_media": "processed", "empty_audio": "keep"}),
    );
    let binary = binary.display().to_string();
    let approved = approve(&harness, &binary, &id);
    assert_eq!(approved.1["state"], "approve.partial");
    assert_eq!(approved.1["removed_count"], 1);
    assert_eq!(approved.1["not_removed_count"], 1);
    assert_eq!(approved.1["refusals"][0]["name"], "audio.flac");
    assert!(segment.join("audio.flac").exists());
    assert!(!segment.join("extra.flac").exists());
}

#[test]
fn product_keep_journal_refuses_a_named_file_that_is_no_longer_empty_terminal() {
    let _guard = EXECUTOR_ENV_LOCK.lock().expect("executor environment lock");
    let binary = support::retention_binary();
    let harness = Harness::new();
    write_keep_journal(harness.root.path(), None);
    let segment = seed_empty_terminal_on(harness.root.path(), "20260701");
    let id = mark_empty_audio(&binary, harness.root.path());
    fs::remove_file(segment.join("audio.jsonl")).expect("drop empty sidecar");
    seed_analyzed_sibling(&segment, "audio.flac");
    let binary = binary.display().to_string();
    let refused = approve(&harness, &binary, &id);
    assert_eq!(refused.1["state"], "approve.refused_after_start");
    assert_eq!(refused.1["removed_count"], 0);
    assert_eq!(refused.1["not_removed_count"], 1);
    assert_eq!(refused.1["refusals"][0]["name"], "audio.flac");
    assert!(segment.join("audio.flac").exists());
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
