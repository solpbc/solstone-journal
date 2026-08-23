// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_artifact_download::{ByteDownload, ByteDownloadError};
use solstone_core_backup_runtime::hosted_runtime::HttpError;
use solstone_core_backup_runtime::{
    Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance, JournalMaintenanceError,
    RESTIC_VERSION, ToolOutput, ToolRequest, ToolRunner,
};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tower::ServiceExt;

struct Captured {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn capture(router: axum::Router, request: Request<Body>) -> Captured {
    let response = router.oneshot(request).await.expect("response");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    Captured {
        status,
        headers,
        body,
    }
}

async fn response_json(router: axum::Router, request: Request<Body>) -> (u16, Value) {
    let response = capture(router, request).await;
    (
        response.status,
        serde_json::from_slice(&response.body).expect("json response"),
    )
}

fn corpus_cache() -> crate::measurement::SharedMeasurementCache {
    crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
        total_bytes: Some(crate::test_support::DEVICE_TOTAL_BYTES),
        free_bytes: Some(crate::test_support::DEVICE_FREE_BYTES),
    })
}

#[tokio::test]
async fn corpus_replays_all_cases() {
    let corpus = crate::test_support::corpus();
    let mut asserted = 0;
    let deferred = 0;
    let mut gate = 0;
    for (phase, cases) in corpus["phases"].as_object().expect("phases") {
        let root = crate::test_support::root(phase);
        for case in cases.as_array().expect("cases") {
            let method: Method = case["method"]
                .as_str()
                .expect("method")
                .parse()
                .expect("valid method");
            let body = case
                .get("request_json")
                .map(|value| serde_json::to_vec(value).expect("request json"))
                .unwrap_or_default();
            let request = Request::builder()
                .method(method)
                .uri(case["path"].as_str().expect("path"))
                .body(Body::from(body))
                .expect("request");
            let established = phase != "unestablished" && phase != "corrupt";
            let native = established;
            // The fixture predates the Python-identical backup.js it hashes. The
            // retained compatibility path intentionally serves crate-local backup.js
            // bytes, mirroring assets.rs's deliberate JS byte-identity divergence.
            let native_copy_deviation = native && case["path"] == "/app/backup/static/backup.js";
            let response = if native {
                capture(
                    crate::routes_with_cache(root.path().to_path_buf(), corpus_cache()),
                    request,
                )
                .await
            } else {
                capture(
                    solstone_core_convey_shell::router(root.path().to_path_buf()),
                    request,
                )
                .await
            };
            let expected = case["status"].as_u64().expect("status") as u16;
            assert_eq!(response.status, expected, "{phase} {}", case["path"]);
            assert_eq!(
                response
                    .headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok()),
                case["content_type"].as_str(),
                "{phase} {} content-type",
                case["path"]
            );
            assert_eq!(
                response
                    .headers
                    .get("location")
                    .and_then(|value| value.to_str().ok()),
                case.get("location").and_then(Value::as_str),
                "{phase} {} location",
                case["path"]
            );
            if let Some(expected_json) = case.get("json") {
                assert_eq!(
                    serde_json::from_slice::<Value>(&response.body).expect("json body"),
                    *expected_json,
                    "{phase} {}",
                    case["path"]
                );
            } else if let Some(expected_digest) = case.get("body_sha256")
                && !native_copy_deviation
            {
                let mut body = String::from_utf8_lossy(&response.body).into_owned();
                if case.get("body_normalized").is_some() {
                    body = body.replace(root.path().to_string_lossy().as_ref(), "<JOURNAL_ROOT>");
                }
                assert_eq!(
                    format!("{:x}", Sha256::digest(body.as_bytes())),
                    expected_digest.as_str().expect("digest"),
                    "{phase} {}",
                    case["path"]
                );
            }
            if native {
                asserted += 1;
            } else {
                gate += 1;
            }
        }
    }
    assert_eq!((asserted, deferred, gate), (52, 0, 26));
}

#[tokio::test]
async fn status_shape_preserves_backup_phase_discrimination() {
    for phase in ["fresh", "enabled_never_run", "broken", "healthy"] {
        let root = crate::test_support::root(phase);
        let (status, body) = response_json(
            crate::routes(root.path().to_path_buf()),
            Request::get("/app/backup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body.as_object().expect("object").len(), 17);
        if phase == "broken" {
            assert_eq!(body["last_backup"]["error_reason"], "locked");
            assert_eq!(body["last_verification"]["reason"], "read_data_mismatch");
        }
    }
}

#[tokio::test]
async fn offload_status_uses_injected_geometry_and_has_exact_shape() {
    let root = crate::test_support::root("fresh");
    let (status, body) = response_json(
        crate::routes_with_cache(root.path().to_path_buf(), corpus_cache()),
        Request::get("/app/backup/offload/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body.as_object().expect("object").len(), 13);
    assert_eq!(
        body["device"],
        json!({"free_bytes":crate::test_support::DEVICE_FREE_BYTES,"total_bytes":crate::test_support::DEVICE_TOTAL_BYTES})
    );
    assert_eq!(
        body["suggested_defaults"],
        json!({"budget_bytes":500_000_000_000u64,"floor_bytes":100_000_000_000u64})
    );
}

#[tokio::test]
async fn generation_is_secure_fill_only_and_confirmation_accepts_display_separators() {
    let first_root = crate::test_support::root("fresh");
    let second_root = crate::test_support::root("fresh");
    for root in [&first_root, &second_root] {
        let (status, _) = response_json(
            crate::routes(root.path().to_path_buf()),
            Request::post("/app/backup/keys/generate")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
    }
    let first = crate::config::backup(first_root.path()).expect("first backup");
    let second = crate::config::backup(second_root.path()).expect("second backup");
    assert_ne!(first["daily_key"], second["daily_key"]);
    assert_ne!(first["recovery_key"], second["recovery_key"]);

    let display = crate::keys::format(first["recovery_key"].as_str().expect("key")).unwrap();
    let (_, confirmed) = response_json(
        crate::routes(first_root.path().to_path_buf()),
        Request::post("/app/backup/confirm")
            .body(Body::from(
                serde_json::to_vec(&json!({"recovery_key": display.replace(' ', "-")})).unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(confirmed["recovery_key_confirmed"], true);
}

#[tokio::test]
async fn generation_fills_each_missing_key_without_replacing_the_other() {
    for (daily, recovery) in [
        (Some("existing-daily"), None),
        (None, Some(crate::test_support::RECOVERY_KEY)),
    ] {
        let root = crate::test_support::root("fresh");
        crate::config::mutate(root.path(), |backup| {
            backup.insert(
                "daily_key".to_owned(),
                daily.map_or(Value::Null, |value| json!(value)),
            );
            backup.insert(
                "recovery_key".to_owned(),
                recovery.map_or(Value::Null, |value| json!(value)),
            );
            (true, ())
        })
        .unwrap();
        let (status, _) = response_json(
            crate::routes(root.path().to_path_buf()),
            Request::post("/app/backup/keys/generate")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
        let stored = crate::config::backup(root.path()).unwrap();
        if let Some(daily) = daily {
            assert_eq!(stored["daily_key"], daily);
            assert_ne!(stored["recovery_key"], Value::Null);
        } else {
            assert_eq!(stored["recovery_key"], recovery.unwrap());
            assert_ne!(stored["daily_key"], Value::Null);
        }
    }
}

#[tokio::test]
async fn retention_and_offload_config_persist_their_coerced_values() {
    let root = crate::test_support::root("fresh");
    let root_path = root.path().to_path_buf();
    let (status, _) = response_json(
        crate::routes(root_path.clone()),
        Request::post("/app/backup/retention")
            .body(Body::from(
                serde_json::to_vec(&json!({"hourly":"1","daily":2,"weekly":"3","monthly":4}))
                    .unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        crate::config::backup(&root_path).unwrap()["retention"],
        json!({"hourly":1,"daily":2,"weekly":3,"monthly":4})
    );
    let (status, _) = response_json(
        crate::routes(root_path.clone()),
        Request::post("/app/backup/offload/config")
            .body(Body::from(
                serde_json::to_vec(&json!({"budget_bytes":101,"floor_bytes":7})).unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        crate::config::backup(&root_path).unwrap()["offload"],
        json!({"enabled":false,"budget_bytes":101,"floor_bytes":7})
    );
}

#[test]
fn offload_defaults_cover_both_clamps() {
    for (total, expected) in [
        (
            1_000_000_000_000,
            json!({"budget_bytes":500_000_000_000u64,"floor_bytes":100_000_000_000u64}),
        ),
        (
            100_000_000_000,
            json!({"budget_bytes":50_000_000_000u64,"floor_bytes":20_000_000_000u64}),
        ),
        (
            40_000_000_000,
            json!({"budget_bytes":20_000_000_000u64,"floor_bytes":10_000_000_000u64}),
        ),
    ] {
        let cache = crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
            free_bytes: Some(total / 4),
            total_bytes: Some(total),
        });
        assert_eq!(
            crate::measurement::snapshot(&cache)["suggested_defaults"],
            expected
        );
    }
}

#[tokio::test]
async fn unreadable_and_zero_geometry_return_non_panicking_shapes() {
    for geometry in [
        crate::measurement::DeviceGeometry {
            free_bytes: None,
            total_bytes: None,
        },
        crate::measurement::DeviceGeometry {
            free_bytes: Some(7),
            total_bytes: Some(0),
        },
    ] {
        let root = crate::test_support::root("fresh");
        let (status, body) = response_json(
            crate::routes_with_cache(
                root.path().to_path_buf(),
                crate::measurement::with_geometry(geometry),
            ),
            Request::get("/app/backup/offload/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body["suggested_defaults"], Value::Null);
    }
}

#[tokio::test]
async fn engine_routes_are_no_longer_native_refusals() {
    let destination = json!({"repository":"s3:bucket","backend":"s3","credentials":{"access_key_id":"key","secret_access_key":"secret"}});
    let cases = [
        ("/app/backup/enable", None),
        ("/app/backup/enable-hosted", None),
        ("/app/backup/destination", Some(destination.clone())),
        ("/app/backup/recovery-key/rotate", None),
        ("/app/backup/teardown", None),
        (
            "/app/backup/restore",
            Some(
                json!({"recovery_key":crate::test_support::RECOVERY_KEY,"repository":"s3:bucket","backend":"s3","credentials":{"access_key_id":"key","secret_access_key":"secret"}}),
            ),
        ),
        (
            "/app/backup/restore-hosted",
            Some(json!({"recovery_key":crate::test_support::RECOVERY_KEY})),
        ),
        ("/app/backup/offload/restore", Some(json!({"all":true}))),
    ];
    let retired = [
        "backup_enable_not_implemented_native",
        "backup_enable_hosted_not_implemented_native",
        "backup_destination_not_implemented_native",
        "backup_recovery_key_rotate_not_implemented_native",
        "backup_teardown_not_implemented_native",
        "backup_restore_not_implemented_native",
        "backup_restore_hosted_not_implemented_native",
        "backup_offload_restore_not_implemented_native",
    ];
    for (path, body) in cases {
        let root = crate::test_support::root("healthy");
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let runner = ScriptRunner::with_outputs(vec![
            version_output(),
            output(10, ""),
            output(0, ""),
            output(0, ""),
            output(0, ""),
            output(0, "[{\"id\":\"snap\"}]"),
            output(0, ""),
            output(0, "[{\"paths\":[\"/original\"]}]"),
            output(0, "[{\"message_type\":\"summary\",\"bytes_restored\":1}]"),
            output(0, ""),
        ]);
        let deps = engine_deps(
            root.path().to_path_buf(),
            Arc::new(runner),
            Arc::new(HttpScript::default()),
            Some(restic.path().to_path_buf()),
        );
        let (status, response) = post_json(&deps, path, body).await;
        assert_ne!(status, 501, "{path}");
        let reason = response["reason_code"].as_str().unwrap_or("");
        assert!(!retired.contains(&reason), "{path} retired reason {reason}");
        if path == "/app/backup/enable-hosted" || path == "/app/backup/restore-hosted" {
            drain_hosted_wait(&deps).await;
        }
    }
}

#[tokio::test]
async fn reveal_and_disable_follow_their_source_read_contracts() {
    let root = crate::test_support::root("fresh");
    let root_path = root.path().to_path_buf();
    let (status, response) = response_json(
        crate::routes(root_path.clone()),
        Request::post("/app/backup/recovery-key/reveal")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["reason_code"], "invalid_operation_for_state");
    let _ = response_json(
        crate::routes(root_path.clone()),
        Request::post("/app/backup/keys/generate")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let (status, response) = response_json(
        crate::routes(root_path.clone()),
        Request::post("/app/backup/recovery-key/reveal")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert!(response["recovery_key_display"].as_str().is_some());

    crate::config::mutate(&root_path, |backup| {
        backup.insert(
            "offload".to_owned(),
            json!({"enabled":true,"budget_bytes":42,"floor_bytes":7}),
        );
        (true, ())
    })
    .unwrap();
    let (status, _) = response_json(
        crate::routes_with_cache(root_path.clone(), corpus_cache()),
        Request::post("/app/backup/offload/disable")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        crate::config::backup(&root_path).unwrap()["offload"],
        json!({"enabled":false,"budget_bytes":42,"floor_bytes":7})
    );
}

#[test]
fn journal_builder_matches_an_independent_python_healthy_capture() {
    // Captured from scripts/convey_backup_corpus.py::_build_journal, not derived
    // from the Rust builder. Retire/update only with an intentional oracle change.
    const PYTHON_HEALTHY: &str = r#"{
  "backup": {
    "confirmed_recovery_key": true,
    "daily_key": "corpus-daily-key",
    "destination": {
      "backend": "s3",
      "credentials": {
        "access_key_id": "CORPUSKEYID",
        "secret_access_key": "corpus-secret"
      },
      "repository": "s3:s3.example.invalid/journal-corpus"
    },
    "enabled": true,
    "last_backup": {
      "error_reason": null,
      "snapshot_id": "9f2c1ab4",
      "status": "ok",
      "time": 1770000000
    },
    "last_prune": {
      "error_reason": null,
      "status": "ok",
      "time": 1769996400
    },
    "last_verification": {
      "checked_subset": "5%",
      "last_ok_time": 1769990000,
      "reason": null,
      "status": "ok",
      "time": 1769990000
    },
    "mode": "byo",
    "offload": {
      "budget_bytes": null,
      "enabled": false,
      "floor_bytes": null
    },
    "recovery_key": "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ",
    "retention": {
      "daily": 7,
      "hourly": 24,
      "monthly": 12,
      "weekly": 4
    },
    "schedule": {
      "enabled": true,
      "every": "daily"
    }
  },
  "setup": {
    "completed_at": 1767225600
  }
}
"#;
    assert_eq!(
        crate::test_support::python_build_journal_bytes("healthy"),
        PYTHON_HEALTHY.as_bytes()
    );
    let root = crate::test_support::root("healthy");
    assert_eq!(
        fs::read(root.path().join("config/journal.json")).unwrap(),
        PYTHON_HEALTHY.as_bytes()
    );
}

#[test]
fn journal_builder_writes_every_phase_or_leaves_unestablished_absent() {
    let unestablished = crate::test_support::root("unestablished");
    assert!(!unestablished.path().join("config/journal.json").exists());
    // SHA-256 values captured directly from Python _build_journal for every
    // config-writing phase; unlike a Rust-to-Rust comparison these catch ordering drift.
    for (phase, expected_digest) in [
        (
            "corrupt",
            "d3570cdf2a87a076f8eebb4897334f413967eeb187b60819b8ceb89d157a46ed",
        ),
        (
            "fresh",
            "edf1a76b667652b1c1bcc1d8ca20b0032511896340c6214efd5b420a27ab1514",
        ),
        (
            "enabled_never_run",
            "4db2f5b06b769d30511f2a943888c6fa7d015efaaa58f1a0d0cb5ef725a13846",
        ),
        (
            "broken",
            "f237582a3c0b4bc91a9eb9ca31335d90a6a8ee0695418e166f8664b61ad7cbfb",
        ),
        (
            "healthy",
            "a10583c4037e019ff91b030499689f188957f477c96a0489cb0f1907abf9e21c",
        ),
    ] {
        let root = crate::test_support::root(phase);
        let written = fs::read(root.path().join("config/journal.json")).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&written)),
            expected_digest,
            "{phase}"
        );
    }
}

#[test]
fn corpus_declares_all_route_cases() {
    let corpus = crate::test_support::corpus();
    assert_eq!(
        corpus["phases"]
            .as_object()
            .expect("phases")
            .values()
            .map(|value| value.as_array().expect("cases").len())
            .sum::<usize>(),
        78
    );
}

struct Hold {
    released: Mutex<bool>,
    release: Condvar,
    started: Mutex<bool>,
    start: Condvar,
}

impl Hold {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            released: Mutex::new(false),
            release: Condvar::new(),
            started: Mutex::new(false),
            start: Condvar::new(),
        })
    }

    fn wait_started(&self) {
        let mut started = self.started.lock().unwrap();
        while !*started {
            started = self.start.wait(started).unwrap();
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.release.notify_all();
    }

    fn arrive_and_wait(&self) {
        *self.started.lock().unwrap() = true;
        self.start.notify_all();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.release.wait(released).unwrap();
        }
    }
}

struct ScriptRunner {
    outputs: Mutex<VecDeque<ToolOutput>>,
    calls: Mutex<Vec<(PathBuf, Vec<String>)>>,
    hold: Option<Arc<Hold>>,
}

impl ScriptRunner {
    fn with_outputs(outputs: Vec<ToolOutput>) -> Self {
        Self {
            outputs: Mutex::new(VecDeque::from(outputs)),
            calls: Mutex::new(vec![]),
            hold: None,
        }
    }

    fn with_hold(outputs: Vec<ToolOutput>, hold: Arc<Hold>) -> Self {
        Self {
            outputs: Mutex::new(VecDeque::from(outputs)),
            calls: Mutex::new(vec![]),
            hold: Some(hold),
        }
    }

    fn argv_heads(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, argv)| argv.first().cloned().unwrap_or_default())
            .collect()
    }
}

impl ToolRunner for ScriptRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        self.calls.lock().unwrap().push((
            PathBuf::from(&request.program),
            request
                .argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        ));
        if let Some(hold) = &self.hold {
            hold.arrive_and_wait();
        }
        Ok(self
            .outputs
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ToolOutput {
                returncode: 0,
                stdout: vec![],
                stderr: vec![],
            }))
    }
}

#[derive(Default)]
struct HttpScript {
    responses: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    poll_responses: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    requests: Mutex<Vec<HttpRequest>>,
    hold: Option<Arc<Hold>>,
    hold_after: usize,
    hold_skips: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

struct ActiveGuard<'a>(&'a AtomicUsize);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

impl HttpScript {
    fn with_responses(responses: Vec<Result<HttpResponse, HttpError>>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from(responses)),
            poll_responses: Mutex::new(VecDeque::new()),
            requests: Mutex::new(vec![]),
            hold: None,
            hold_after: 0,
            hold_skips: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }

    fn with_poll_responses(self, responses: Vec<Result<HttpResponse, HttpError>>) -> Self {
        *self.poll_responses.lock().unwrap() = VecDeque::from(responses);
        self
    }

    fn with_hold(self, hold: Arc<Hold>) -> Self {
        Self {
            hold: Some(hold),
            ..self
        }
    }

    fn with_hold_after(self, skip: usize, hold: Arc<Hold>) -> Self {
        Self {
            hold: Some(hold),
            hold_after: skip,
            ..self
        }
    }

    fn active_executions(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    fn max_concurrency(&self) -> usize {
        self.max_active.load(Ordering::Acquire)
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl HttpTransport for HttpScript {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.requests.lock().unwrap().push(request.clone());
        let now = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(now, Ordering::AcqRel);
        let _active = ActiveGuard(&self.active);
        if let Some(hold) = &self.hold {
            let seen = self.hold_skips.fetch_add(1, Ordering::Relaxed);
            if seen >= self.hold_after {
                hold.arrive_and_wait();
            }
        }
        if request.url.contains("/handoff/backup") {
            self.poll_responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(HttpResponse {
                    status: 204,
                    headers: vec![],
                    body: vec![],
                }))
        } else {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(HttpError::Unreachable))
        }
    }
}

struct PanicDownload;

impl ByteDownload for PanicDownload {
    fn fetch(&self, _: &str, _: Duration) -> Result<Vec<u8>, ByteDownloadError> {
        panic!("must not download")
    }
}

struct TestClock;

impl Clock for TestClock {
    fn now_unix(&self) -> i64 {
        50
    }
    fn iso_week(&self) -> u8 {
        1
    }
}

struct OkMaintenance;

impl JournalMaintenance for OkMaintenance {
    fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        Ok(())
    }
    fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        Ok(())
    }
}

fn output(code: i32, stdout: &str) -> ToolOutput {
    ToolOutput {
        returncode: code,
        stdout: stdout.as_bytes().to_vec(),
        stderr: vec![],
    }
}

fn version_output() -> ToolOutput {
    output(0, &format!("restic {RESTIC_VERSION}\n"))
}

fn credentials_response() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::to_vec(&json!({
            "access_key_id": "ACCESS",
            "secret_access_key": "SECRET",
            "session_token": "SESSION",
            "endpoint": "https://s3.example",
            "expires_at": "tomorrow"
        }))
        .unwrap(),
    }
}

fn xml_response(body: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![],
        body: body.as_bytes().to_vec(),
    }
}

fn engine_deps(
    journal_root: PathBuf,
    runner: Arc<dyn ToolRunner + Send + Sync>,
    http: Arc<dyn HttpTransport + Send + Sync>,
    restic_install_dir: Option<PathBuf>,
) -> crate::BackupWebDeps {
    crate::BackupWebDeps {
        journal_root,
        cache: corpus_cache(),
        operations: crate::operation::new_slot(),
        runner,
        http,
        downloader: Arc::new(PanicDownload),
        clock: Arc::new(TestClock),
        journal_maintenance: Arc::new(OkMaintenance),
        restic_install_dir,
        rclone_install_dir: None,
        portal_base: crate::test_support::PORTAL_BASE.into(),
        version: "test",
        handoff_poll_lease: Arc::new(AtomicBool::new(false)),
    }
}

fn prepared(
    journal_root: PathBuf,
    runner: Arc<dyn ToolRunner + Send + Sync>,
) -> (crate::BackupWebDeps, tempfile::TempDir) {
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    (
        engine_deps(
            journal_root,
            runner,
            Arc::new(HttpScript::default()),
            Some(restic.path().to_path_buf()),
        ),
        restic,
    )
}

async fn post_json(deps: &crate::BackupWebDeps, path: &str, body: Option<Value>) -> (u16, Value) {
    response_json(
        crate::routes_with_deps(deps.clone()),
        Request::post(path)
            .body(Body::from(
                body.map(|value| serde_json::to_vec(&value).unwrap())
                    .unwrap_or_default(),
            ))
            .unwrap(),
    )
    .await
}

async fn get_status_json(deps: &crate::BackupWebDeps) -> (u16, Value) {
    response_json(
        crate::routes_with_deps(deps.clone()),
        Request::get("/app/backup/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn wait_terminal(deps: &crate::BackupWebDeps) -> Value {
    for _ in 0..400 {
        let (_, body) = get_status_json(deps).await;
        let phase = body["operation"]["phase"].as_str().unwrap_or("");
        if crate::operation::is_terminal(phase) || body["operation"].is_null() {
            return body;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("operation did not terminate")
}

async fn drain_hosted_wait(deps: &crate::BackupWebDeps) {
    crate::operation::backdate_started(
        &deps.operations,
        crate::operation::HANDOFF_TTL + Duration::from_secs(1),
    );
    let _ = wait_terminal(deps).await;
}

fn disable_backup(root: &Path) {
    solstone_core_backup::set_enabled(root, false).unwrap();
}

fn restore_body() -> Value {
    json!({
        "recovery_key": crate::test_support::RECOVERY_KEY,
        "repository": "s3:bucket",
        "backend": "s3",
        "credentials": {"access_key_id": "key", "secret_access_key": "secret"}
    })
}

fn init_outputs() -> Vec<ToolOutput> {
    vec![
        version_output(),
        output(10, ""),
        output(0, ""),
        output(0, ""),
        output(0, ""),
    ]
}

fn restore_outputs() -> Vec<ToolOutput> {
    vec![
        version_output(),
        output(0, "[{\"paths\":[\"/original\"]}]"),
        output(0, "[{\"message_type\":\"summary\",\"bytes_restored\":12}]"),
        output(0, ""),
    ]
}

fn teardown_outputs() -> Vec<ToolOutput> {
    vec![
        version_output(),
        output(0, "[{\"id\":\"snap\"}]"),
        output(0, ""),
    ]
}

fn rotate_outputs() -> Vec<ToolOutput> {
    vec![
        version_output(),
        output(0, "[{\"id\":\"old\",\"current\":true}]"),
        output(0, ""),
        output(0, ""),
        output(0, ""),
    ]
}

#[tokio::test]
async fn engine_routes_return_running_while_restic_is_held() {
    struct Case {
        path: &'static str,
        body: Option<Value>,
        kind: &'static str,
        phase: &'static str,
        disable: bool,
        outputs: Vec<ToolOutput>,
    }
    let cases = [
        Case {
            path: "/app/backup/enable",
            body: None,
            kind: "enable",
            phase: "setting_up",
            disable: true,
            outputs: init_outputs(),
        },
        Case {
            path: "/app/backup/restore",
            body: Some(restore_body()),
            kind: "restore",
            phase: "restoring",
            disable: false,
            outputs: restore_outputs(),
        },
        Case {
            path: "/app/backup/teardown",
            body: None,
            kind: "teardown",
            phase: "tearing_down",
            disable: false,
            outputs: teardown_outputs(),
        },
        Case {
            path: "/app/backup/recovery-key/rotate",
            body: None,
            kind: "rotate",
            phase: "rotating",
            disable: false,
            outputs: rotate_outputs(),
        },
        Case {
            path: "/app/backup/offload/restore",
            body: Some(json!({"all": true})),
            kind: "offload_restore",
            phase: "restoring",
            disable: false,
            outputs: vec![version_output()],
        },
    ];
    for case in cases {
        let root = crate::test_support::root("healthy");
        if case.disable {
            disable_backup(root.path());
        }
        let hold = Hold::new();
        let runner = ScriptRunner::with_hold(case.outputs, hold.clone());
        let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
        let (status, body) = post_json(&deps, case.path, case.body).await;
        assert_eq!(status, 200, "{}", case.path);
        assert_eq!(body["operation"]["kind"], case.kind, "{}", case.path);
        assert_eq!(body["operation"]["phase"], case.phase, "{}", case.path);
        hold.wait_started();
        let (status, during) = get_status_json(&deps).await;
        assert_eq!(status, 200, "{}", case.path);
        assert_eq!(during["operation"]["phase"], case.phase, "{}", case.path);
        assert!(
            !crate::operation::is_terminal(during["operation"]["phase"].as_str().unwrap()),
            "{}",
            case.path
        );
        hold.release();
        let done = wait_terminal(&deps).await;
        assert!(
            crate::operation::is_terminal(done["operation"]["phase"].as_str().unwrap_or("")),
            "{} {}",
            case.path,
            done["operation"]["phase"]
        );
    }
}

#[tokio::test]
async fn enable_init_failure_leaves_disabled_and_errors() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let runner = ScriptRunner::with_outputs(vec![version_output(), output(10, ""), output(12, "")]);
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let (status, _) = post_json(&deps, "/app/backup/enable", None).await;
    assert_eq!(status, 200);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "auth_failed");
    assert_eq!(done["enabled"], false);
}

#[tokio::test]
async fn restore_success_records_snapshots_then_restore() {
    let root = crate::test_support::root("healthy");
    let before = fs::read(root.path().join("config/journal.json")).unwrap();
    let runner = ScriptRunner::with_outputs(vec![
        version_output(),
        output(0, "[{\"paths\":[\"/original\"]}]"),
        output(0, "[{\"message_type\":\"summary\",\"bytes_restored\":12}]"),
        output(0, ""),
    ]);
    let runner = Arc::new(runner);
    let (deps, _restic) = prepared(root.path().to_path_buf(), runner.clone());
    let (status, body) = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "restore");
    assert_eq!(body["operation"]["phase"], "restoring");
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    let heads = runner.argv_heads();
    assert!(heads.iter().any(|head| head == "snapshots"));
    assert!(heads.iter().any(|head| head == "restore"));
    assert_ne!(
        fs::read(root.path().join("config/journal.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn restore_empty_snapshots_errors_without_publishing_destination() {
    let root = crate::test_support::root("healthy");
    let before = fs::read(root.path().join("config/journal.json")).unwrap();
    let runner = ScriptRunner::with_outputs(vec![version_output(), output(0, "[]")]);
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let _ = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(
        fs::read(root.path().join("config/journal.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn restore_check_failure_is_degraded() {
    let root = crate::test_support::root("healthy");
    let runner = ScriptRunner::with_outputs(vec![
        version_output(),
        output(0, "[{\"paths\":[\"/original\"]}]"),
        output(0, "[{\"message_type\":\"summary\",\"bytes_restored\":12}]"),
        output(11, ""),
    ]);
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let _ = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "degraded");
    assert_eq!(done["operation"]["reason_code"], "integrity_unverified");
}

#[tokio::test]
async fn teardown_byo_forgets_then_clears() {
    let root = crate::test_support::root("healthy");
    let runner = ScriptRunner::with_outputs(vec![
        version_output(),
        output(0, "[{\"id\":\"snap\"}]"),
        output(0, ""),
    ]);
    let runner = Arc::new(runner);
    let (deps, _restic) = prepared(root.path().to_path_buf(), runner.clone());
    let (status, body) = post_json(&deps, "/app/backup/teardown", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "teardown");
    assert_eq!(body["operation"]["phase"], "tearing_down");
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["enabled"], false);
    let heads = runner.argv_heads();
    assert!(
        heads
            .iter()
            .any(|head| head == "forget" || head == "snapshots")
    );
}

#[tokio::test]
async fn teardown_operated_wipes_via_http() {
    let root = crate::test_support::hosted_bound_root();
    let runner = ScriptRunner::with_outputs(vec![version_output()]);
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = HttpScript::with_responses(vec![
        Ok(credentials_response()),
        Ok(xml_response(
            "<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>",
        )),
        Ok(xml_response(
            "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>",
        )),
    ]);
    let http = Arc::new(http);
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(runner),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/teardown", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert!(!http.requests.lock().unwrap().is_empty());
    assert_eq!(done["hosted"]["bound"], false);
}

#[tokio::test]
async fn rotate_does_not_echo_recovery_key() {
    let root = crate::test_support::root("healthy");
    let runner = ScriptRunner::with_outputs(vec![
        version_output(),
        output(0, "[{\"id\":\"old\",\"current\":true}]"),
        output(0, ""),
        output(0, ""),
        output(0, ""),
    ]);
    let runner = Arc::new(runner);
    let (deps, _restic) = prepared(root.path().to_path_buf(), runner.clone());
    let (status, body) = post_json(&deps, "/app/backup/recovery-key/rotate", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "rotate");
    assert!(body.get("recovery_key").is_none());
    assert!(body.get("daily_key").is_none());
    let rendered = body.to_string();
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
    hold_until_rotate_done(deps, runner).await;
}

async fn hold_until_rotate_done(deps: crate::BackupWebDeps, runner: Arc<ScriptRunner>) {
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["recovery_key_confirmed"], false);
    assert!(done.get("recovery_key").is_none());
    assert!(runner.argv_heads().iter().any(|head| head == "key"));
}

#[tokio::test]
async fn second_engine_post_is_backup_busy() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let hold = Hold::new();
    let runner = ScriptRunner::with_hold(init_outputs(), hold.clone());
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let (status, body) = post_json(&deps, "/app/backup/enable", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["phase"], "setting_up");
    let (busy_status, busy) = post_json(&deps, "/app/backup/teardown", None).await;
    assert_eq!(busy_status, 400);
    assert_eq!(busy["reason_code"], "backup_busy");
    hold.release();
    let _ = wait_terminal(&deps).await;
}

#[tokio::test]
async fn destination_returns_distinct_reason_codes_from_cat_config() {
    let destination = json!({"repository":"s3:bucket","backend":"s3","credentials":{"access_key_id":"key","secret_access_key":"secret"}});
    for (code, reason) in [(0, "repo_exists"), (12, "auth_failed"), (99, "unreachable")] {
        let root = crate::test_support::root("fresh");
        let runner = ScriptRunner::with_outputs(vec![version_output(), output(code, "")]);
        let runner = Arc::new(runner);
        let (deps, _restic) = prepared(root.path().to_path_buf(), runner.clone());
        let (status, body) =
            post_json(&deps, "/app/backup/destination", Some(destination.clone())).await;
        assert_eq!(status, 200, "{reason}");
        assert_eq!(body["destination_status"]["reason_code"], reason);
        let argv: Vec<Vec<String>> = runner
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, argv)| argv.clone())
            .collect();
        assert!(
            argv.iter()
                .any(|args| args.first().map(String::as_str) == Some("cat")
                    && args.get(1).map(String::as_str) == Some("config")),
            "{reason} {argv:?}"
        );
        assert!(
            argv.iter()
                .all(|args| args.first().map(String::as_str) != Some("snapshots")),
            "{reason}"
        );
    }
}

fn assert_enable_portal_url(url: &str) {
    let relative = url.strip_prefix("https://services.solstone.app").unwrap();
    let (path, query) = relative.split_once('?').unwrap();
    assert_eq!(path, "/enable/backup");
    let nonce = query
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let instance = query
        .split("instance=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    assert_eq!(nonce.len(), solstone_core_handoff_nonce::NONCE_LENGTH_CHARS);
    assert!(
        nonce
            .bytes()
            .all(|byte| { solstone_core_handoff_nonce::NONCE_ALPHABET.contains(&byte) })
    );
    assert_eq!(instance.len(), 32);
    assert!(
        instance
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    );
}

#[tokio::test]
async fn enable_hosted_returns_portal_url() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (status, body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "enable_hosted");
    assert_eq!(body["operation"]["phase"], "setting_up");
    let url = body["operation"]["portal_url"].as_str().unwrap();
    assert_enable_portal_url(url);
    drain_hosted_wait(&deps).await;
}

#[tokio::test]
async fn handoff_binding_enables_operated_mode() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let runner = ScriptRunner::with_outputs(init_outputs());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = HttpScript::with_responses(vec![Ok(credentials_response())]);
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(runner),
        Arc::new(http),
        Some(restic.path().to_path_buf()),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let url = started["operation"]["portal_url"].as_str().unwrap();
    let nonce = url
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let binding = crate::test_support::hosted_binding();
    let (status, _) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(json!({
            "nonce": nonce,
            "broker_endpoint": binding.broker_endpoint,
            "account_id": binding.account_id,
            "instance_id": binding.instance_id,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
            "broker_token": binding.broker_token
        })),
    )
    .await;
    assert_eq!(status, 200);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["hosted"]["bound"], true);
    assert_eq!(done["hosted"]["bucket"], "bucket");
    assert_eq!(done["hosted"]["prefix"], "owner/prefix");
    assert!(done["hosted"].get("broker_token").is_none());
    assert_eq!(done["mode"], "operated");
    assert_eq!(done["enabled"], true);
    let rendered = done.to_string();
    assert!(!rendered.contains("broker-token-secret"));
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
}

#[tokio::test]
async fn handoff_needs_subscription_is_terminal_without_binding() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let url = started["operation"]["portal_url"].as_str().unwrap();
    let nonce = url
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let _ = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_needs_subscription_payload(nonce)),
    )
    .await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "needs_subscription");
    assert_eq!(done["hosted"]["bound"], false);
}

#[tokio::test]
async fn bound_restore_hosted_maps_broker_402_to_entitlement_error() {
    let root = crate::test_support::hosted_bound_root();
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = HttpScript::with_responses(vec![Ok(HttpResponse {
        status: 402,
        headers: vec![],
        body: vec![],
    })]);
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        Arc::new(http),
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(
        &deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(
        done["operation"]["reason_code"],
        "hosted_entitlement_inactive"
    );
}

#[tokio::test]
async fn unbound_restore_hosted_returns_portal_and_restoring_phase() {
    let root = crate::test_support::root("fresh");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "restore_hosted");
    assert_eq!(body["operation"]["phase"], "restoring");
    assert_enable_portal_url(body["operation"]["portal_url"].as_str().unwrap());
    drain_hosted_wait(&deps).await;
}

#[tokio::test]
async fn restore_hosted_missing_key_is_still_400() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (status, body) = post_json(&deps, "/app/backup/restore-hosted", Some(json!({}))).await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "missing_required_field");
}

#[tokio::test]
async fn status_reports_hosted_binding_without_token() {
    let root = crate::test_support::hosted_bound_root();
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (status, body) = get_status_json(&deps).await;
    assert_eq!(status, 200);
    assert_eq!(body["hosted"]["bound"], true);
    assert_eq!(body["hosted"]["bucket"], "bucket");
    assert_eq!(body["hosted"]["prefix"], "owner/prefix");
    assert!(body["hosted"].get("broker_token").is_none());
}

#[tokio::test]
async fn offload_status_matches_builder_across_distinct_ledgers() {
    let first = crate::test_support::offload_inventory_root();
    let second = crate::test_support::root("healthy");
    let first_seg = second.path().join("chronicle/20260303/030000_001");
    fs::create_dir_all(&first_seg).unwrap();
    solstone_core_offload::append_offload_event(
        second.path(),
        "20260303",
        "_default",
        "030000_001",
        "snapshot-c",
        &[solstone_core_offload::OffloadFile {
            name: "only.webm".into(),
            bytes: 21,
            sha256: "c".repeat(64),
        }],
        3,
    )
    .unwrap();
    let degraded = crate::test_support::degraded_offload_root();
    let unreadable = crate::test_support::unreadable_offload_root();
    let mut totals = vec![];
    for journal in [
        first.path(),
        second.path(),
        degraded.path(),
        unreadable.path(),
    ] {
        let (deps, _restic) = prepared(
            journal.to_path_buf(),
            Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        );
        let (status, body) = response_json(
            crate::routes_with_deps(deps.clone()),
            Request::get("/app/backup/offload/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            body["device"],
            json!({"free_bytes":crate::test_support::DEVICE_FREE_BYTES,"total_bytes":crate::test_support::DEVICE_TOTAL_BYTES})
        );
        let built = solstone_core_offload::build_offload_status(journal)
            .unwrap()
            .value;
        totals.push((
            body["backup_only"]["total_days"].as_u64().unwrap(),
            body["backup_only"]["total_bytes"].as_u64().unwrap(),
            body["backup_only"]["degraded"].as_bool().unwrap(),
        ));
        assert_eq!(
            body["backup_only"]["total_bytes"],
            built["backup_only"]["total_bytes"]
        );
        assert_eq!(
            body["backup_only"]["total_days"],
            built["backup_only"]["total_days"]
        );
        assert_eq!(
            body["backup_only"]["degraded"],
            built["backup_only"]["degraded"]
        );
    }
    assert_ne!(totals[0].1, 0);
    assert_ne!(totals[1].1, 0);
    assert_ne!(totals[0], totals[1]);
    assert!(totals[2].2);
    let unreadable_status = solstone_core_offload::build_offload_status(unreadable.path())
        .unwrap()
        .value;
    assert_eq!(unreadable_status["backup_only"]["degraded"], true);
    assert!(
        !unreadable_status["backup_only"]["unreadable_ledgers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn offload_restore_digest_mismatch_is_verification_failed() {
    let root = crate::test_support::root("healthy");
    let segment = root.path().join("chronicle/20260101/010000_001");
    fs::create_dir_all(&segment).unwrap();
    solstone_core_offload::append_offload_event(
        root.path(),
        "20260101",
        "_default",
        "010000_001",
        "snapshot",
        &[solstone_core_offload::OffloadFile {
            name: "new.webm".into(),
            bytes: 8,
            sha256: "d".repeat(64),
        }],
        1,
    )
    .unwrap();
    let runner = RestoreMismatchRunner::default();
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let (status, body) = post_json(
        &deps,
        "/app/backup/offload/restore",
        Some(json!({"all": true})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "offload_restore");
    let done = wait_terminal(&deps).await;
    assert!(done["operation"]["phase"] == "error" || done["operation"]["phase"] == "degraded");
    assert_eq!(done["operation"]["reason_code"], "verification_failed");
}

#[derive(Default)]
struct RestoreMismatchRunner {
    calls: Mutex<Vec<Vec<String>>>,
}

impl ToolRunner for RestoreMismatchRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let argv = request
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.calls.lock().unwrap().push(argv.clone());
        if argv.first().map(String::as_str) == Some("version") {
            return Ok(version_output());
        }
        if argv.first().map(String::as_str) == Some("restore") {
            let target = argv
                .windows(2)
                .find(|pair| pair[0] == "--target")
                .map(|pair| PathBuf::from(&pair[1]))
                .expect("target");
            fs::write(target.join("new.webm"), b"corrupt").unwrap();
        }
        Ok(output(0, ""))
    }
}

#[tokio::test]
async fn enable_records_resolved_restic_program_not_path_decoy() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let restic = tempfile::tempdir().unwrap();
    let expected = crate::test_support::write_ready_restic(restic.path());
    let runner = ScriptRunner::with_outputs(init_outputs());
    let runner = Arc::new(runner);
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        Arc::new(HttpScript::default()),
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable", None).await;
    let _ = wait_terminal(&deps).await;
    let programs: Vec<PathBuf> = runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(program, _)| program.clone())
        .collect();
    assert!(
        programs.iter().any(|program| program == &expected),
        "{programs:?}"
    );
    assert!(
        programs.iter().all(
            |program| program.file_name() != Some(std::ffi::OsStr::new("restic"))
                || program == &expected
        ),
        "{programs:?}"
    );
}

#[tokio::test]
async fn enable_success_response_shape_has_operation_without_secrets() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(init_outputs())),
    );
    let (status, body) = post_json(&deps, "/app/backup/enable", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["success"], true);
    assert_eq!(body["operation"]["kind"], "enable");
    assert!(body["operation"]["phase"].as_str().is_some());
    assert!(body.get("recovery_key").is_none());
    assert!(body.get("daily_key").is_none());
    assert!(body.get("broker_token").is_none());
    let _ = wait_terminal(&deps).await;
}

#[tokio::test]
async fn offload_restore_day_digest_mismatch_is_verification_failed() {
    let root = crate::test_support::root("healthy");
    let segment = root.path().join("chronicle/20260101/010000_001");
    fs::create_dir_all(&segment).unwrap();
    solstone_core_offload::append_offload_event(
        root.path(),
        "20260101",
        "_default",
        "010000_001",
        "snapshot",
        &[solstone_core_offload::OffloadFile {
            name: "new.webm".into(),
            bytes: 8,
            sha256: "d".repeat(64),
        }],
        1,
    )
    .unwrap();
    let runner = RestoreMismatchRunner::default();
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let (status, body) = post_json(
        &deps,
        "/app/backup/offload/restore",
        Some(json!({"day": "20260101"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "offload_restore");
    let done = wait_terminal(&deps).await;
    assert!(done["operation"]["phase"] == "error" || done["operation"]["phase"] == "degraded");
    assert_eq!(done["operation"]["reason_code"], "verification_failed");
}

#[tokio::test]
async fn bound_restore_hosted_returns_running_then_done_while_held() {
    let root = crate::test_support::hosted_bound_root();
    let hold = Hold::new();
    let runner = ScriptRunner::with_hold(restore_outputs(), hold.clone());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(runner),
        Arc::new(HttpScript::with_responses(vec![Ok(credentials_response())])),
        Some(restic.path().to_path_buf()),
    );
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "restore_hosted");
    assert_eq!(body["operation"]["phase"], "restoring");
    hold.wait_started();
    let (status, during) = get_status_json(&deps).await;
    assert_eq!(status, 200);
    assert_eq!(during["operation"]["phase"], "restoring");
    hold.release();
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
}

#[tokio::test]
async fn expired_hosted_wait_clears_busy_on_status_and_begin() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (status, body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["phase"], "setting_up");
    crate::operation::backdate_started(
        &deps.operations,
        crate::operation::HANDOFF_TTL + Duration::from_secs(1),
    );
    let (status, expired) = get_status_json(&deps).await;
    assert_eq!(status, 200);
    assert_eq!(expired["operation"]["phase"], "error");
    assert_eq!(expired["operation"]["reason_code"], "expired");
    disable_backup(root.path());
    let (status, enable) = post_json(&deps, "/app/backup/enable", None).await;
    assert_eq!(status, 200);
    assert_eq!(enable["reason_code"], Value::Null);
    assert_eq!(enable["operation"]["kind"], "enable");
}

#[tokio::test]
async fn second_handoff_with_the_same_nonce_is_rejected() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let runner = ScriptRunner::with_outputs(init_outputs());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = HttpScript::with_responses(vec![Ok(credentials_response())]);
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(runner),
        Arc::new(http),
        Some(restic.path().to_path_buf()),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let url = started["operation"]["portal_url"].as_str().unwrap();
    let nonce = url
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let binding = crate::test_support::hosted_binding();
    let payload = json!({
        "nonce": nonce,
        "broker_endpoint": binding.broker_endpoint,
        "account_id": binding.account_id,
        "instance_id": binding.instance_id,
        "bucket": binding.bucket,
        "prefix": binding.prefix,
        "broker_token": binding.broker_token
    });
    let (first, _) = post_json(&deps, "/app/backup/handoff", Some(payload.clone())).await;
    assert_eq!(first, 200);
    let (second, body) = post_json(&deps, "/app/backup/handoff", Some(payload)).await;
    assert_eq!(second, 400);
    assert_eq!(body["reason_code"], "invalid_operation_for_state");
    let _ = wait_terminal(&deps).await;
}

fn hosted_handoff_payload(nonce: &str) -> Value {
    let binding = crate::test_support::hosted_binding();
    json!({
        "nonce": nonce,
        "broker_endpoint": binding.broker_endpoint,
        "account_id": binding.account_id,
        "instance_id": binding.instance_id,
        "bucket": binding.bucket,
        "prefix": binding.prefix,
        "broker_token": binding.broker_token
    })
}

fn hosted_needs_subscription_payload(nonce: &str) -> Value {
    let binding = crate::test_support::hosted_binding();
    json!({
        "nonce": nonce,
        "needs_subscription": true,
        "subscribe_url": format!("{}/services/backup", crate::test_support::PORTAL_BASE),
        "broker_endpoint": binding.broker_endpoint,
        "account_id": binding.account_id,
        "instance_id": binding.instance_id,
        "bucket": binding.bucket,
        "prefix": binding.prefix,
        "broker_token": binding.broker_token
    })
}

fn portal_nonce(body: &Value) -> String {
    body["operation"]["portal_url"]
        .as_str()
        .unwrap()
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned()
}

fn credentials_posts(http: &HttpScript) -> usize {
    http.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.url.contains("/backup/credentials"))
        .count()
}

fn exact_poll_url(nonce: &str) -> String {
    format!(
        "{}/handoff/backup?nonce={nonce}",
        crate::test_support::PORTAL_BASE
    )
}

fn argv_in_order(heads: &[String], expected: &[&str]) {
    let mut start = 0;
    for name in expected {
        let Some(offset) = heads[start..].iter().position(|head| head == *name) else {
            panic!("missing restic command in recorded sequence");
        };
        start += offset + 1;
    }
}

fn journal_contains(root: &Path, needle: &str) -> bool {
    fn walk(path: &Path, needle: &str) -> bool {
        if path.is_dir() {
            let Ok(entries) = fs::read_dir(path) else {
                return false;
            };
            return entries.flatten().any(|entry| walk(&entry.path(), needle));
        }
        fs::read(path)
            .ok()
            .is_some_and(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
    }
    walk(root, needle)
}

fn assert_json_hides_secrets(body: &Value, daily_key: Option<&str>) {
    let rendered = body.to_string();
    assert!(!rendered.contains("broker-token-secret"));
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
    assert!(!rendered.contains("ACCESS"));
    assert!(!rendered.contains("SECRET"));
    assert!(!rendered.contains("SESSION"));
    if let Some(daily_key) = daily_key {
        assert!(!rendered.contains(daily_key));
    }
}

fn assert_handoff_refused(status: u16, body: &Value) {
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_operation_for_state");
    assert_eq!(
        body["error"],
        "I couldn't take that action in the current state."
    );
    assert_eq!(body["detail"], "");
}

#[tokio::test]
async fn mismatched_nonces_are_refused_without_consuming_the_handoff() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let runner = ScriptRunner::with_outputs(init_outputs());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = HttpScript::with_responses(vec![Ok(credentials_response())]);
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(runner),
        Arc::new(http),
        Some(restic.path().to_path_buf()),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let url = started["operation"]["portal_url"].as_str().unwrap();
    let nonce = url
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();

    let original = nonce.as_bytes()[0];
    let flipped = solstone_core_handoff_nonce::NONCE_ALPHABET
        .iter()
        .copied()
        .find(|&byte| byte != original)
        .expect("alphabet has a second character");
    let mutated = {
        let mut bytes = nonce.clone().into_bytes();
        bytes[0] = flipped;
        String::from_utf8(bytes).unwrap()
    };
    assert_ne!(mutated, nonce);
    assert!(
        mutated
            .bytes()
            .all(|byte| solstone_core_handoff_nonce::NONCE_ALPHABET.contains(&byte))
    );

    let mut out_of_alphabet = nonce.clone().into_bytes();
    out_of_alphabet[0] = b'0';
    let out_of_alphabet = String::from_utf8(out_of_alphabet).unwrap();
    assert!(!solstone_core_handoff_nonce::NONCE_ALPHABET.contains(&b'0'));

    let (status, body) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_handoff_payload(&mutated)),
    )
    .await;
    assert_handoff_refused(status, &body);

    let (status, body) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_handoff_payload(&out_of_alphabet)),
    )
    .await;
    assert_handoff_refused(status, &body);

    let (status, _) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_handoff_payload(&nonce)),
    )
    .await;
    assert_eq!(status, 200);
    let _ = wait_terminal(&deps).await;
}

fn approved_poll_response() -> HttpResponse {
    let binding = crate::test_support::hosted_binding();
    HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::to_vec(&json!({
            "status": "approved",
            "broker_endpoint": binding.broker_endpoint,
            "account_id": binding.account_id,
            "instance_id": binding.instance_id,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
            "broker_token": binding.broker_token
        }))
        .unwrap(),
    }
}

fn needs_subscription_poll_body(subscribe_url: &str) -> HttpResponse {
    let binding = crate::test_support::hosted_binding();
    HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::to_vec(&json!({
            "status": "needs_subscription",
            "subscribe_url": subscribe_url,
            "broker_endpoint": binding.broker_endpoint,
            "account_id": binding.account_id,
            "instance_id": binding.instance_id,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
            "broker_token": binding.broker_token
        }))
        .unwrap(),
    }
}

fn wrong_origin_poll_response() -> HttpResponse {
    let binding = crate::test_support::hosted_binding_wrong_origin();
    HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::to_vec(&json!({
            "status": "approved",
            "broker_endpoint": binding.broker_endpoint,
            "account_id": binding.account_id,
            "instance_id": binding.instance_id,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
            "broker_token": binding.broker_token
        }))
        .unwrap(),
    }
}

fn hosted_handoff_payload_from(
    nonce: &str,
    binding: &solstone_core_backup::HostedBinding,
) -> Value {
    json!({
        "nonce": nonce,
        "broker_endpoint": binding.broker_endpoint,
        "account_id": binding.account_id,
        "instance_id": binding.instance_id,
        "bucket": binding.bucket,
        "prefix": binding.prefix,
        "broker_token": binding.broker_token
    })
}

fn poll_json(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: serde_json::to_vec(&body).unwrap(),
    }
}

async fn wait_poll_get(http: &HttpScript) -> HttpRequest {
    for _ in 0..200 {
        let request = http
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.url.contains("/handoff/backup"))
            .cloned();
        if let Some(request) = request {
            return request;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("poll GET not witnessed")
}

fn poll_gets(http: &HttpScript) -> Vec<HttpRequest> {
    http.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.url.contains("/handoff/backup"))
        .cloned()
        .collect()
}

async fn wait_poll_gets(http: &HttpScript, min: usize) -> Vec<HttpRequest> {
    for _ in 0..200 {
        let gets = poll_gets(http);
        if gets.len() >= min {
            return gets;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("expected at least {min} poll GETs")
}

#[tokio::test]
async fn hosted_poll_gets_handoff_backup_nonce_without_instance() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    let portal = body["operation"]["portal_url"].as_str().unwrap();
    let nonce = portal
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let request = wait_poll_get(&http).await;
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.url,
        format!("https://services.solstone.app/handoff/backup?nonce={nonce}")
    );
    assert!(!request.url.contains("instance"));
    assert!(!request.url.contains("/enable/backup"));
    drain_hosted_wait(&deps).await;
}

#[tokio::test]
async fn hosted_poll_approved_enables_operated_mode() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(init_outputs()));
    let http = Arc::new(
        HttpScript::with_responses(vec![Ok(credentials_response())]).with_poll_responses(vec![
            Ok(HttpResponse {
                status: 204,
                headers: vec![],
                body: vec![],
            }),
            Ok(approved_poll_response()),
        ]),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    let nonce = portal_nonce(&started);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["hosted"]["bound"], true);
    assert_eq!(done["hosted"]["bucket"], "bucket");
    assert_eq!(done["hosted"]["prefix"], "owner/prefix");
    assert!(done["hosted"].get("broker_token").is_none());
    assert_eq!(done["mode"], "operated");
    assert_eq!(done["enabled"], true);
    let rendered = done.to_string();
    assert!(!rendered.contains("broker-token-secret"));
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
    let expected_poll = exact_poll_url(&nonce);
    let gets = poll_gets(&http);
    assert!(!gets.is_empty());
    assert!(
        gets.iter()
            .all(|request| request.method == "GET" && request.url == expected_poll)
    );
    assert_eq!(credentials_posts(&http), 1);
    argv_in_order(&runner.argv_heads(), &["version", "cat", "init", "key"]);
}

#[tokio::test]
async fn hosted_poll_needs_subscription_is_terminal_without_binding() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default().with_poll_responses(vec![Ok(
        needs_subscription_poll_body("https://services.solstone.app/services/backup"),
    )]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "needs_subscription");
    assert_eq!(done["hosted"]["bound"], false);
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
}

#[tokio::test]
async fn hosted_poll_subscribe_url_and_secrets_stay_off_status() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let subscribe = "https://services.solstone.app/services/backup/subscribe-ac4";
    let http = Arc::new(
        HttpScript::default()
            .with_poll_responses(vec![Ok(needs_subscription_poll_body(subscribe))]),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    let rendered = done.to_string();
    assert!(!rendered.contains(subscribe));
    assert!(!rendered.contains("subscribe_url"));
    assert!(!rendered.contains("broker-token-secret"));
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
    assert!(done.get("subscribe_url").is_none());
    assert!(done["operation"].get("subscribe_url").is_none());
}

#[tokio::test]
async fn hosted_poll_response_classes_retry_or_fail() {
    struct Case {
        name: String,
        poll: Vec<Result<HttpResponse, HttpError>>,
        credentials: bool,
        init: bool,
        phase: &'static str,
        reason: Option<&'static str>,
    }
    let approved = approved_poll_response();
    let mut cases = vec![
        Case {
            name: "204 then approved".into(),
            poll: vec![
                Ok(HttpResponse {
                    status: 204,
                    headers: vec![],
                    body: vec![],
                }),
                Ok(approved.clone()),
            ],
            credentials: true,
            init: true,
            phase: "done",
            reason: None,
        },
        Case {
            name: "timeout then approved".into(),
            poll: vec![Err(HttpError::Timeout), Ok(approved.clone())],
            credentials: true,
            init: true,
            phase: "done",
            reason: None,
        },
        Case {
            name: "unreachable".into(),
            poll: vec![Err(HttpError::Unreachable)],
            credentials: false,
            init: false,
            phase: "error",
            reason: Some("unreachable"),
        },
        Case {
            name: "http 410".into(),
            poll: vec![Ok(HttpResponse {
                status: 410,
                headers: vec![],
                body: vec![],
            })],
            credentials: false,
            init: false,
            phase: "error",
            reason: Some("expired"),
        },
        Case {
            name: "http 400".into(),
            poll: vec![Ok(HttpResponse {
                status: 400,
                headers: vec![],
                body: vec![],
            })],
            credentials: false,
            init: false,
            phase: "error",
            reason: Some("failed"),
        },
        Case {
            name: "http 500".into(),
            poll: vec![Ok(HttpResponse {
                status: 500,
                headers: vec![],
                body: vec![],
            })],
            credentials: false,
            init: false,
            phase: "error",
            reason: Some("failed"),
        },
        Case {
            name: "malformed json".into(),
            poll: vec![Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: b"{".to_vec(),
            })],
            credentials: false,
            init: false,
            phase: "error",
            reason: Some("failed"),
        },
        Case {
            name: "unknown status".into(),
            poll: vec![Ok(poll_json(200, json!({"status": "pending"})))],
            credentials: false,
            init: false,
            phase: "error",
            reason: Some("failed"),
        },
    ];
    let binding = crate::test_support::hosted_binding();
    for status in ["approved", "needs_subscription"] {
        for field in [
            "broker_endpoint",
            "account_id",
            "instance_id",
            "bucket",
            "prefix",
            "broker_token",
        ] {
            let mut body = json!({
                "status": status,
                "subscribe_url": format!("{}/services/backup", crate::test_support::PORTAL_BASE),
                "broker_endpoint": binding.broker_endpoint,
                "account_id": binding.account_id,
                "instance_id": binding.instance_id,
                "bucket": binding.bucket,
                "prefix": binding.prefix,
                "broker_token": binding.broker_token
            });
            let object = body.as_object_mut().unwrap();
            object.remove(field);
            if status == "approved" {
                object.remove("subscribe_url");
            }
            cases.push(Case {
                name: format!("{status} missing {field}"),
                poll: vec![Ok(poll_json(200, body))],
                credentials: false,
                init: false,
                phase: "error",
                reason: Some("failed"),
            });
        }
    }
    for case in cases {
        let root = crate::test_support::root("healthy");
        if case.init {
            disable_backup(root.path());
        }
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let http = HttpScript::with_responses(if case.credentials {
            vec![Ok(credentials_response())]
        } else {
            vec![]
        })
        .with_poll_responses(case.poll);
        let runner = if case.init {
            ScriptRunner::with_outputs(init_outputs())
        } else {
            ScriptRunner::with_outputs(vec![version_output()])
        };
        let deps = engine_deps(
            root.path().to_path_buf(),
            Arc::new(runner),
            Arc::new(http),
            Some(restic.path().to_path_buf()),
        );
        let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
        let done = wait_terminal(&deps).await;
        assert_eq!(done["operation"]["phase"], case.phase, "{}", case.name);
        match case.reason {
            Some(reason) => assert_eq!(done["operation"]["reason_code"], reason, "{}", case.name),
            None => assert!(done["operation"]["reason_code"].is_null(), "{}", case.name),
        }
    }
}

#[tokio::test]
async fn hosted_poll_watchdog_expires_while_get_is_held() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let hold = Hold::new();
    let http = Arc::new(HttpScript::default().with_hold(hold.clone()));
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, _) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    hold.wait_started();
    assert!(
        http.requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.url.contains("/handoff/backup"))
    );
    crate::operation::backdate_started(
        &deps.operations,
        crate::operation::HANDOFF_TTL + Duration::from_secs(1),
    );
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "expired");
    hold.release();
}

#[tokio::test]
async fn hosted_poll_needs_subscription_rejects_non_https_subscribe_url() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default().with_poll_responses(vec![Ok(
        needs_subscription_poll_body("http://services.solstone.app/services/backup"),
    )]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert_eq!(done["hosted"]["bound"], false);
}

#[tokio::test]
async fn hosted_poll_restore_uses_slot_recovery_key() {
    let root = crate::test_support::root("fresh");
    let before = fs::read(root.path().join("config/journal.json")).unwrap();
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(restore_outputs()));
    let http = Arc::new(
        HttpScript::with_responses(vec![Ok(credentials_response())])
            .with_poll_responses(vec![Ok(approved_poll_response())]),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, started) = post_json(
        &deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    assert_eq!(status, 200);
    let nonce = portal_nonce(&started);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["mode"], "operated");
    assert_ne!(
        fs::read(root.path().join("config/journal.json")).unwrap(),
        before
    );
    assert_eq!(done["recovery_key_confirmed"], true);
    assert!(done["destination"]["repository"].as_str().is_some());
    let rendered = done.to_string();
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
    assert!(!rendered.contains("broker-token-secret"));
    assert!(!rendered.contains("ACCESS"));
    assert!(!rendered.contains("SECRET"));
    assert!(!rendered.contains("SESSION"));
    let expected_poll = exact_poll_url(&nonce);
    let gets = poll_gets(&http);
    assert!(!gets.is_empty());
    assert!(
        gets.iter()
            .all(|request| request.method == "GET" && request.url == expected_poll)
    );
    assert_eq!(credentials_posts(&http), 1);
    argv_in_order(&runner.argv_heads(), &["snapshots", "restore", "check"]);
}

#[tokio::test]
async fn hosted_poll_restore_failure_publishes_no_destination_or_recovery_state() {
    let root = crate::test_support::root("fresh");
    let before = fs::read(root.path().join("config/journal.json")).unwrap();
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(
        HttpScript::with_responses(vec![Ok(credentials_response())])
            .with_poll_responses(vec![Ok(approved_poll_response())]),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![
            version_output(),
            output(0, "[{\"paths\":[\"/original\"]}]"),
            output(1, ""),
        ])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(
        &deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_ne!(done["operation"]["phase"], "degraded");
    assert_eq!(
        fs::read(root.path().join("config/journal.json")).unwrap(),
        before
    );
    assert_ne!(done["mode"], "operated");
}

#[tokio::test]
async fn hosted_poll_stale_generation_cannot_persist_binding() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let hold = Hold::new();
    let http = Arc::new(
        HttpScript::default()
            .with_poll_responses(vec![Ok(approved_poll_response())])
            .with_hold(hold.clone()),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    hold.wait_started();
    crate::operation::backdate_started(
        &deps.operations,
        crate::operation::HANDOFF_TTL + Duration::from_secs(1),
    );
    let expired = wait_terminal(&deps).await;
    assert_eq!(expired["operation"]["phase"], "error");
    assert_eq!(expired["operation"]["reason_code"], "expired");
    hold.release();
    let (status, second) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert_eq!(second["operation"]["kind"], "enable_hosted");
    assert_eq!(second["operation"]["phase"], "setting_up");
    assert_eq!(second["hosted"]["bound"], false);
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
    drain_hosted_wait(&deps).await;
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
}

#[tokio::test]
async fn hosted_poll_second_local_handoff_after_poll_approval_is_rejected() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(init_outputs()));
    let http = Arc::new(
        HttpScript::with_responses(vec![Ok(credentials_response())])
            .with_poll_responses(vec![Ok(approved_poll_response())]),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    let nonce = started["operation"]["portal_url"]
        .as_str()
        .unwrap()
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["hosted"]["bound"], true);
    let bound_bucket = done["hosted"]["bucket"].clone();
    let bound_prefix = done["hosted"]["prefix"].clone();
    let restic_calls = runner.calls.lock().unwrap().len();
    let broker_posts = http
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.url.contains("/backup/credentials"))
        .count();
    assert_eq!(broker_posts, 1);
    let original = crate::test_support::hosted_binding();
    let (status, body) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(json!({
            "nonce": nonce,
            "broker_endpoint": original.broker_endpoint,
            "account_id": original.account_id,
            "instance_id": original.instance_id,
            "bucket": "other-bucket",
            "prefix": "other/prefix",
            "broker_token": "other-token"
        })),
    )
    .await;
    assert_handoff_refused(status, &body);
    let persisted = solstone_core_backup::load_hosted_binding(root.path()).expect("binding");
    assert_eq!(persisted, original);
    assert_eq!(persisted.bucket, bound_bucket.as_str().unwrap());
    assert_eq!(persisted.prefix, bound_prefix.as_str().unwrap());
    assert_ne!(persisted.bucket, "other-bucket");
    assert_eq!(runner.calls.lock().unwrap().len(), restic_calls);
    assert_eq!(
        http.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.url.contains("/backup/credentials"))
            .count(),
        1
    );
    let (_, after) = get_status_json(&deps).await;
    assert_eq!(after["operation"]["phase"], "done");
    assert_eq!(after["hosted"]["bucket"], bound_bucket);
    assert_eq!(after["hosted"]["prefix"], bound_prefix);
}

#[tokio::test]
async fn hosted_poll_get_timeout_is_bounded_by_remaining_ttl() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, _) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    let first = wait_poll_get(&http).await;
    assert_eq!(first.timeout, crate::handoff_poll::HANDOFF_POLL_TIMEOUT);
    crate::operation::backdate_started(
        &deps.operations,
        crate::operation::HANDOFF_TTL - Duration::from_secs(3) + Duration::from_millis(100),
    );
    let bounded = loop {
        if let Some(request) = poll_gets(&http)
            .into_iter()
            .find(|request| request.timeout <= Duration::from_secs(3))
        {
            break request;
        }
        wait_poll_gets(&http, poll_gets(&http).len() + 1).await;
    };
    assert!(bounded.timeout < crate::handoff_poll::HANDOFF_POLL_TIMEOUT);
    let _ = wait_terminal(&deps).await;
}

#[tokio::test]
async fn hosted_poll_broker_unreachable_matches_local_handoff_broker_unreachable_contract() {
    async fn run(
        poll_approved: bool,
        handoff_locally: bool,
    ) -> (Value, Option<solstone_core_backup::HostedBinding>) {
        let root = crate::test_support::root("healthy");
        disable_backup(root.path());
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let mut script = HttpScript::with_responses(vec![Err(HttpError::Unreachable)]);
        if poll_approved {
            script = script.with_poll_responses(vec![Ok(approved_poll_response())]);
        }
        let deps = engine_deps(
            root.path().to_path_buf(),
            Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
            Arc::new(script),
            Some(restic.path().to_path_buf()),
        );
        let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 200);
        if handoff_locally {
            let nonce = started["operation"]["portal_url"]
                .as_str()
                .unwrap()
                .split("nonce=")
                .nth(1)
                .unwrap()
                .split('&')
                .next()
                .unwrap()
                .to_owned();
            let (status, _) = post_json(
                &deps,
                "/app/backup/handoff",
                Some(hosted_handoff_payload(&nonce)),
            )
            .await;
            assert_eq!(status, 200);
        }
        let done = wait_terminal(&deps).await;
        let binding = solstone_core_backup::load_hosted_binding(root.path());
        (done, binding)
    }

    let (poll, poll_binding) = run(true, false).await;
    let (local, local_binding) = run(false, true).await;
    for (label, body, binding) in [
        ("poll", &poll, poll_binding),
        ("local", &local, local_binding),
    ] {
        assert_eq!(body["operation"]["phase"], "error", "{label}");
        assert_eq!(
            body["operation"]["reason_code"], "broker_unreachable",
            "{label}"
        );
        assert_eq!(
            binding,
            Some(crate::test_support::hosted_binding()),
            "{label} persists binding before the broker call"
        );
        assert_ne!(body["mode"], "operated", "{label}");
        assert_eq!(body["enabled"], false, "{label}");
        assert_eq!(body["hosted"]["bound"], true, "{label}");
        assert!(!body.to_string().contains("broker-token-secret"), "{label}");
    }
    assert_eq!(poll["operation"]["phase"], local["operation"]["phase"]);
    assert_eq!(
        poll["operation"]["reason_code"],
        local["operation"]["reason_code"]
    );
}

fn wait_watchdog_expired(slot: &crate::operation::SharedOperationSlot) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        {
            let guard = slot.lock().expect("operation slot lock");
            if let Some(current) = guard.as_ref()
                && current.view.phase == "error"
                && current.view.reason_code.as_deref() == Some("expired")
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("watchdog did not expire the slot");
}

fn wait_lease_released(deps: &crate::BackupWebDeps, http: &HttpScript) {
    for _ in 0..400 {
        if http.active_executions() == 0 && !deps.handoff_poll_lease.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("poll lease did not release");
}

#[tokio::test]
async fn hosted_poll_watchdog_expiry_is_proven_by_direct_slot_lock_not_observation() {
    let root = crate::test_support::root("fresh");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let hold = Hold::new();
    let http = Arc::new(
        HttpScript::default()
            .with_poll_responses(vec![Ok(approved_poll_response())])
            .with_hold(hold.clone()),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, _) = post_json(
        &deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    assert_eq!(status, 200);
    hold.wait_started();
    {
        let guard = deps.operations.lock().expect("operation slot lock");
        let slot = guard.as_ref().expect("slot");
        assert!(slot.view.portal_url.is_some());
        assert!(slot.nonce.is_some());
        assert!(slot.restore_key.is_some());
        assert!(!crate::operation::is_terminal(&slot.view.phase));
    }
    crate::operation::backdate_started(
        &deps.operations,
        crate::operation::HANDOFF_TTL + Duration::from_secs(1),
    );
    wait_watchdog_expired(&deps.operations);
    {
        let guard = deps.operations.lock().expect("operation slot lock");
        let slot = guard.as_ref().expect("slot");
        assert_eq!(slot.view.phase, "error");
        assert_eq!(slot.view.reason_code.as_deref(), Some("expired"));
        assert!(crate::operation::is_terminal(&slot.view.phase));
        assert!(slot.view.portal_url.is_none());
        assert!(slot.nonce.is_none());
        assert!(slot.restore_key.is_none());
    }
    assert_eq!(http.active_executions(), 1);
    hold.release();
    wait_lease_released(&deps, &http);
    {
        let guard = deps.operations.lock().expect("operation slot lock");
        let slot = guard.as_ref().expect("slot");
        assert_eq!(slot.view.phase, "error");
        assert_eq!(slot.view.reason_code.as_deref(), Some("expired"));
        assert!(slot.view.portal_url.is_none());
        assert!(slot.nonce.is_none());
        assert!(slot.restore_key.is_none());
    }
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
}

#[tokio::test]
async fn handoff_needs_subscription_missing_field_is_failed() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let nonce = portal_nonce(&started);
    let mut payload = hosted_needs_subscription_payload(&nonce);
    payload.as_object_mut().unwrap().remove("bucket");
    let (status, body) = post_json(&deps, "/app/backup/handoff", Some(payload)).await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "missing_required_field");
    let (_, done) = get_status_json(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
}

#[tokio::test]
async fn hosted_poll_approved_wrong_origin_is_failed() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http =
        Arc::new(HttpScript::default().with_poll_responses(vec![Ok(wrong_origin_poll_response())]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
    assert_eq!(credentials_posts(&http), 0);
}

#[tokio::test]
async fn handoff_approved_wrong_origin_is_rejected() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(vec![version_output()]));
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let nonce = portal_nonce(&started);
    let (status, body) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_handoff_payload_from(
            &nonce,
            &crate::test_support::hosted_binding_wrong_origin(),
        )),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
    assert_json_hides_secrets(&body, None);
    let (_, done) = get_status_json(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
    assert_eq!(credentials_posts(&http), 0);
    assert_eq!(runner.calls.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn hosted_poll_wrong_origin_matches_local_wrong_origin_contract() {
    async fn run(
        poll_approved: bool,
        handoff_locally: bool,
    ) -> (Value, Option<solstone_core_backup::HostedBinding>, usize) {
        let root = crate::test_support::root("healthy");
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let mut script = HttpScript::with_responses(vec![]);
        if poll_approved {
            script = script.with_poll_responses(vec![Ok(wrong_origin_poll_response())]);
        }
        let http = Arc::new(script);
        let deps = engine_deps(
            root.path().to_path_buf(),
            Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
            http.clone(),
            Some(restic.path().to_path_buf()),
        );
        let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 200);
        if handoff_locally {
            let nonce = portal_nonce(&started);
            let (status, _) = post_json(
                &deps,
                "/app/backup/handoff",
                Some(hosted_handoff_payload_from(
                    &nonce,
                    &crate::test_support::hosted_binding_wrong_origin(),
                )),
            )
            .await;
            assert_eq!(status, 400);
        }
        let done = wait_terminal(&deps).await;
        let binding = solstone_core_backup::load_hosted_binding(root.path());
        (done, binding, credentials_posts(&http))
    }

    let (poll, poll_binding, poll_creds) = run(true, false).await;
    let (local, local_binding, local_creds) = run(false, true).await;
    for (label, body, binding, creds) in [
        ("poll", &poll, poll_binding, poll_creds),
        ("local", &local, local_binding, local_creds),
    ] {
        assert_eq!(body["operation"]["phase"], "error", "{label}");
        assert_eq!(body["operation"]["reason_code"], "failed", "{label}");
        assert_eq!(binding, None, "{label}");
        assert_eq!(creds, 0, "{label}");
        assert!(!body.to_string().contains("broker-token-secret"), "{label}");
    }
}

#[tokio::test]
async fn hosted_poll_needs_subscription_rejects_wrong_origin_subscribe_url() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default().with_poll_responses(vec![Ok(
        needs_subscription_poll_body("https://evil.example/subscribe"),
    )]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert_eq!(done["hosted"]["bound"], false);
}

#[tokio::test]
async fn hosted_poll_needs_subscription_rejects_userinfo_subscribe_url() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default().with_poll_responses(vec![Ok(
        needs_subscription_poll_body("https://user:pass@services.solstone.app/services/backup"),
    )]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
}

#[tokio::test]
async fn handoff_needs_subscription_rejects_non_https_subscribe_url() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let nonce = portal_nonce(&started);
    let mut payload = hosted_needs_subscription_payload(&nonce);
    payload["subscribe_url"] = json!("http://services.solstone.app/services/backup");
    let (status, body) = post_json(&deps, "/app/backup/handoff", Some(payload)).await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
    let (_, done) = get_status_json(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
}

#[tokio::test]
async fn hosted_poll_needs_subscription_matches_local_needs_subscription_contract() {
    async fn run(poll_approved: bool, handoff_locally: bool) -> Value {
        let root = crate::test_support::root("healthy");
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let mut script = HttpScript::default();
        if poll_approved {
            script = script.with_poll_responses(vec![Ok(needs_subscription_poll_body(&format!(
                "{}/services/backup",
                crate::test_support::PORTAL_BASE
            )))]);
        }
        let deps = engine_deps(
            root.path().to_path_buf(),
            Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
            Arc::new(script),
            Some(restic.path().to_path_buf()),
        );
        let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 200);
        if handoff_locally {
            let nonce = portal_nonce(&started);
            let (status, _) = post_json(
                &deps,
                "/app/backup/handoff",
                Some(hosted_needs_subscription_payload(&nonce)),
            )
            .await;
            assert_eq!(status, 200);
        }
        let done = wait_terminal(&deps).await;
        assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
        assert!(!crate::operation::is_busy(&deps.operations));
        done
    }

    let poll = run(true, false).await;
    let local = run(false, true).await;
    for (label, body) in [("poll", &poll), ("local", &local)] {
        assert_eq!(body["operation"]["phase"], "needs_subscription", "{label}");
        assert_eq!(body["hosted"]["bound"], false, "{label}");
        assert!(!body.to_string().contains("subscribe_url"), "{label}");
        assert!(!body.to_string().contains("broker-token-secret"), "{label}");
    }
}

#[tokio::test]
async fn hosted_poll_second_get_is_resource_bounded_and_lease_releases_on_completion() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let hold = Hold::new();
    let http = Arc::new(
        HttpScript::default()
            .with_poll_responses(vec![
                Ok(HttpResponse {
                    status: 204,
                    headers: vec![],
                    body: vec![],
                }),
                Ok(approved_poll_response()),
            ])
            .with_hold_after(1, hold.clone()),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![version_output()])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, _) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    hold.wait_started();
    assert_eq!(http.active_executions(), 1);
    assert_eq!(http.max_concurrency(), 1);
    assert_eq!(http.request_count(), 2);
    assert_eq!(poll_gets(&http).len(), 2);

    crate::operation::backdate_started(
        &deps.operations,
        crate::operation::HANDOFF_TTL + Duration::from_secs(1),
    );
    wait_watchdog_expired(&deps.operations);
    assert_eq!(http.active_executions(), 1);
    assert_eq!(http.request_count(), 2);

    let (status, second) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert_eq!(second["operation"]["phase"], "error");
    assert_eq!(second["operation"]["reason_code"], "failed");
    assert_eq!(http.active_executions(), 1);
    assert_eq!(http.max_concurrency(), 1);
    assert_eq!(http.request_count(), 2);
    let second_generation = {
        let guard = deps.operations.lock().expect("operation slot lock");
        let slot = guard.as_ref().expect("slot");
        assert_eq!(slot.view.phase, "error");
        assert_eq!(slot.view.reason_code.as_deref(), Some("failed"));
        slot.generation
    };

    hold.release();
    wait_lease_released(&deps, &http);
    assert_eq!(http.active_executions(), 0);
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
    {
        let guard = deps.operations.lock().expect("operation slot lock");
        let slot = guard.as_ref().expect("slot");
        assert_eq!(slot.generation, second_generation);
        assert_eq!(slot.view.phase, "error");
        assert_eq!(slot.view.reason_code.as_deref(), Some("failed"));
    }

    let (status, third) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert_eq!(third["operation"]["kind"], "enable_hosted");
    assert_eq!(third["operation"]["phase"], "setting_up");
    let _ = wait_poll_gets(&http, 3).await;
    assert_eq!(http.request_count(), 3);
    drain_hosted_wait(&deps).await;
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
}

#[tokio::test]
async fn hosted_poll_binding_write_failure_matches_local_handoff_contract() {
    async fn run(
        poll_approved: bool,
        handoff_locally: bool,
    ) -> (
        Value,
        Option<solstone_core_backup::HostedBinding>,
        usize,
        usize,
    ) {
        let root = crate::test_support::root("healthy");
        disable_backup(root.path());
        fs::create_dir_all(root.path().join("backup/hosted/binding.json")).unwrap();
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let runner = Arc::new(ScriptRunner::with_outputs(vec![version_output()]));
        let mut script = HttpScript::with_responses(vec![]);
        if poll_approved {
            script = script.with_poll_responses(vec![Ok(approved_poll_response())]);
        }
        let http = Arc::new(script);
        let deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            Some(restic.path().to_path_buf()),
        );
        let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 200);
        if handoff_locally {
            let nonce = portal_nonce(&started);
            let (status, _) = post_json(
                &deps,
                "/app/backup/handoff",
                Some(hosted_handoff_payload(&nonce)),
            )
            .await;
            assert_eq!(status, 500);
        }
        let done = wait_terminal(&deps).await;
        assert!(!crate::operation::is_busy(&deps.operations));
        (
            done,
            solstone_core_backup::load_hosted_binding(root.path()),
            credentials_posts(&http),
            runner.calls.lock().unwrap().len(),
        )
    }

    let (poll, poll_binding, poll_creds, poll_restic) = run(true, false).await;
    let (local, local_binding, local_creds, local_restic) = run(false, true).await;
    for (label, body, binding, creds, restic) in [
        ("poll", &poll, poll_binding, poll_creds, poll_restic),
        ("local", &local, local_binding, local_creds, local_restic),
    ] {
        assert_eq!(body["operation"]["phase"], "error", "{label}");
        assert_eq!(body["operation"]["reason_code"], "failed", "{label}");
        assert_eq!(binding, None, "{label}");
        assert_eq!(creds, 0, "{label}");
        assert_eq!(restic, 0, "{label}");
    }
    assert_eq!(poll["operation"]["phase"], local["operation"]["phase"]);
    assert_eq!(
        poll["operation"]["reason_code"],
        local["operation"]["reason_code"]
    );
}

#[tokio::test]
async fn hosted_poll_repository_init_failure_matches_local_handoff_contract() {
    async fn run(
        poll_approved: bool,
        handoff_locally: bool,
    ) -> (
        Value,
        Option<solstone_core_backup::HostedBinding>,
        usize,
        usize,
    ) {
        let root = crate::test_support::root("healthy");
        disable_backup(root.path());
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let runner = Arc::new(ScriptRunner::with_outputs(vec![
            version_output(),
            output(10, ""),
            output(12, ""),
        ]));
        let mut script = HttpScript::with_responses(vec![Ok(credentials_response())]);
        if poll_approved {
            script = script.with_poll_responses(vec![Ok(approved_poll_response())]);
        }
        let http = Arc::new(script);
        let deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            Some(restic.path().to_path_buf()),
        );
        let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 200);
        if handoff_locally {
            let nonce = portal_nonce(&started);
            let (status, _) = post_json(
                &deps,
                "/app/backup/handoff",
                Some(hosted_handoff_payload(&nonce)),
            )
            .await;
            assert_eq!(status, 200);
        }
        let done = wait_terminal(&deps).await;
        assert!(!crate::operation::is_busy(&deps.operations));
        (
            done,
            solstone_core_backup::load_hosted_binding(root.path()),
            credentials_posts(&http),
            runner.calls.lock().unwrap().len(),
        )
    }

    let (poll, poll_binding, poll_creds, poll_restic) = run(true, false).await;
    let (local, local_binding, local_creds, local_restic) = run(false, true).await;
    for (label, body, binding, creds, restic) in [
        ("poll", &poll, poll_binding, poll_creds, poll_restic),
        ("local", &local, local_binding, local_creds, local_restic),
    ] {
        assert_eq!(body["operation"]["phase"], "error", "{label}");
        assert_eq!(body["operation"]["reason_code"], "auth_failed", "{label}");
        assert_eq!(
            binding,
            Some(crate::test_support::hosted_binding()),
            "{label}"
        );
        assert_eq!(body["hosted"]["bound"], true, "{label}");
        assert_eq!(creds, 1, "{label}");
        assert_eq!(restic, 3, "{label}");
        assert_ne!(body["mode"], "operated", "{label}");
        assert_eq!(body["enabled"], false, "{label}");
        let rendered = body.to_string();
        assert!(!rendered.contains("broker-token-secret"), "{label}");
        assert!(!rendered.contains("ACCESS"), "{label}");
        assert!(!rendered.contains("SECRET"), "{label}");
        assert!(!rendered.contains("SESSION"), "{label}");
    }
}

#[tokio::test]
async fn hosted_poll_restore_failure_matches_local_handoff_contract() {
    async fn run(
        poll_approved: bool,
        handoff_locally: bool,
    ) -> (
        Value,
        Option<solstone_core_backup::HostedBinding>,
        usize,
        usize,
    ) {
        let root = crate::test_support::root("fresh");
        let before = fs::read(root.path().join("config/journal.json")).unwrap();
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let runner = Arc::new(ScriptRunner::with_outputs(vec![
            version_output(),
            output(0, "[{\"paths\":[\"/original\"]}]"),
            output(1, ""),
        ]));
        let mut script = HttpScript::with_responses(vec![Ok(credentials_response())]);
        if poll_approved {
            script = script.with_poll_responses(vec![Ok(approved_poll_response())]);
        }
        let http = Arc::new(script);
        let deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            Some(restic.path().to_path_buf()),
        );
        let (status, started) = post_json(
            &deps,
            "/app/backup/restore-hosted",
            Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
        )
        .await;
        assert_eq!(status, 200);
        if handoff_locally {
            let nonce = portal_nonce(&started);
            let (status, _) = post_json(
                &deps,
                "/app/backup/handoff",
                Some(hosted_handoff_payload(&nonce)),
            )
            .await;
            assert_eq!(status, 200);
        }
        let done = wait_terminal(&deps).await;
        assert!(!crate::operation::is_busy(&deps.operations));
        assert_eq!(
            fs::read(root.path().join("config/journal.json")).unwrap(),
            before
        );
        (
            done,
            solstone_core_backup::load_hosted_binding(root.path()),
            credentials_posts(&http),
            runner.calls.lock().unwrap().len(),
        )
    }

    let (poll, poll_binding, poll_creds, poll_restic) = run(true, false).await;
    let (local, local_binding, local_creds, local_restic) = run(false, true).await;
    for (label, body, binding, creds, restic) in [
        ("poll", &poll, poll_binding, poll_creds, poll_restic),
        ("local", &local, local_binding, local_creds, local_restic),
    ] {
        assert_eq!(body["operation"]["phase"], "error", "{label}");
        assert_eq!(body["operation"]["reason_code"], "failed", "{label}");
        assert_ne!(body["operation"]["phase"], "degraded", "{label}");
        assert_eq!(
            binding,
            Some(crate::test_support::hosted_binding()),
            "{label}"
        );
        assert_eq!(creds, 1, "{label}");
        assert_eq!(restic, 3, "{label}");
        assert_ne!(body["mode"], "operated", "{label}");
        assert!(!body.to_string().contains("broker-token-secret"), "{label}");
        assert!(
            !body.to_string().contains(crate::test_support::RECOVERY_KEY),
            "{label}"
        );
    }
    assert_eq!(poll["operation"]["phase"], local["operation"]["phase"]);
    assert_eq!(
        poll["operation"]["reason_code"],
        local["operation"]["reason_code"]
    );
}

#[tokio::test]
async fn handoff_restore_composition_nonce_one_use_and_command_order() {
    let root = crate::test_support::root("fresh");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(restore_outputs()));
    let http = Arc::new(HttpScript::with_responses(vec![Ok(credentials_response())]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http,
        Some(restic.path().to_path_buf()),
    );
    let (status, started) = post_json(
        &deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    assert_eq!(status, 200);
    let nonce = portal_nonce(&started);
    let (status, _) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_handoff_payload(&nonce)),
    )
    .await;
    assert_eq!(status, 200);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["mode"], "operated");
    let rendered = done.to_string();
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
    assert!(!rendered.contains("broker-token-secret"));
    let heads = runner.argv_heads();
    let snapshots = heads.iter().position(|head| head == "snapshots");
    let restore = heads.iter().position(|head| head == "restore");
    assert!(snapshots.is_some() && restore.is_some());
    assert!(snapshots.unwrap() < restore.unwrap());
    let (status, body) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_handoff_payload(&nonce)),
    )
    .await;
    assert_handoff_refused(status, &body);
}

#[tokio::test]
async fn hosted_secrets_and_binding_mode_are_never_exposed() {
    struct Scenario {
        name: &'static str,
        poll: Vec<Result<HttpResponse, HttpError>>,
        credentials: Vec<Result<HttpResponse, HttpError>>,
        init: bool,
        disable: bool,
        bound: bool,
    }
    let subscribe = format!("{}/services/backup", crate::test_support::PORTAL_BASE);
    let scenarios = [
        Scenario {
            name: "success",
            poll: vec![
                Ok(HttpResponse {
                    status: 204,
                    headers: vec![],
                    body: vec![],
                }),
                Ok(approved_poll_response()),
            ],
            credentials: vec![Ok(credentials_response())],
            init: true,
            disable: true,
            bound: true,
        },
        Scenario {
            name: "refusal",
            poll: vec![Ok(needs_subscription_poll_body(&subscribe))],
            credentials: vec![],
            init: false,
            disable: false,
            bound: false,
        },
        Scenario {
            name: "parse error",
            poll: vec![Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: b"{".to_vec(),
            })],
            credentials: vec![],
            init: false,
            disable: false,
            bound: false,
        },
        Scenario {
            name: "expiry",
            poll: vec![Ok(HttpResponse {
                status: 410,
                headers: vec![],
                body: vec![],
            })],
            credentials: vec![],
            init: false,
            disable: false,
            bound: false,
        },
        Scenario {
            name: "broker failure",
            poll: vec![Ok(approved_poll_response())],
            credentials: vec![Err(HttpError::Unreachable)],
            init: false,
            disable: true,
            bound: true,
        },
    ];
    for scenario in scenarios {
        let root = crate::test_support::root("healthy");
        if scenario.disable {
            disable_backup(root.path());
        }
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let http = Arc::new(
            HttpScript::with_responses(scenario.credentials).with_poll_responses(scenario.poll),
        );
        let runner = if scenario.init {
            ScriptRunner::with_outputs(init_outputs())
        } else {
            ScriptRunner::with_outputs(vec![version_output()])
        };
        let deps = engine_deps(
            root.path().to_path_buf(),
            Arc::new(runner),
            http,
            Some(restic.path().to_path_buf()),
        );
        let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
        let done = wait_terminal(&deps).await;
        let daily_key = crate::config::backup(root.path()).ok().and_then(|backup| {
            backup
                .get("daily_key")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        assert_json_hides_secrets(&done, daily_key.as_deref());
        assert_eq!(done["hosted"]["bound"], scenario.bound, "{}", scenario.name);
        if scenario.bound {
            assert!(
                !journal_contains(root.path(), "ACCESS"),
                "{}",
                scenario.name
            );
            assert!(
                !journal_contains(root.path(), "SECRET"),
                "{}",
                scenario.name
            );
            assert!(
                !journal_contains(root.path(), "SESSION"),
                "{}",
                scenario.name
            );
        }
        if scenario.name == "success" {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let path = root.path().join("backup/hosted/binding.json");
                assert_eq!(
                    fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
}
