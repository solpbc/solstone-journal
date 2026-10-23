// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_backup_runtime::hosted_runtime::HttpError;
use solstone_core_backup_runtime::{
    Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance, JournalMaintenanceError,
    NativeRestoreRecorder, ToolOutput, ToolRequest, ToolRunner,
};

#[cfg(feature = "full-tests")]
const RESTIC_VERSION: &str = "0.19.0";
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;
#[cfg(feature = "full-tests")]
use std::{
    any::Any,
    future::{Future, poll_fn},
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Condvar, TryLockError},
    task::Poll,
    time::Instant,
};
use tower::ServiceExt;

#[cfg(all(test, feature = "full-tests"))]
#[cfg(not(windows))]
#[path = "handoff_poll_owned_tests.rs"]
mod owned_poll_tests;

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
                #[allow(unused_mut)]
                let mut expected_json = expected_json.clone();
                #[cfg(windows)]
                if matches!(
                    case["path"].as_str(),
                    Some("/app/backup/status" | "/app/backup/offload/status")
                ) && response.status == 200
                {
                    expected_json["cleanup_admission"] = json!("ready");
                }
                assert_eq!(
                    serde_json::from_slice::<Value>(&response.body).expect("json body"),
                    expected_json,
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

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn week_subset(value: &str) -> bool {
    let Some((count, weeks)) = value.split_once('/') else {
        return false;
    };
    weeks == "52"
        && !count.is_empty()
        && count.as_bytes()[0].is_ascii_digit()
        && count.as_bytes()[0] != b'0'
        && count.bytes().all(|byte| byte.is_ascii_digit())
}

#[tokio::test]
async fn demo_journal_fixture_serves_enabled_completed_verification() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/journal");
    let copy = tempfile::tempdir().unwrap();
    copy_tree(&source, copy.path());
    let (status, body) = response_json(
        crate::routes(copy.path().to_path_buf()),
        Request::get("/app/backup/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["last_verification"]["status"], "ok");
    let subset = body["last_verification"]["checked_subset"]
        .as_str()
        .expect("checked_subset");
    assert!(week_subset(subset), "{subset}");
    assert!(!body["last_verification"]["last_ok_time"].is_null());
    assert_eq!(body["last_backup"]["status"], "ok");
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
        assert_eq!(
            body.as_object().expect("object").len(),
            20 + usize::from(cfg!(windows))
        );
        assert_eq!(body["recovery_key_revocation_pending"], false);
        assert_eq!(body["recovery_key_revocation_unresolved"], false);
        #[cfg(windows)]
        assert_eq!(body["cleanup_admission"], "ready");
        assert!(
            body["last_backup_current_setup"].is_null(),
            "historical fixtures have no setup provenance"
        );
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
    assert_eq!(
        body.as_object().expect("object").len(),
        13 + usize::from(cfg!(windows))
    );
    #[cfg(windows)]
    assert_eq!(body["cleanup_admission"], "ready");
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
async fn backup_status_preserves_written_success_through_failure_and_destination_change() {
    use solstone_core_backup::{Destination, record_backup_result, set_destination, status_view};
    let root = crate::test_support::root("enabled_never_run");
    record_backup_result(
        root.path(),
        "ok",
        json!(100),
        json!("saved-copy"),
        json!("incomplete"),
        None,
        None,
    )
    .unwrap();
    record_backup_result(
        root.path(),
        "error",
        json!(200),
        Value::Null,
        json!("timeout"),
        None,
        None,
    )
    .unwrap();
    set_destination(
        root.path(),
        &Destination {
            repository: "s3:new-bucket/new-backup".into(),
            backend: "s3".into(),
            credentials: serde_json::Map::new(),
        },
    )
    .unwrap();
    let (_, body) = response_json(
        crate::routes(root.path().to_path_buf()),
        Request::get("/app/backup/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(body["last_backup"]["status"], "error");
    assert_eq!(body["last_backup"]["time"], 200);
    assert_eq!(
        body["last_backup"]["last_success"]["snapshot_id"],
        "saved-copy"
    );
    assert_eq!(
        body["last_backup"]["last_success"]["error_reason"],
        "incomplete"
    );
    assert_eq!(
        body["last_backup"],
        status_view(root.path()).unwrap()["last_backup"]
    );
    // Historical results have no attribution to the newly configured destination.
    assert!(
        body["last_backup"]["last_success"]
            .get("destination")
            .is_none()
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

#[cfg(feature = "full-tests")]
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
            output(10, ""),
            output(0, ""),
            output(0, ""),
            output(0, ""),
            output(0, "[{\"id\":\"snap\"}]"),
            output(0, ""),
            output(0, journal_catalog()),
            output(0, restore_summary()),
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
      "checked_subset": "1/52",
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
    let healthy: Value = serde_json::from_str(PYTHON_HEALTHY).expect("python healthy json");
    let pin = &healthy["backup"]["last_verification"]["checked_subset"];
    assert_eq!(
        pin.as_str(),
        Some("1/52"),
        "healthy checked_subset documenting literal"
    );
    assert_eq!(
        pin.as_str().expect("non-null subset"),
        solstone_core_backup_runtime::engine::verification_subset_for_week(1),
        "healthy checked_subset matches verification_subset_for_week(1)",
    );
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
            "cd114904a645f1ae8c92f7b1a057a3cee73b1ce59640e2a3aa4aca90b0b0432d",
        ),
        (
            "healthy",
            "f3068f71269517465d408f9c80e53f3726f1b1c477e416408efad70c360f40a2",
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

#[cfg(feature = "full-tests")]
struct Hold {
    released: Mutex<bool>,
    release: Condvar,
    started: AtomicBool,
    expired: AtomicBool,
}

#[cfg(feature = "full-tests")]
impl Hold {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            released: Mutex::new(false),
            release: Condvar::new(),
            started: AtomicBool::new(false),
            expired: AtomicBool::new(false),
        })
    }

    fn has_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    fn try_release(&self) -> Result<bool, &'static str> {
        match self.released.try_lock() {
            Ok(mut released) => {
                *released = true;
                self.release.notify_all();
                Ok(true)
            }
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Poisoned(_)) => Err("hold release lock poisoned"),
        }
    }

    async fn release(&self) {
        let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
        loop {
            if self.try_release().expect("hold release") {
                return;
            }
            fixture_tick(deadline)
                .await
                .expect("hold release timed out");
        }
    }

    fn arrive_and_wait(&self) -> Result<(), ()> {
        self.started.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut released = self.released.lock().map_err(|_| ())?;
        while !*released {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                self.expired.store(true, Ordering::Release);
                *released = true;
                self.release.notify_all();
                return Err(());
            };
            released = self
                .release
                .wait_timeout(released, remaining)
                .map_err(|_| ())?
                .0;
        }
        if self.expired.load(Ordering::Acquire) {
            Err(())
        } else {
            Ok(())
        }
    }
}

#[cfg(feature = "full-tests")]
const FIXTURE_WAIT: Duration = Duration::from_secs(5);

#[cfg(feature = "full-tests")]
async fn fixture_tick(deadline: tokio::time::Instant) -> Result<(), &'static str> {
    let remaining = deadline
        .checked_duration_since(tokio::time::Instant::now())
        .ok_or("fixture observation timed out")?;
    tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
    Ok(())
}

#[cfg(feature = "full-tests")]
#[derive(Debug, PartialEq, Eq)]
enum HoldStartFailure {
    TerminalBeforeStart {
        phase: String,
        reason_code: Option<String>,
    },
    GenerationChanged,
    NoOperation,
    SlotUnavailable,
    TimedOut,
}

#[cfg(feature = "full-tests")]
async fn fixture_generation(deps: &crate::BackupWebDeps) -> u64 {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        match deps.operations.try_lock() {
            Ok(guard) => {
                return guard
                    .current
                    .as_ref()
                    .expect("fixture operation")
                    .generation;
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(_)) => panic!("fixture operation slot poisoned"),
        }
        fixture_tick(deadline)
            .await
            .expect("fixture generation timed out");
    }
}

#[cfg(feature = "full-tests")]
async fn wait_started_or_terminal(
    hold: &Hold,
    deps: &crate::BackupWebDeps,
    generation: u64,
) -> Result<(), HoldStartFailure> {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        if hold.has_started() {
            return Ok(());
        }
        match deps.operations.try_lock() {
            Ok(guard) => {
                let current = guard
                    .current
                    .as_ref()
                    .ok_or(HoldStartFailure::NoOperation)?;
                if current.generation != generation {
                    return Err(HoldStartFailure::GenerationChanged);
                }
                if crate::operation::is_terminal(&current.view.phase) {
                    return Err(HoldStartFailure::TerminalBeforeStart {
                        phase: current.view.phase.clone(),
                        reason_code: current.view.reason_code.clone(),
                    });
                }
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(_)) => return Err(HoldStartFailure::SlotUnavailable),
        }
        fixture_tick(deadline)
            .await
            .map_err(|_| HoldStartFailure::TimedOut)?;
    }
}

#[cfg(feature = "full-tests")]
fn fixture_worker_settled(current: &crate::operation::Slot) -> bool {
    #[cfg(windows)]
    if current.cleanup.as_ref().is_some_and(|pending| {
        pending.owners.iter().any(|owner| {
            !matches!(
                owner.observe(),
                solstone_core_system::process::HelperCleanupStatus::Quiescent
            )
        })
    }) {
        return false;
    }
    !current.worker_active
}

#[cfg(feature = "full-tests")]
async fn catch_fixture_panic<T>(future: impl Future<Output = T>) -> std::thread::Result<T> {
    let mut future = Box::pin(future);
    let result = poll_fn(
        |cx| match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(panic) => Poll::Ready(Err(panic)),
        },
    )
    .await;
    let dropped = catch_unwind(AssertUnwindSafe(|| drop(future)));
    result.and_then(|value| dropped.map(|()| value))
}

#[cfg(feature = "full-tests")]
fn panic_description(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("non-string fixture panic")
}

#[cfg(feature = "full-tests")]
async fn settle_held_fixture(
    deps: &crate::BackupWebDeps,
    holds: &[Arc<Hold>],
    transports: &[Arc<HttpScript>],
) -> Result<(), &'static str> {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        let mut released = true;
        for hold in holds {
            released &= hold.try_release()?;
        }
        // Approval publishes a successor worker before releasing its poll lease.
        // Observe that release before the final worker/helper snapshot.
        let poll_settled = !deps.handoff_poll_lease.load(Ordering::Acquire)
            && !crate::operation::handoff_poll_active(&deps.operations);
        let owner_settled = match deps.operations.try_lock() {
            Ok(mut guard) => {
                if let Some(current) = guard.current.as_mut() {
                    // This is the existing test-only backdate state update, made
                    // under a fallible lock rather than the blocking helper.
                    if released
                        && !poll_settled
                        && !current.worker_active
                        && matches!(
                            current.view.kind.as_str(),
                            "enable_hosted" | "restore_hosted"
                        )
                        && current.nonce.is_some()
                        && !crate::operation::is_terminal(&current.view.phase)
                    {
                        current.started = Instant::now()
                            .checked_sub(crate::operation::HANDOFF_TTL + Duration::from_secs(1))
                            .ok_or("fixture backdate unavailable")?;
                    }
                    poll_settled && fixture_worker_settled(current)
                } else {
                    poll_settled
                }
            }
            Err(TryLockError::WouldBlock) => false,
            Err(TryLockError::Poisoned(_)) => {
                return Err("operation slot poisoned during settlement");
            }
        };
        if released && owner_settled && transports.iter().all(|http| http.active_executions() == 0)
        {
            return Ok(());
        }
        fixture_tick(deadline).await?;
    }
}

#[cfg(feature = "full-tests")]
async fn held_fixture<T>(
    deps: crate::BackupWebDeps,
    holds: Vec<Arc<Hold>>,
    transports: Vec<Arc<HttpScript>>,
    mut roots: Vec<tempfile::TempDir>,
    exercise: impl Future<Output = T>,
) -> T {
    // Cleanup stays disabled through any panic, including cleanup's own failure.
    for root in &mut roots {
        root.disable_cleanup(true);
    }
    let mut exercise = Box::pin(exercise);
    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        poll_fn(
            |cx| match catch_unwind(AssertUnwindSafe(|| exercise.as_mut().poll(cx))) {
                Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
                Ok(Poll::Pending) => Poll::Pending,
                Err(panic) => Poll::Ready(Err(panic)),
            },
        ),
    )
    .await;
    let drop_failure = catch_unwind(AssertUnwindSafe(|| drop(exercise)));
    let settlement = catch_fixture_panic(settle_held_fixture(&deps, &holds, &transports)).await;
    let settlement_failure = match &settlement {
        Ok(Ok(())) => None,
        Ok(Err(reason)) => Some((*reason).to_owned()),
        Err(panic) => Some(format!(
            "cleanup panicked: {}",
            panic_description(panic.as_ref())
        )),
    };
    if settlement_failure.is_some() || drop_failure.is_err() {
        let retained: Vec<_> = roots.into_iter().map(tempfile::TempDir::keep).collect();
        let exercise_failure = match &outcome {
            Ok(Ok(_)) => "none".to_owned(),
            Ok(Err(panic)) => format!("panic: {}", panic_description(panic.as_ref())),
            Err(_) => "exercise timed out".to_owned(),
        };
        let drop_failure = drop_failure
            .err()
            .map(|panic| panic_description(panic.as_ref()).to_owned());
        panic!(
            "exercise failure: {exercise_failure}; exercise drop failure: {drop_failure:?}; settlement failure: {settlement_failure:?}; retained roots: {retained:?}"
        );
    }
    for root in &mut roots {
        root.disable_cleanup(false);
    }
    drop(roots);
    match outcome {
        Ok(Ok(value)) => {
            assert!(
                holds
                    .iter()
                    .all(|hold| !hold.expired.load(Ordering::Acquire)),
                "fixture hold release deadline expired"
            );
            value
        }
        Ok(Err(panic)) => resume_unwind(panic),
        Err(_) => panic!("fixture exercise timed out after settled cleanup"),
    }
}

struct ScriptRunner {
    outputs: Mutex<VecDeque<ToolOutput>>,
    calls: Mutex<Vec<(PathBuf, Vec<String>)>>,
    overruns: AtomicUsize,
    #[cfg(feature = "full-tests")]
    hold: Option<Arc<Hold>>,
}

impl ScriptRunner {
    fn with_outputs(outputs: Vec<ToolOutput>) -> Self {
        Self {
            outputs: Mutex::new(VecDeque::from(outputs)),
            calls: Mutex::new(vec![]),
            overruns: AtomicUsize::new(0),
            #[cfg(feature = "full-tests")]
            hold: None,
        }
    }

    #[cfg(feature = "full-tests")]
    fn with_hold(outputs: Vec<ToolOutput>, hold: Arc<Hold>) -> Self {
        Self {
            outputs: Mutex::new(VecDeque::from(outputs)),
            calls: Mutex::new(vec![]),
            overruns: AtomicUsize::new(0),
            hold: Some(hold),
        }
    }

    #[cfg(feature = "full-tests")]
    fn argv_heads(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, argv)| argv.first().cloned().unwrap_or_default())
            .collect()
    }

    #[cfg(feature = "full-tests")]
    fn overrun_count(&self) -> usize {
        self.overruns.load(Ordering::Acquire)
    }
}

impl ToolRunner for ScriptRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let argv: Vec<String> = request
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        self.calls
            .lock()
            .unwrap()
            .push((PathBuf::from(&request.program), argv.clone()));
        #[cfg(feature = "full-tests")]
        if let Some(hold) = &self.hold {
            hold.arrive_and_wait()
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "fixture hold expired"))?;
        }
        let Some(output) = self.outputs.lock().unwrap().pop_front() else {
            self.overruns.fetch_add(1, Ordering::AcqRel);
            return Err(io::Error::other(format!(
                "ScriptRunner has no remaining output for {argv:?}"
            )));
        };
        Ok(output)
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _cancellation: &solstone_core_backup::archive_admission::ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        self.run(request)
    }
}

#[cfg(feature = "full-tests")]
struct PanicAfterVersionRunner;

#[cfg(feature = "full-tests")]
impl ToolRunner for PanicAfterVersionRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        if request.argv.first().and_then(|a| a.to_str()) == Some("version") {
            return Ok(version_output());
        }
        panic!("restore fixture panic")
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _cancellation: &solstone_core_backup::archive_admission::ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        self.run(request)
    }
}

#[derive(Default)]
struct HttpScript {
    responses: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    poll_responses: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    requests: Mutex<Vec<HttpRequest>>,
    poll_cancellations: Mutex<Vec<solstone_core_backup::archive_admission::ArchiveCancellation>>,
    #[cfg(feature = "full-tests")]
    hold: Option<Arc<Hold>>,
    #[cfg(feature = "full-tests")]
    hold_after: usize,
    #[cfg(all(test, feature = "full-tests"))]
    hold_url_suffix: Option<&'static str>,
    #[cfg(feature = "full-tests")]
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
    #[cfg(feature = "full-tests")]
    fn with_responses(responses: Vec<Result<HttpResponse, HttpError>>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from(responses)),
            poll_responses: Mutex::new(VecDeque::new()),
            requests: Mutex::new(vec![]),
            poll_cancellations: Mutex::new(vec![]),
            #[cfg(feature = "full-tests")]
            hold: None,
            #[cfg(feature = "full-tests")]
            hold_after: 0,
            #[cfg(all(test, feature = "full-tests"))]
            hold_url_suffix: None,
            #[cfg(feature = "full-tests")]
            hold_skips: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }

    #[cfg(feature = "full-tests")]
    fn with_poll_responses(self, responses: Vec<Result<HttpResponse, HttpError>>) -> Self {
        *self.poll_responses.lock().unwrap() = VecDeque::from(responses);
        self
    }

    #[cfg(feature = "full-tests")]
    fn with_hold(self, hold: Arc<Hold>) -> Self {
        Self {
            hold: Some(hold),
            ..self
        }
    }

    #[cfg(feature = "full-tests")]
    fn with_hold_after(self, skip: usize, hold: Arc<Hold>) -> Self {
        Self {
            hold: Some(hold),
            hold_after: skip,
            ..self
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn with_hold_for_url_suffix(self, suffix: &'static str, hold: Arc<Hold>) -> Self {
        Self {
            hold: Some(hold),
            hold_url_suffix: Some(suffix),
            ..self
        }
    }

    #[cfg(feature = "full-tests")]
    fn active_executions(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn max_concurrency(&self) -> usize {
        self.max_active.load(Ordering::Acquire)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl HttpTransport for HttpScript {
    fn execute_cancellable(
        &self,
        request: &HttpRequest,
        cancellation: &solstone_core_backup::archive_admission::ArchiveCancellation,
    ) -> Result<HttpResponse, HttpError> {
        self.poll_cancellations
            .lock()
            .unwrap()
            .push(cancellation.clone());
        if cancellation.is_cancelled() {
            return Err(HttpError::Other);
        }
        // This fixture deliberately retains its Hold until released, even after stop.
        self.execute(request)
    }

    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.requests.lock().unwrap().push(request.clone());
        let now = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(now, Ordering::AcqRel);
        let _active = ActiveGuard(&self.active);
        #[cfg(all(test, feature = "full-tests"))]
        if let Some(hold) = &self.hold
            && self
                .hold_url_suffix
                .is_none_or(|suffix| request.url.ends_with(suffix))
        {
            let seen = self.hold_skips.fetch_add(1, Ordering::Relaxed);
            if seen >= self.hold_after {
                hold.arrive_and_wait().map_err(|_| HttpError::Other)?;
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

#[cfg(feature = "full-tests")]
fn version_output() -> ToolOutput {
    output(0, &format!("restic {RESTIC_VERSION}\n"))
}

#[cfg(feature = "full-tests")]
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

fn engine_deps(
    journal_root: PathBuf,
    runner: Arc<dyn ToolRunner + Send + Sync>,
    http: Arc<dyn HttpTransport + Send + Sync>,
    executable: Option<PathBuf>,
) -> crate::BackupWebDeps {
    write_test_committed_identity(&journal_root);
    let executable = match executable {
        Some(path) => {
            if path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name == "restic" || name == "restic.exe")
            {
                path.parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("bin/solstone"))
                    .unwrap_or(path)
            } else if path.is_dir() {
                path.join("bin/solstone")
            } else {
                path
            }
        }
        None => PathBuf::from(""),
    };
    crate::BackupWebDeps {
        journal_root,
        cache: corpus_cache(),
        operations: crate::operation::new_slot(),
        runner,
        http,
        clock: Arc::new(TestClock),
        journal_maintenance: Arc::new(OkMaintenance),
        restore_recorder: Arc::new(NativeRestoreRecorder),
        executable,
        portal_base: crate::test_support::PORTAL_BASE.into(),
        version: "test",
        handoff_poll_lease: Arc::new(AtomicBool::new(false)),
        restore_prepare: crate::restore_prepare::new_shared(),
    }
}

fn write_test_committed_identity(journal_root: &Path) -> String {
    let ca = solstone_core_sol_link::ca::generate_ca().expect("ca");
    let cert_pem = ca.certificate_pem();
    let key_pem = ca.private_key_pem();
    let instance_id = solstone_core_sol_link::ca::jid_from_spki(ca.spki_der()).expect("jid");

    let ca_dir = journal_root.join("link/ca");
    fs::create_dir_all(&ca_dir).expect("ca dir");
    fs::write(ca_dir.join("cert.pem"), cert_pem).expect("cert");
    fs::write(ca_dir.join("private.pem"), key_pem).expect("key");
    let state_path = journal_root.join("link/state.json");
    fs::write(
        state_path,
        format!(r#"{{"instance_id":"{instance_id}","home_label":"Test Home"}}"#),
    )
    .expect("state");
    instance_id
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

#[cfg(feature = "full-tests")]
async fn wait_terminal(deps: &crate::BackupWebDeps) -> Value {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        let (status, body) = tokio::time::timeout_at(deadline, get_status_json(deps))
            .await
            .expect("operation status timed out");
        let phase = body["operation"]["phase"].as_str().unwrap_or("");
        let settled = match deps.operations.try_lock() {
            Ok(guard) => guard.current.as_ref().is_none_or(fixture_worker_settled),
            Err(TryLockError::WouldBlock) => false,
            Err(TryLockError::Poisoned(_)) => panic!("operation slot poisoned"),
        };
        if status == 200
            && settled
            && (crate::operation::is_terminal(phase) || body["operation"].is_null())
        {
            return body;
        }
        fixture_tick(deadline)
            .await
            .expect("operation did not terminate");
    }
}

#[cfg(feature = "full-tests")]
async fn drain_hosted_wait(deps: &crate::BackupWebDeps) {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        let updated = match deps.operations.try_lock() {
            Ok(mut guard) => {
                if let Some(current) = guard.current.as_mut() {
                    current.started = Instant::now()
                        .checked_sub(crate::operation::HANDOFF_TTL + Duration::from_secs(1))
                        .expect("fixture backdate");
                }
                true
            }
            Err(TryLockError::WouldBlock) => false,
            Err(TryLockError::Poisoned(_)) => panic!("operation slot poisoned"),
        };
        if updated {
            break;
        }
        fixture_tick(deadline)
            .await
            .expect("fixture backdate timed out");
    }
    let _ = wait_terminal(deps).await;
    while deps.handoff_poll_lease.load(Ordering::Acquire) {
        fixture_tick(deadline)
            .await
            .expect("poll lease did not release");
    }
}

#[cfg(feature = "full-tests")]
fn disable_backup(root: &Path) {
    solstone_core_backup::set_enabled(root, false).unwrap();
}

#[cfg(feature = "full-tests")]
fn restore_body() -> Value {
    json!({
        "recovery_key": crate::test_support::RECOVERY_KEY,
        "repository": "s3:bucket",
        "backend": "s3",
        "credentials": {"access_key_id": "key", "secret_access_key": "secret"}
    })
}

#[cfg(feature = "full-tests")]
const JOURNAL_SNAPSHOT_ID: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[cfg(feature = "full-tests")]
fn journal_catalog() -> &'static str {
    "[{\"id\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"time\":\"2026-01-01T00:00:00.000000000+00:00\",\"paths\":[\"/original\"],\"summary\":{\"total_files_processed\":4}}]"
}

#[cfg(feature = "full-tests")]
fn restore_summary() -> &'static str {
    "[{\"message_type\":\"summary\",\"total_files\":4,\"files_restored\":4,\"total_bytes\":12,\"bytes_restored\":12}]"
}

#[cfg(feature = "full-tests")]
fn init_outputs() -> Vec<ToolOutput> {
    vec![output(10, ""), output(0, ""), output(0, ""), output(0, "")]
}

#[cfg(feature = "full-tests")]
fn restore_outputs() -> Vec<ToolOutput> {
    vec![
        output(0, journal_catalog()),
        output(0, restore_summary()),
        output(0, ""),
    ]
}

#[cfg(feature = "full-tests")]
fn rotate_outputs() -> Vec<ToolOutput> {
    vec![
        output(0, "[{\"id\":\"old\",\"current\":true}]"),
        output(0, ""),
        output(0, ""),
        output(0, ""),
    ]
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
            path: "/app/backup/recovery-key/rotate",
            body: None,
            kind: "rotate",
            phase: "rotating",
            disable: false,
            outputs: rotate_outputs(),
        },
    ];
    for case in cases {
        let root = crate::test_support::root("healthy");
        let journal_path = root.path().to_path_buf();
        if case.disable {
            disable_backup(journal_path.as_path());
        }
        let hold = Hold::new();
        let runner = ScriptRunner::with_hold(case.outputs, hold.clone());
        let (deps, _restic) = prepared(journal_path.as_path().to_path_buf(), Arc::new(runner));
        held_fixture(
            deps.clone(),
            vec![hold.clone()],
            vec![],
            vec![root, _restic],
            async {
                let (status, body) = post_json(&deps, case.path, case.body).await;
                assert_eq!(status, 200, "{}", case.path);
                assert_eq!(body["operation"]["kind"], case.kind, "{}", case.path);
                assert_eq!(body["operation"]["phase"], case.phase, "{}", case.path);
                wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                    .await
                    .expect("fixture did not enter Hold");
                let (status, during) = get_status_json(&deps).await;
                assert_eq!(status, 200, "{}", case.path);
                assert_eq!(during["operation"]["phase"], case.phase, "{}", case.path);
                assert!(
                    !crate::operation::is_terminal(during["operation"]["phase"].as_str().unwrap()),
                    "{}",
                    case.path
                );
                hold.release().await;
                let done = wait_terminal(&deps).await;
                assert!(
                    crate::operation::is_terminal(
                        done["operation"]["phase"].as_str().unwrap_or("")
                    ),
                    "{} {}",
                    case.path,
                    done["operation"]["phase"]
                );
            },
        )
        .await;
    }
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn engine_route_missing_package_reports_terminal_before_hold_start() {
    let root = crate::test_support::root("healthy");
    let journal_path = root.path().to_path_buf();
    let hold = Hold::new();
    let runner = Arc::new(ScriptRunner::with_hold(restore_outputs(), hold.clone()));
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        journal_path.clone(),
        runner.clone(),
        http.clone(),
        Some(journal_path.join("missing-package/bin/solstone")),
    );
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone()],
        vec![root],
        async {
            let (status, _) = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
            assert_eq!(status, 200);
            assert_eq!(
                wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await).await,
                Err(HoldStartFailure::TerminalBeforeStart {
                    phase: "error".into(),
                    reason_code: Some("restic_unavailable".into()),
                }),
            );
            assert!(!hold.has_started());
            assert!(runner.calls.lock().unwrap().is_empty());
            assert!(http.requests.lock().unwrap().is_empty());
            let done = wait_terminal(&deps).await;
            assert_eq!(done["operation"]["reason_code"], "restic_unavailable");
        },
    )
    .await;
    assert!(!journal_path.exists());
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn held_fixture_failure_releases_and_settles_before_dropping_roots() {
    for (hosted_poll, approved) in [(false, false), (true, false), (true, true)] {
        let root = crate::test_support::root("healthy");
        disable_backup(root.path());
        let journal_path = root.path().to_path_buf();
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let package_path = restic.path().to_path_buf();
        let hold = Hold::new();
        let runner = Arc::new(if approved {
            let mut outputs = init_outputs();
            outputs.push(output(0, "[]"));
            ScriptRunner::with_outputs(outputs)
        } else {
            ScriptRunner::with_hold(init_outputs(), hold.clone())
        });
        let http = Arc::new(if approved {
            HttpScript::with_responses(vec![Ok(credentials_response())])
                .with_poll_responses(vec![Ok(approved_poll_response())])
                .with_hold_after(1, hold.clone())
        } else if hosted_poll {
            // Keep the pending-poll failure distinct from approved successor work.
            HttpScript::default().with_hold(hold.clone())
        } else {
            HttpScript::default()
        });
        let deps = engine_deps(
            journal_path.clone(),
            runner.clone(),
            http.clone(),
            Some(package_path.clone()),
        );
        let result = catch_fixture_panic(held_fixture(
            deps.clone(),
            vec![hold.clone()],
            vec![http.clone()],
            vec![root, restic],
            async {
                let route = if hosted_poll {
                    "/app/backup/enable-hosted"
                } else {
                    "/app/backup/enable"
                };
                assert_eq!(post_json(&deps, route, None).await.0, 200);
                wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                    .await
                    .expect("fixture did not enter Hold");
                assert!(journal_path.exists() && package_path.exists());
                if approved {
                    assert_eq!(poll_gets(&http).len(), 1);
                    assert_credentials_requests(&http, Some("operated"));
                    assert_eq!(http.active_executions(), 1);
                    let worker_active = {
                        let guard = deps.operations.try_lock().expect("fixture slot");
                        guard
                            .current
                            .as_ref()
                            .expect("fixture operation")
                            .worker_active
                    };
                    assert!(worker_active);
                }
                panic!("deliberate held fixture assertion failure");
            },
        ))
        .await;
        let panic = result.expect_err("exercise must fail");
        assert_eq!(
            panic_description(panic.as_ref()),
            "deliberate held fixture assertion failure"
        );
        assert!(hold.has_started());
        assert!(!hold.expired.load(Ordering::Acquire));
        assert_eq!(http.active_executions(), 0);
        assert!(!deps.handoff_poll_lease.load(Ordering::Acquire));
        let view = {
            let guard = deps.operations.try_lock().expect("fixture slot");
            let current = guard.current.as_ref().expect("fixture operation");
            let settled = fixture_worker_settled(current);
            (current.view.clone(), settled)
        };
        assert!(view.1);
        assert_eq!(
            view.0.phase,
            if hosted_poll && !approved {
                "error"
            } else {
                "done"
            }
        );
        if hosted_poll && !approved {
            assert_eq!(view.0.reason_code.as_deref(), Some("expired"));
        } else {
            assert!(view.0.reason_code.is_none());
        }
        if approved {
            argv_in_order(&runner.argv_heads(), &["cat", "init", "key"]);
            assert_eq!(runner.overrun_count(), 0);
        }
        assert!(!journal_path.exists() && !package_path.exists());
    }
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn enable_init_failure_leaves_disabled_and_errors() {
    let root = crate::test_support::root("healthy");
    disable_backup(root.path());
    let runner = ScriptRunner::with_outputs(vec![output(10, ""), output(12, "")]);
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let (status, _) = post_json(&deps, "/app/backup/enable", None).await;
    assert_eq!(status, 200);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "auth_failed");
    assert_eq!(done["enabled"], false);
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn restore_success_records_snapshots_then_restore() {
    let root = crate::test_support::root("healthy");
    let runner = Arc::new(
        solstone_core_backup_runtime::test_support::ArgvResticFixture::new(
            journal_catalog(),
            output(0, restore_summary()),
            output(0, ""),
        ),
    );
    let (deps, _restic) = prepared(root.path().to_path_buf(), runner.clone());
    let (status, body) = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "restore");
    assert_eq!(body["operation"]["phase"], "restoring");
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(
        runner.calls(),
        vec![
            vec!["snapshots".into(), "--json".into()],
            vec![
                "restore".into(),
                format!("{JOURNAL_SNAPSHOT_ID}:/original"),
                "--target".into(),
                root.path().display().to_string(),
                "--exclude".into(),
                "/health/supervisor.process_instance".into(),
                "--exclude".into(),
                "/health/callosum.pipe-*".into(),
                #[cfg(target_os = "linux")]
                "--exclude".into(),
                #[cfg(target_os = "linux")]
                "backup-activity.lock".into(),
                "--json".into(),
            ],
            vec!["check".into()],
        ]
    );
    assert!(runner.refusals().is_empty());
    assert_eq!(done["last_restore"]["status"], "ok");
    assert_eq!(done["last_restore"]["scope"], "journal");
    let files = if cfg!(target_os = "linux") {
        Value::Null
    } else {
        json!(4)
    };
    assert_eq!(done["last_restore"]["files_expected"], files);
    assert_eq!(done["last_restore"]["files_restored"], files);
    assert_eq!(done["last_restore"]["bytes_restored"], 12);
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn restore_empty_snapshots_records_journal_snapshot_not_found() {
    let root = crate::test_support::root("healthy");
    let runner = ScriptRunner::with_outputs(vec![output(0, "[]")]);
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let _ = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(
        done["operation"]["reason_code"],
        "journal_snapshot_not_found"
    );
    assert_eq!(done["last_restore"]["status"], "error");
    assert_eq!(done["last_restore"]["reason"], "journal_snapshot_not_found");
    assert_eq!(done["last_restore"]["scope"], "journal");
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn restore_adapter_records_invalid_key_once_after_tool_resolution() {
    let root = crate::test_support::root("healthy");
    let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
    let (mut deps, _restic) = prepared(root.path().to_path_buf(), runner);
    let recorder = Arc::new(solstone_core_backup_runtime::test_support::RestoreRecorderSpy::new());
    deps.restore_recorder = recorder.clone();
    let mut body = restore_body();
    body["recovery_key"] = Value::String("not a valid recovery key".into());

    let (status, _) = post_json(&deps, "/app/backup/restore", Some(body)).await;
    assert_eq!(status, 200);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["reason_code"], "invalid_key");
    assert_eq!(recorder.calls().len(), 1);
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn restore_worker_panic_is_restore_failed_and_records_once() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().expect("restic");
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(PanicAfterVersionRunner);
    let mut deps = engine_deps(
        root.path().to_path_buf(),
        runner,
        Arc::new(HttpScript::default()),
        Some(restic.path().to_path_buf()),
    );
    let recorder = Arc::new(solstone_core_backup_runtime::test_support::RestoreRecorderSpy::new());
    deps.restore_recorder = recorder.clone();

    let (status, _) = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
    assert_eq!(status, 200);
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "restore_failed");
    assert_eq!(recorder.calls().len(), 1);
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn restore_check_failure_is_degraded() {
    let root = crate::test_support::root("healthy");
    let runner = ScriptRunner::with_outputs(vec![
        output(0, journal_catalog()),
        output(0, restore_summary()),
        output(11, ""),
    ]);
    let (deps, _restic) = prepared(root.path().to_path_buf(), Arc::new(runner));
    let _ = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "degraded");
    assert_eq!(done["operation"]["reason_code"], "integrity_unverified");
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn rotate_does_not_echo_recovery_key() {
    let root = crate::test_support::root("healthy");
    let runner = ScriptRunner::with_outputs(vec![
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

#[cfg(feature = "full-tests")]
async fn hold_until_rotate_done(deps: crate::BackupWebDeps, runner: Arc<ScriptRunner>) {
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["recovery_key_confirmed"], false);
    assert!(done.get("recovery_key").is_none());
    assert!(runner.argv_heads().iter().any(|head| head == "key"));
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn second_engine_post_is_backup_busy() {
    let root = crate::test_support::root("healthy");
    let journal_path = root.path().to_path_buf();
    disable_backup(journal_path.as_path());
    let hold = Hold::new();
    let runner = ScriptRunner::with_hold(init_outputs(), hold.clone());
    let (deps, _restic) = prepared(journal_path.as_path().to_path_buf(), Arc::new(runner));
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![],
        vec![root, _restic],
        async {
            let (status, body) = post_json(&deps, "/app/backup/enable", None).await;
            assert_eq!(status, 200);
            assert_eq!(body["operation"]["phase"], "setting_up");
            wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                .await
                .expect("fixture did not enter Hold");
            let (busy_status, busy) = post_json(&deps, "/app/backup/teardown", None).await;
            assert_eq!(busy_status, 400);
            assert_eq!(busy["reason_code"], "backup_busy");
            hold.release().await;
            let _ = wait_terminal(&deps).await;
        },
    )
    .await;
}

// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn destination_returns_distinct_reason_codes_from_cat_config() {
    let destination = json!({"repository":"s3:bucket","backend":"s3","credentials":{"access_key_id":"key","secret_access_key":"secret"}});
    for (code, reason) in [(0, "repo_exists"), (12, "auth_failed"), (99, "unreachable")] {
        let root = crate::test_support::root("fresh");
        let runner = ScriptRunner::with_outputs(vec![output(code, "")]);
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

#[cfg(feature = "full-tests")]
fn assert_enable_portal_url(url: &str) {
    let relative = url.strip_prefix("https://services.solstone.app").unwrap();
    let (path, query) = relative.split_once('?').unwrap();
    assert_eq!(path, "/enable/backup");
    let pairs = query
        .split('&')
        .map(|p| p.split_once('=').unwrap())
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(pairs.len(), 4);

    let nonce = pairs.get("nonce").copied().unwrap();
    assert_eq!(nonce.len(), solstone_core_handoff_nonce::NONCE_LENGTH_CHARS);
    assert!(
        nonce
            .bytes()
            .all(|byte| solstone_core_handoff_nonce::NONCE_ALPHABET.contains(&byte))
    );

    let instance = pairs.get("instance").copied().unwrap();
    assert_eq!(instance.len(), 36);
    let parts: Vec<&str> = instance.split('-').collect();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0].len(), 8);
    assert_eq!(parts[1].len(), 4);
    assert_eq!(parts[2].len(), 4);
    assert_eq!(parts[3].len(), 4);
    assert_eq!(parts[4].len(), 12);

    let assertion_encoded = pairs.get("assertion").copied().unwrap();
    let compact = solstone_core_sol_link::home_reach::percent_decode(assertion_encoded).unwrap();
    let ca_pubkey_encoded = pairs.get("ca_pubkey").copied().unwrap();
    let ca_pubkey = solstone_core_sol_link::home_reach::percent_decode(ca_pubkey_encoded).unwrap();
    assert!(!ca_pubkey.contains("PRIVATE KEY"));
    assert!(ca_pubkey.starts_with("-----BEGIN PUBLIC KEY-----"));
    assert!(ca_pubkey.ends_with("-----END PUBLIC KEY-----"));

    let claims =
        solstone_core_sol_link::home_reach::decode_service_enable_claims(&compact).unwrap();
    assert_eq!(claims["iss"], format!("home:{instance}"));
    assert_eq!(claims["aud"], "solstone-reach");
    assert_eq!(claims["scope"], "services.enable");
    assert_eq!(claims["instance_id"], instance);
    assert_eq!(claims["nonce"], nonce);
    assert_eq!(claims["service"], "spb");
    let iat = claims["iat"].as_i64().unwrap();
    let exp = claims["exp"].as_i64().unwrap();
    assert_eq!(exp - iat, 1800);
}

#[cfg(feature = "full-tests")]
fn assert_restore_portal_url(url: &str) {
    let relative = url.strip_prefix("https://services.solstone.app").unwrap();
    let (path, query) = relative.split_once('?').unwrap();
    assert_eq!(path, "/enable/backup");
    let pairs = query
        .split('&')
        .map(|pair| pair.split_once('=').unwrap())
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs.get("intent"), Some(&"restore"));
    assert!(!pairs.contains_key("instance"));
    let nonce = pairs.get("nonce").copied().unwrap();
    assert_eq!(nonce.len(), solstone_core_handoff_nonce::NONCE_LENGTH_CHARS);
    assert!(
        nonce
            .bytes()
            .all(|byte| solstone_core_handoff_nonce::NONCE_ALPHABET.contains(&byte))
    );
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn enable_hosted_returns_portal_url() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    let (status, body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["operation"]["kind"], "enable_hosted");
    assert_eq!(body["operation"]["phase"], "setting_up");
    let url = body["operation"]["portal_url"].as_str().unwrap();
    assert_enable_portal_url(url);
    drain_hosted_wait(&deps).await;
}

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn enable_hosted_verifies_refusals_and_busy_behavior() {
    use solstone_core_sol_link::home_reach::{
        HomeReachFaultGuard, HomeReachFaultPrimitive, verify_service_enable_compact,
    };

    // 1. Success case independently ring-verifies and checks response body
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    let (status, body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert!(
        !serde_json::to_string(&body)
            .unwrap()
            .contains("PRIVATE KEY")
    );
    let url = body["operation"]["portal_url"].as_str().unwrap();
    assert_enable_portal_url(url);
    let pairs = url
        .split_once('?')
        .unwrap()
        .1
        .split('&')
        .map(|p| p.split_once('=').unwrap())
        .collect::<std::collections::BTreeMap<_, _>>();
    let compact = solstone_core_sol_link::home_reach::percent_decode(pairs["assertion"]).unwrap();
    let pem = solstone_core_sol_link::home_reach::percent_decode(pairs["ca_pubkey"]).unwrap();
    assert!(verify_service_enable_compact(&pem, &compact));

    // Busy while open returns existing refusal and keeps stored URL
    let (busy_status, busy_body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(busy_status, 400);
    assert_eq!(busy_body["reason_code"], "backup_busy");
    let (_, status_body) = get_status_json(&deps).await;
    assert_eq!(
        status_body["operation"]["portal_url"].as_str().unwrap(),
        url
    );

    drain_hosted_wait(&deps).await;

    // After finish, next start gets new nonce and different assertion
    let (next_status, next_body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(next_status, 200);
    let next_url = next_body["operation"]["portal_url"].as_str().unwrap();
    assert_ne!(url, next_url);
    drain_hosted_wait(&deps).await;

    // 2. Refusals: missing CA
    let missing_ca_root = crate::test_support::root("healthy");
    let (missing_deps, _restic) = prepared(
        missing_ca_root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    fs::remove_file(missing_ca_root.path().join("link/ca/cert.pem")).unwrap();
    let (status, body) = post_json(&missing_deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 500);
    assert_eq!(body["reason_code"], "internal_error");
    assert!(missing_deps.operations.lock().unwrap().current.is_none());

    // Corrupt CA
    let corrupt_ca_root = crate::test_support::root("healthy");
    let (corrupt_deps, _restic) = prepared(
        corrupt_ca_root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    fs::write(
        corrupt_ca_root.path().join("link/ca/cert.pem"),
        b"not a pem",
    )
    .unwrap();
    let (status, body) = post_json(&corrupt_deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 500);
    assert_eq!(body["reason_code"], "internal_error");
    assert!(corrupt_deps.operations.lock().unwrap().current.is_none());

    // Missing state
    let missing_state_root = crate::test_support::root("healthy");
    let (missing_state_deps, _restic) = prepared(
        missing_state_root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    fs::remove_file(missing_state_root.path().join("link/state.json")).unwrap();
    let (status, body) = post_json(&missing_state_deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 500);
    assert_eq!(body["reason_code"], "internal_error");
    assert!(
        missing_state_deps
            .operations
            .lock()
            .unwrap()
            .current
            .is_none()
    );

    // Mismatched state
    let mismatched_state_root = crate::test_support::root("healthy");
    let (mismatched_state_deps, _restic) = prepared(
        mismatched_state_root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    fs::write(
        mismatched_state_root.path().join("link/state.json"),
        b"{\"instance_id\":\"wrong-instance-id\",\"home_label\":\"Test\"}",
    )
    .unwrap();
    let (status, body) = post_json(&mismatched_state_deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 500);
    assert_eq!(body["reason_code"], "internal_error");
    assert!(
        mismatched_state_deps
            .operations
            .lock()
            .unwrap()
            .current
            .is_none()
    );

    // Fault primitives
    for primitive in [
        HomeReachFaultPrimitive::HeaderJsonSerialization,
        HomeReachFaultPrimitive::ClaimsJsonSerialization,
        HomeReachFaultPrimitive::SigningKeyLoad,
        HomeReachFaultPrimitive::EcdsaSign,
    ] {
        let fault_root = crate::test_support::root("healthy");
        let (fault_deps, _restic) = prepared(
            fault_root.path().to_path_buf(),
            Arc::new(ScriptRunner::with_outputs(vec![])),
        );
        let guard = HomeReachFaultGuard::install(primitive);
        let (status, body) = post_json(&fault_deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 500);
        assert_eq!(body["reason_code"], "internal_error");
        assert!(guard.was_consumed());
        assert!(fault_deps.operations.lock().unwrap().current.is_none());
    }
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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

#[cfg(all(test, feature = "full-tests"))]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn hosted_enable_hides_persisted_binding_until_done_while_credentials_are_held() {
    let root = crate::test_support::root("healthy");
    let journal_path = root.path().to_path_buf();
    disable_backup(journal_path.as_path());
    let hold = Hold::new();
    let runner = ScriptRunner::with_outputs(init_outputs());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(
        HttpScript::with_responses(vec![Ok(credentials_response())])
            .with_hold_for_url_suffix("/backup/credentials", hold.clone()),
    );
    let mut deps = engine_deps(
        journal_path.as_path().to_path_buf(),
        Arc::new(runner),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );

    let teardown_http = Arc::new(HttpScript::with_responses(vec![]));

    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone(), teardown_http.clone()],
        vec![root, restic],
        async {
            let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
            assert_eq!(status, 200);
            let nonce = portal_nonce(&started);
            let (status, _) = post_json(
                &deps,
                "/app/backup/handoff",
                Some(hosted_handoff_payload(&nonce)),
            )
            .await;
            assert_eq!(status, 200);
            wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                .await
                .expect("fixture did not enter Hold");
            assert!(
                http.requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|request| request.url.ends_with("/backup/credentials"))
            );
            assert_eq!(
                solstone_core_backup::load_hosted_binding(journal_path.as_path()),
                Some(crate::test_support::hosted_binding())
            );

            let (status, during) = get_status_json(&deps).await;
            assert_eq!(status, 200);
            assert_eq!(during["operation"]["kind"], "enable_hosted");
            assert_eq!(during["operation"]["phase"], "setting_up");
            assert_eq!(during["hosted"]["bound"], false);
            assert!(during["hosted"].get("bucket").is_none());
            assert!(during["hosted"].get("prefix").is_none());

            hold.release().await;
            let done = wait_terminal(&deps).await;
            assert_eq!(done["operation"]["phase"], "done");
            assert_eq!(done["hosted"]["bound"], true);
            assert_eq!(done["hosted"]["bucket"], "bucket");
            assert_eq!(done["hosted"]["prefix"], "owner/prefix");
            assert_eq!(done["mode"], "operated");
            assert_eq!(done["enabled"], true);

            // Media recorded only in this backup must stop teardown before any request.
            std::fs::create_dir_all(journal_path.join("chronicle/20260101/010000_001")).unwrap();
            solstone_core_offload::append_offload_event(
                journal_path.as_path(),
                "20260101",
                "_default",
                "010000_001",
                "snapshot-a",
                &[solstone_core_retention::archive_catalog::OffloadFile {
                    name: "clip.webm".into(),
                    bytes: 3,
                    sha256: "a".repeat(64),
                }],
                1,
            )
            .unwrap();
            let config_before = std::fs::read(journal_path.join("config/journal.json")).unwrap();
            let binding_before = solstone_core_backup::load_hosted_binding(journal_path.as_path());
            deps.http = teardown_http.clone();
            let (status, teardown) = post_json(&deps, "/app/backup/teardown", None).await;
            assert_eq!(status, 200);
            assert_eq!(teardown["operation"]["kind"], "teardown");
            let refused = wait_terminal(&deps).await;
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                assert_eq!(refused["operation"]["phase"], "error");
                assert_eq!(refused["operation"]["reason_code"], "backup_only_held");
                let report = &refused["operation"]["report"];
                assert_eq!(report["phase"], "preparation");
                assert_eq!(report["refusal"]["kind"], "carrier_preparation");
                assert_eq!(
                    report["preparation_failure"]["refusal"],
                    serde_json::json!({"kind":"reader","reason":"backup_only_held"})
                );
                assert_eq!(report["remote"]["broker_attempted"], false);
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                assert_eq!(refused["operation"]["phase"], "refused");
                assert_eq!(refused["operation"]["report"]["phase"], "prelaunch");
                assert_eq!(
                    refused["operation"]["report"]["refusal"]["kind"],
                    "unsupported_platform"
                );
            }
            assert!(teardown_http.requests.lock().unwrap().is_empty());
            assert_eq!(
                std::fs::read(journal_path.join("config/journal.json")).unwrap(),
                config_before
            );
            assert_eq!(
                solstone_core_backup::load_hosted_binding(journal_path.as_path()),
                binding_before
            );
            assert_eq!(refused["enabled"], true);
            assert!(!crate::operation::is_busy(&deps.operations));
        },
    )
    .await;
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn handoff_needs_subscription_is_terminal_without_binding() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
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

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
        Arc::new(ScriptRunner::with_outputs(vec![])),
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

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn unbound_restore_hosted_returns_portal_and_restoring_phase() {
    let root = crate::test_support::root("fresh");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    let capability = prepare_unbound_restore(&deps).await;
    let keyed = key_unbound_restore(&deps, &capability).await;
    assert_restore_portal_url(keyed["portal_url"].as_str().unwrap());
    let armed = arm_unbound_restore(&deps, &capability).await;
    assert_eq!(armed["operation"]["kind"], "restore_hosted");
    assert_eq!(armed["operation"]["phase"], "restoring");
    let body = activate_unbound_restore(&deps, &capability).await;
    assert_eq!(body["operation"]["kind"], "restore_hosted");
    assert_eq!(body["operation"]["phase"], "restoring");
    assert_restore_portal_url(body["operation"]["portal_url"].as_str().unwrap());
    drain_hosted_wait(&deps).await;
}

#[tokio::test]
async fn unbound_restore_hosted_requires_prepare_flow() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    let (status, body) = post_json(&deps, "/app/backup/restore-hosted", Some(json!({}))).await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_hosted_prepare_required");
}

#[tokio::test]
async fn status_reports_hosted_binding_without_token() {
    let root = crate::test_support::hosted_bound_root();
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    let (status, body) = get_status_json(&deps).await;
    assert_eq!(status, 200);
    assert_eq!(body["hosted"]["bound"], true);
    assert_eq!(body["hosted"]["bucket"], "bucket");
    assert_eq!(body["hosted"]["prefix"], "owner/prefix");
    assert!(body["hosted"].get("broker_token").is_none());
}

#[tokio::test]
async fn offload_status_preserves_checked_coverage_across_distinct_ledgers() {
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
        &[solstone_core_retention::archive_catalog::OffloadFile {
            name: "only.webm".into(),
            bytes: 21,
            sha256: "c".repeat(64),
        }],
        3,
    )
    .unwrap();
    let degraded = crate::test_support::degraded_offload_root();
    let unreadable = crate::test_support::unreadable_offload_root();
    for (journal, local_bytes, unbound_files, is_degraded) in [
        (first.path(), 33, 2, false),
        (second.path(), 0, 1, false),
        (degraded.path(), 33, 2, true),
        (unreadable.path(), 33, 2, true),
    ] {
        let (deps, _restic) = prepared(
            journal.to_path_buf(),
            Arc::new(ScriptRunner::with_outputs(vec![])),
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
        assert_eq!(body["raw_media"]["total_bytes"], local_bytes);
        assert_eq!(body["raw_media"]["complete"], true);
        assert_eq!(body["backup_only"]["unbound_files"], unbound_files);
        assert_eq!(body["backup_only"]["complete"], false);
        assert_eq!(body["backup_only"]["degraded"], is_degraded);
        for key in ["total_bytes", "total_files", "total_segments", "total_days"] {
            assert!(body["backup_only"][key].is_null());
        }
        assert_eq!(body["proposed_release"]["available"], false);
        assert!(body["proposed_release"]["eligible_bytes"].is_null());
        if journal == unreadable.path() {
            assert!(
                !body["backup_only"]["unreadable_ledgers"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod held_restore_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn walk(root: &Path, relative: &Path, state: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            for entry in fs::read_dir(root.join(relative)).unwrap() {
                let entry = entry.unwrap();
                let path = relative.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    state.insert(path.clone(), None);
                    walk(root, &path, state);
                } else {
                    state.insert(path, Some(fs::read(entry.path()).unwrap()));
                }
            }
        }
        let mut state = BTreeMap::new();
        walk(root, Path::new(""), &mut state);
        state
    }

    fn archive(root: &Path, bound: bool) {
        let directory = if bound {
            "010000_001_full"
        } else {
            "010000_001"
        };
        let segment = root.join("chronicle/20260101").join(directory);
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("owner.webm"), b"existing owner media").unwrap();
        if bound {
            let event = json!({
                "event_kind":"offload_archive_v2", "schema_version":2, "time":2,
                "day":"20260101", "stream":"_default", "segment":"010000_001",
                "segment_dir":directory, "layout":"direct",
                "journal_id":"native-root-v1:unix:0000000000000001:0000000000000002",
                "repository_id":"c".repeat(64), "snapshot_id":"b".repeat(64),
                "manifest_id":"manifest", "files":[{
                    "witness_id":"witness", "name":"new.webm", "bytes":8,
                    "sha256":"a".repeat(64), "archive_path":"/stage/new.webm",
                    "processing":{"record":null,"has_analysis_row":true}
                }]
            });
            let ledger = solstone_core_offload::ledger_path_for_day(root, "20260101").unwrap();
            fs::create_dir_all(ledger.parent().unwrap()).unwrap();
            fs::write(ledger, format!("{event}\n")).unwrap();
        } else {
            solstone_core_offload::append_offload_event(
                root,
                "20260101",
                "_default",
                directory,
                "legacy-snapshot",
                &[solstone_core_retention::archive_catalog::OffloadFile {
                    name: "new.webm".into(),
                    bytes: 8,
                    sha256: "a".repeat(64),
                }],
                1,
            )
            .unwrap();
        }
        let catalog = solstone_core_offload::ledger::catalog_journal(root);
        assert!(!catalog.degraded(), "valid held selection fixture");
        let witnesses = &catalog.days[0].segments[0].active;
        assert_eq!(witnesses.len(), 1);
        assert_eq!(witnesses[0].repository_id.is_some(), bound);
    }

    fn missing_tool_deps(
        root: &Path,
    ) -> (crate::BackupWebDeps, Arc<ScriptRunner>, Arc<HttpScript>) {
        let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
        let http = Arc::new(HttpScript::default());
        let deps = engine_deps(
            root.to_path_buf(),
            runner.clone(),
            http.clone(),
            Some(root.join("missing-package/bin/solstone")),
        );
        assert_eq!(
            crate::resolve_tools(&deps).unwrap_err().to_string(),
            "restic_unavailable"
        );
        (deps, runner, http)
    }

    #[tokio::test]
    async fn day_and_all_preserve_preflight_and_owned_tool_refusal() {
        for bound in [false, true] {
            for request in [json!({"day":"20260101"}), json!({"all":true})] {
                let root = crate::test_support::root("healthy");
                archive(root.path(), bound);
                let (deps, runner, http) = missing_tool_deps(root.path());
                let before = snapshot(root.path());
                let (status, value) =
                    post_json(&deps, "/app/backup/offload/restore", Some(request)).await;
                // Historical files without a recorded repository are reviewed
                // inside the same admitted restore, so neither kind refuses early.
                if cfg!(any(
                    target_os = "linux",
                    target_os = "macos",
                    target_os = "windows"
                )) {
                    assert_eq!(status, 200);
                    let terminal = wait_terminal(&deps).await;
                    assert_eq!(terminal["operation"]["kind"], "offload_restore");
                    assert_eq!(terminal["operation"]["phase"], "refused");
                    assert_eq!(terminal["operation"]["reason_code"], "restic_unavailable");
                    assert_eq!(terminal["operation"]["report"]["files_restored"], 0);
                    assert_eq!(terminal["operation"]["report"]["bytes_restored"], 0);
                    assert!(!crate::operation::is_busy(&deps.operations));
                } else {
                    assert_eq!(status, 400);
                    assert_eq!(value["reason_code"], "restore_held");
                    assert!(!crate::operation::is_busy(&deps.operations));
                }
                assert!(runner.calls.lock().unwrap().is_empty());
                assert!(http.requests.lock().unwrap().is_empty());
                assert_eq!(snapshot(root.path()), before);
            }
        }
    }

    #[tokio::test]
    async fn empty_and_malformed_catalogs_preserve_owning_outcomes_without_tools() {
        for (malformed, reason) in [(false, "nothing_to_restore"), (true, "ledger_degraded")] {
            for request in [json!({"day":"20260101"}), json!({"all":true})] {
                let root = crate::test_support::root("healthy");
                if malformed {
                    archive(root.path(), false);
                    let ledger =
                        solstone_core_offload::ledger_path_for_day(root.path(), "20260101")
                            .unwrap();
                    fs::write(ledger, b"not json\n").unwrap();
                    assert!(solstone_core_offload::ledger::catalog_journal(root.path()).degraded());
                }
                let (deps, runner, http) = missing_tool_deps(root.path());
                let before = snapshot(root.path());
                let (status, value) =
                    post_json(&deps, "/app/backup/offload/restore", Some(request)).await;
                assert_eq!(status, 400);
                assert_eq!(
                    value["reason_code"],
                    if cfg!(any(
                        target_os = "linux",
                        target_os = "macos",
                        target_os = "windows"
                    )) {
                        reason
                    } else {
                        "restore_held"
                    }
                );
                assert!(!crate::operation::is_busy(&deps.operations));
                assert!(runner.calls.lock().unwrap().is_empty());
                assert!(http.requests.lock().unwrap().is_empty());
                assert_eq!(snapshot(root.path()), before);
            }
        }
    }

    #[tokio::test]
    async fn ordinary_restore_still_requires_installed_tools() {
        let root = crate::test_support::root("healthy");
        let (deps, runner, http) = missing_tool_deps(root.path());
        let (status, _) = post_json(&deps, "/app/backup/restore", Some(restore_body())).await;
        assert_eq!(status, 200);
        let terminal = wait_terminal(&deps).await;
        assert_eq!(terminal["operation"]["kind"], "restore");
        assert_eq!(terminal["operation"]["phase"], "error");
        assert_eq!(terminal["operation"]["reason_code"], "restic_unavailable");
        assert!(runner.calls.lock().unwrap().is_empty());
        assert!(http.requests.lock().unwrap().is_empty());
    }
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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

#[cfg(feature = "full-tests")]
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

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn bound_restore_hosted_returns_running_then_done_while_held() {
    let root = crate::test_support::hosted_bound_root();
    let journal_path = root.path().to_path_buf();
    let hold = Hold::new();
    let runner = ScriptRunner::with_hold(restore_outputs(), hold.clone());
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::with_responses(vec![Ok(credentials_response())]));
    let deps = engine_deps(
        journal_path.as_path().to_path_buf(),
        Arc::new(runner),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone()],
        vec![root, restic],
        async {
            let (status, body) = post_json(
                &deps,
                "/app/backup/restore-hosted",
                Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
            )
            .await;
            assert_eq!(status, 200);
            assert_eq!(body["operation"]["kind"], "restore_hosted");
            assert_eq!(body["operation"]["phase"], "restoring");
            wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                .await
                .expect("fixture did not enter Hold");
            let (status, during) = get_status_json(&deps).await;
            assert_eq!(status, 200);
            assert_eq!(during["operation"]["phase"], "restoring");
            assert_eq!(during["hosted"]["bound"], true);
            assert_eq!(during["hosted"]["bucket"], "bucket");
            assert_eq!(during["hosted"]["prefix"], "owner/prefix");
            hold.release().await;
            let done = wait_terminal(&deps).await;
            assert_eq!(done["operation"]["phase"], "done");
        },
    )
    .await;
}

#[cfg(all(test, feature = "full-tests"))]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn expired_hosted_wait_clears_busy_on_status_and_begin() {
    let root = crate::test_support::root("healthy");
    let journal_path = root.path().to_path_buf();
    let hold = Hold::new();
    let http = Arc::new(HttpScript::default().with_hold(hold.clone()));
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let deps = engine_deps(
        journal_path.clone(),
        Arc::new(ScriptRunner::with_outputs(init_outputs())),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone()],
        vec![root, restic],
        async {
            let (status, body) = post_json(&deps, "/app/backup/enable-hosted", None).await;
            assert_eq!(status, 200);
            assert_eq!(body["operation"]["phase"], "setting_up");
            let generation = fixture_generation(&deps).await;
            wait_started_or_terminal(&hold, &deps, generation)
                .await
                .expect("fixture did not enter poll Hold");
            let cancellation = http.poll_cancellations.lock().unwrap()[0].clone();
            crate::operation::backdate_started(
                &deps.operations,
                crate::operation::HANDOFF_TTL + Duration::from_secs(1),
            );
            let (status, stopping) = get_status_json(&deps).await;
            assert_eq!(status, 200);
            assert_eq!(stopping["operation"]["phase"], "cancelling");
            assert_eq!(stopping["operation"]["reason_code"], "expired");
            assert!(stopping["operation"]["portal_url"].is_null());
            assert!(cancellation.is_cancelled());
            assert!(crate::operation::is_busy(&deps.operations));
            disable_backup(&journal_path);
            let (status, busy) = post_json(&deps, "/app/backup/enable", None).await;
            assert_eq!(status, 400);
            assert_eq!(busy["reason_code"], "backup_busy");
            assert_eq!(fixture_generation(&deps).await, generation);

            hold.release().await;
            let expired = wait_terminal(&deps).await;
            assert_eq!(expired["operation"]["phase"], "error");
            assert_eq!(expired["operation"]["reason_code"], "expired");
            assert!(!deps.handoff_poll_lease.load(Ordering::Acquire));
            assert_eq!(http.active_executions(), 0);
            let (status, enable) = post_json(&deps, "/app/backup/enable", None).await;
            assert_eq!(status, 200);
            assert_eq!(enable["reason_code"], Value::Null);
            assert_eq!(enable["operation"]["kind"], "enable");
            assert_ne!(fixture_generation(&deps).await, generation);
            let done = wait_terminal(&deps).await;
            assert_eq!(done["operation"]["kind"], "enable");
            assert_eq!(done["operation"]["phase"], "done");
        },
    )
    .await;
}

#[cfg(feature = "full-tests")]
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

#[cfg(feature = "full-tests")]
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

#[cfg(all(test, feature = "full-tests"))]
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

#[cfg(all(test, feature = "full-tests"))]
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

async fn prepare_unbound_restore(deps: &crate::BackupWebDeps) -> String {
    let (status, body) = post_json(deps, "/app/backup/restore-hosted/prepare", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["expires_in_seconds"], 180);
    body["capability"].as_str().unwrap().to_owned()
}

async fn key_unbound_restore(deps: &crate::BackupWebDeps, capability: &str) -> Value {
    let (status, body) = post_json(
        deps,
        "/app/backup/restore-hosted/key",
        Some(json!({
            "capability": capability,
            "recovery_key": crate::test_support::RECOVERY_KEY,
        })),
    )
    .await;
    assert_eq!(status, 200);
    body
}

async fn arm_unbound_restore(deps: &crate::BackupWebDeps, capability: &str) -> Value {
    let (status, body) = post_json(
        deps,
        "/app/backup/restore-hosted/arm",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 200);
    body
}

async fn activate_unbound_restore(deps: &crate::BackupWebDeps, capability: &str) -> Value {
    let (status, body) = post_json(
        deps,
        "/app/backup/restore-hosted/activate",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 200);
    body
}

fn assert_handoff_state_cleared(deps: &crate::BackupWebDeps) {
    let guard = deps.operations.lock().expect("operation slot lock");
    let slot = guard.current.as_ref().expect("operation slot");
    assert!(slot.view.portal_url.is_none());
    assert!(slot.nonce.is_none());
    assert!(slot.restore_key.is_none());
}

#[cfg(feature = "full-tests")]
fn refused_poll_response(reason_code: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![],
        body: serde_json::to_vec(&json!({
            "status": "refused",
            "reason_code": reason_code,
        }))
        .unwrap(),
    }
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn restore_prepare_does_not_allocate_an_enable_instance_id() {
    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );

    crate::operation::reset_instance_allocations();
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    assert_eq!(crate::operation::instance_allocations(), 0);
    let (status, _) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 200);

    let enabled_root = crate::test_support::root("healthy");
    let (enabled_deps, _restic) = prepared(
        enabled_root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    crate::operation::reset_instance_allocations();
    let (status, _) = post_json(&enabled_deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    assert_eq!(crate::operation::instance_allocations(), 0);
    drain_hosted_wait(&enabled_deps).await;
}

#[tokio::test]
async fn restore_prepare_does_not_allocate_identity_before_activation() {
    let root = crate::test_support::root("fresh");
    let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http.clone(),
        None,
    );
    crate::operation::reset_instance_allocations();
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    assert_eq!(crate::operation::instance_allocations(), 0);
    let (status, _) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 200);
    let (view, worker_active) = {
        let guard = deps.operations.lock().unwrap();
        let current = guard.current.as_ref().unwrap();
        (current.view.clone(), current.worker_active)
    };
    assert_eq!(view.phase, "error");
    assert_eq!(view.reason_code.as_deref(), Some("cancelled"));
    assert!(!worker_active);
    assert!(!deps.handoff_poll_lease.load(Ordering::Acquire));
    assert!(runner.calls.lock().unwrap().is_empty());
    assert!(http.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn restore_prepare_rejects_duplicate_prepares_and_reuses_an_expired_lease() {
    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let (_, status_body) = get_status_json(&deps).await;
    assert!(!status_body.to_string().contains(&capability));
    let (status, body) = post_json(&deps, "/app/backup/restore-hosted/prepare", None).await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_unavailable");

    crate::restore_prepare::backdate_restore_prepare_issued_at(
        &deps.restore_prepare,
        crate::restore_prepare::RESTORE_PREPARE_RECLAIM_WINDOW + Duration::from_secs(1),
    );
    let replacement = prepare_unbound_restore(&deps).await;
    assert_ne!(replacement, capability);
    let (status, _) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": replacement})),
    )
    .await;
    assert_eq!(status, 200);
    assert!(crate::operation::current(&deps.operations).is_none());
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted/key",
        Some(json!({
            "capability": replacement,
            "recovery_key": crate::test_support::RECOVERY_KEY,
        })),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_invalid_capability");
}

#[tokio::test]
async fn restore_prepare_key_is_single_use_and_generation_bound() {
    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let barrier = Arc::new(Barrier::new(3));
    let first = {
        let deps = deps.clone();
        let capability = capability.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(post_json(
                    &deps,
                    "/app/backup/restore-hosted/key",
                    Some(json!({
                        "capability": capability,
                        "recovery_key": crate::test_support::RECOVERY_KEY,
                    })),
                ))
        })
    };
    let second = {
        let deps = deps.clone();
        let capability = capability.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(post_json(
                    &deps,
                    "/app/backup/restore-hosted/key",
                    Some(json!({
                        "capability": capability,
                        "recovery_key": crate::test_support::RECOVERY_KEY,
                    })),
                ))
        })
    };
    barrier.wait();
    let mut statuses = [first.join().unwrap().0, second.join().unwrap().0];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 409]);
    assert_eq!(
        crate::operation::current(&deps.operations)
            .as_ref()
            .map(|operation| operation.kind.as_str()),
        Some("restore_hosted")
    );
    let (status, _) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 200);

    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let competing = crate::operation::begin(&deps.operations, "rotate", None, None, None).unwrap();
    crate::operation::finish(&deps.operations, competing.generation, "done", None, None);
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted/key",
        Some(json!({
            "capability": capability,
            "recovery_key": crate::test_support::RECOVERY_KEY,
        })),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_generation_changed");
    let (status, _) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 200);
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn restore_prepare_arm_activate_and_cancel_enforce_the_lifecycle() {
    let root = crate::test_support::root("fresh");
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted/arm",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_wrong_stage");
    let _ = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    let _ = wait_poll_get(&http).await;
    assert_eq!(poll_gets(&http).len(), 1);

    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": "foreign"})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_invalid_capability");
    let (status, cancelled) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 200);
    assert!(matches!(
        cancelled["operation"]["phase"].as_str(),
        Some("cancelling" | "error")
    ));
    assert_eq!(cancelled["operation"]["reason_code"], "cancelled");
    let cancelled = wait_terminal(&deps).await;
    assert_eq!(cancelled["operation"]["phase"], "error");
    assert_eq!(cancelled["operation"]["reason_code"], "cancelled");
    assert_handoff_state_cleared(&deps);
    assert_eq!(credentials_posts(&http), 0);
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_invalid_capability");
}

#[tokio::test]
async fn restore_prepare_cancellation_and_expiry_clear_deferred_restore_state() {
    for stage in ["keyed", "armed"] {
        let root = crate::test_support::root("fresh");
        let http = Arc::new(HttpScript::default());
        let deps = engine_deps(
            root.path().to_path_buf(),
            Arc::new(ScriptRunner::with_outputs(vec![])),
            http.clone(),
            None,
        );
        let capability = prepare_unbound_restore(&deps).await;
        let _ = key_unbound_restore(&deps, &capability).await;
        if stage == "armed" {
            let _ = arm_unbound_restore(&deps, &capability).await;
        }
        let (status, body) = post_json(
            &deps,
            "/app/backup/restore-hosted/cancel",
            Some(json!({"capability": capability})),
        )
        .await;
        assert_eq!(status, 200, "{stage}");
        assert_eq!(body["operation"]["reason_code"], "cancelled", "{stage}");
        assert_handoff_state_cleared(&deps);
        assert!(poll_gets(&http).is_empty(), "{stage}");
        assert_eq!(credentials_posts(&http), 0, "{stage}");
    }

    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    crate::restore_prepare::backdate_restore_prepare_issued_at(
        &deps.restore_prepare,
        crate::restore_prepare::RESTORE_PREPARE_RECLAIM_WINDOW + Duration::from_secs(1),
    );
    let (_, surviving) = get_status_json(&deps).await;
    assert_ne!(surviving["operation"]["phase"], "error");
    assert!(crate::operation::is_busy(&deps.operations));

    crate::restore_prepare::backdate_restore_prepare_issued_at(
        &deps.restore_prepare,
        crate::restore_prepare::RESTORE_PREPARE_CONSENT_WINDOW + Duration::from_secs(1),
    );
    let (_, expired) = get_status_json(&deps).await;
    assert_eq!(expired["operation"]["phase"], "error");
    assert_eq!(
        expired["operation"]["reason_code"],
        "restore_prepare_expired"
    );
    assert_handoff_state_cleared(&deps);
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted/activate",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_invalid_capability");
}

#[tokio::test]
async fn restore_prepare_activated_lease_is_exempt_from_consent_window_expiry() {
    let root = crate::test_support::root("fresh");
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    crate::restore_prepare::backdate_restore_prepare_issued_at(
        &deps.restore_prepare,
        crate::restore_prepare::RESTORE_PREPARE_CONSENT_WINDOW + Duration::from_secs(1),
    );

    let (_, body) = get_status_json(&deps).await;
    assert!(crate::operation::is_busy(&deps.operations));
    assert_ne!(body["operation"]["phase"], "error");
}

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn restore_prepare_activated_lease_is_cleared_once_operation_terminates() {
    let root = crate::test_support::root("fresh");
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    let generation = crate::operation::generation_of(&deps.operations).unwrap();
    crate::operation::finish(&deps.operations, generation, "done", None, None);

    let _ = wait_terminal(&deps).await;
    // A status read can observe completion after its initial lease reconciliation.
    // Exercise cleanup with a poll after the worker has actually settled.
    let (status, _) = get_status_json(&deps).await;
    assert_eq!(status, 200);
    let (status, body) = post_json(
        &deps,
        "/app/backup/restore-hosted/cancel",
        Some(json!({"capability": capability})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["reason_code"], "restore_prepare_invalid_capability");
}

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn restore_prepare_prepare_self_heals_a_terminated_activated_lease() {
    let root = crate::test_support::root("fresh");
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    let generation = crate::operation::generation_of(&deps.operations).unwrap();
    crate::operation::finish(&deps.operations, generation, "done", None, None);

    let _ = wait_terminal(&deps).await;
    let (status, body) = post_json(&deps, "/app/backup/restore-hosted/prepare", None).await;
    assert_eq!(status, 200);
    assert!(body["capability"].as_str().is_some());
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn hosted_restore_refused_poll_outcomes_clear_handoff_state_without_restore_work() {
    for reason_code in ["no_hosted_backup", "hosted_backup_expired"] {
        let root = crate::test_support::root("fresh");
        let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
        let http = Arc::new(
            HttpScript::default().with_poll_responses(vec![Ok(refused_poll_response(reason_code))]),
        );
        let deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            None,
        );
        let capability = prepare_unbound_restore(&deps).await;
        let _ = key_unbound_restore(&deps, &capability).await;
        let _ = arm_unbound_restore(&deps, &capability).await;
        let done = activate_unbound_restore(&deps, &capability).await;
        let done = if done["operation"]["phase"] == "refused" {
            done
        } else {
            wait_terminal(&deps).await
        };
        assert_eq!(done["operation"]["phase"], "refused", "{reason_code}");
        assert_eq!(
            done["operation"]["reason_code"], reason_code,
            "{reason_code}"
        );
        assert_handoff_state_cleared(&deps);
        assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
        assert_eq!(credentials_posts(&http), 0, "{reason_code}");
        assert_eq!(runner.calls.lock().unwrap().len(), 0, "{reason_code}");
    }
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn refused_handoff_validation_is_shared_by_poll_and_local_transport() {
    for malformed in [
        json!({"status": "refused"}),
        json!({"status": "refused", "reason_code": "unknown"}),
        json!({"reason_code": "no_hosted_backup"}),
        json!({"status": "refused", "reason_code": "no_hosted_backup", "extra": true}),
    ] {
        let root = crate::test_support::root("fresh");
        let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
        let http = Arc::new(
            HttpScript::default().with_poll_responses(vec![Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::to_vec(&malformed).unwrap(),
            })]),
        );
        let deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            None,
        );
        let capability = prepare_unbound_restore(&deps).await;
        let _ = key_unbound_restore(&deps, &capability).await;
        let _ = arm_unbound_restore(&deps, &capability).await;
        let _ = activate_unbound_restore(&deps, &capability).await;
        let done = wait_terminal(&deps).await;
        assert_eq!(done["operation"]["phase"], "error");
        assert_eq!(done["operation"]["reason_code"], "failed");
        assert_handoff_state_cleared(&deps);
        assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
        assert_eq!(credentials_posts(&http), 0);
        assert_eq!(runner.calls.lock().unwrap().len(), 0);
    }

    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let keyed = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let nonce = keyed["portal_url"]
        .as_str()
        .unwrap()
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let (status, accepted) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(json!({
            "nonce": nonce,
            "status": "refused",
            "reason_code": "no_hosted_backup",
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(accepted["operation"]["phase"], "refused");
    assert_eq!(accepted["operation"]["reason_code"], "no_hosted_backup");
    assert_handoff_state_cleared(&deps);

    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let keyed = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let nonce = keyed["portal_url"]
        .as_str()
        .unwrap()
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let (status, body) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(json!({
            "nonce": nonce,
            "status": "refused",
            "reason_code": "no_hosted_backup",
            "extra": true,
        })),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert_handoff_state_cleared(&deps);
}

#[tokio::test]
async fn malformed_refused_handoff_with_wrong_nonce_leaves_live_restore_unchanged() {
    let root = crate::test_support::root("fresh");
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        Arc::new(HttpScript::default()),
        None,
    );
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let expected = crate::operation::current(&deps.operations).expect("live restore");
    assert_eq!(expected.kind, "restore_hosted");
    assert_eq!(expected.phase, "restoring");
    assert!(expected.portal_url.is_some());

    for malformed in [
        json!({"status": "refused"}),
        json!({"status": "refused", "reason_code": "unknown"}),
        json!({"reason_code": "no_hosted_backup"}),
        json!({"status": "refused", "reason_code": "no_hosted_backup", "extra": true}),
    ] {
        let mut request = malformed;
        request
            .as_object_mut()
            .unwrap()
            .insert("nonce".into(), json!("not-the-real-nonce"));
        let (status, body) = post_json(&deps, "/app/backup/handoff", Some(request)).await;
        assert_eq!(status, 400);
        assert_eq!(body["reason_code"], "invalid_operation_for_state");
        assert_eq!(
            crate::operation::current(&deps.operations),
            Some(expected.clone())
        );
    }
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn restore_handoff_needs_subscription_can_retry_with_a_new_generation() {
    let root = crate::test_support::root("fresh");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(restore_outputs()));
    let http = Arc::new(
        HttpScript::with_responses(vec![Ok(credentials_response())]).with_poll_responses(vec![
            Ok(needs_subscription_poll_body(
                "https://services.solstone.app/services/backup",
            )),
            Ok(approved_poll_response()),
        ]),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let first_capability = prepare_unbound_restore(&deps).await;
    let first_keyed = key_unbound_restore(&deps, &first_capability).await;
    let _ = arm_unbound_restore(&deps, &first_capability).await;
    let _ = activate_unbound_restore(&deps, &first_capability).await;
    let first = wait_terminal(&deps).await;
    assert_eq!(first["operation"]["phase"], "needs_subscription");
    assert_eq!(first["operation"]["reason_code"], Value::Null);
    assert_handoff_state_cleared(&deps);
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
    assert_eq!(credentials_posts(&http), 0);
    assert_eq!(runner.calls.lock().unwrap().len(), 0);

    let second_capability = prepare_unbound_restore(&deps).await;
    let second_keyed = key_unbound_restore(&deps, &second_capability).await;
    assert_ne!(first_keyed["portal_url"], second_keyed["portal_url"]);
    let _ = arm_unbound_restore(&deps, &second_capability).await;
    let _ = activate_unbound_restore(&deps, &second_capability).await;
    let second = wait_terminal(&deps).await;
    assert_eq!(second["operation"]["phase"], "done");
    assert_eq!(
        solstone_core_backup::load_hosted_binding(root.path()),
        Some(crate::test_support::hosted_binding())
    );
    assert_eq!(credentials_posts(&http), 1);

    let malformed_root = crate::test_support::root("fresh");
    let mut malformed = serde_json::from_slice::<Value>(
        &needs_subscription_poll_body("https://services.solstone.app/services/backup").body,
    )
    .unwrap();
    malformed.as_object_mut().unwrap().remove("subscribe_url");
    let malformed_http = Arc::new(HttpScript::default().with_poll_responses(vec![Ok(
        HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&malformed).unwrap(),
        },
    )]));
    let malformed_runner = Arc::new(ScriptRunner::with_outputs(vec![]));
    let malformed_deps = engine_deps(
        malformed_root.path().to_path_buf(),
        malformed_runner.clone(),
        malformed_http.clone(),
        None,
    );
    let capability = prepare_unbound_restore(&malformed_deps).await;
    let _ = key_unbound_restore(&malformed_deps, &capability).await;
    let _ = arm_unbound_restore(&malformed_deps, &capability).await;
    let _ = activate_unbound_restore(&malformed_deps, &capability).await;
    let malformed_done = wait_terminal(&malformed_deps).await;
    assert_eq!(malformed_done["operation"]["phase"], "error");
    assert_eq!(malformed_done["operation"]["reason_code"], "failed");
    assert_handoff_state_cleared(&malformed_deps);
    assert!(solstone_core_backup::load_hosted_binding(malformed_root.path()).is_none());
    assert_eq!(credentials_posts(&malformed_http), 0);
    assert_eq!(malformed_runner.calls.lock().unwrap().len(), 0);
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn bound_and_byo_restore_bypass_hosted_handoff_transport() {
    let bound_root = crate::test_support::hosted_bound_root();
    let bound_restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(bound_restic.path());
    let bound_http = Arc::new(HttpScript::with_responses(vec![Ok(credentials_response())]));
    let bound_deps = engine_deps(
        bound_root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(restore_outputs())),
        bound_http.clone(),
        Some(bound_restic.path().to_path_buf()),
    );
    let (status, _) = post_json(
        &bound_deps,
        "/app/backup/restore-hosted",
        Some(json!({"recovery_key": crate::test_support::RECOVERY_KEY})),
    )
    .await;
    assert_eq!(status, 200);
    let bound_done = wait_terminal(&bound_deps).await;
    assert_eq!(bound_done["operation"]["phase"], "done");
    assert!(poll_gets(&bound_http).is_empty());

    let byo_root = crate::test_support::root("fresh");
    let byo_restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(byo_restic.path());
    let byo_http = Arc::new(HttpScript::default());
    let byo_deps = engine_deps(
        byo_root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(restore_outputs())),
        byo_http.clone(),
        Some(byo_restic.path().to_path_buf()),
    );
    let (status, _) = post_json(&byo_deps, "/app/backup/restore", Some(restore_body())).await;
    assert_eq!(status, 200);
    let byo_done = wait_terminal(&byo_deps).await;
    assert_eq!(byo_done["operation"]["phase"], "done");
    assert!(poll_gets(&byo_http).is_empty());
}

fn credentials_posts(http: &HttpScript) -> usize {
    http.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.url.contains("/backup/credentials"))
        .count()
}

#[cfg(feature = "full-tests")]
fn assert_credentials_requests(http: &HttpScript, expected_scope: Option<&str>) {
    let requests = http
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.url.contains("/backup/credentials"))
        .cloned()
        .collect::<Vec<_>>();
    let Some(expected_scope) = expected_scope else {
        assert!(
            requests.is_empty(),
            "unexpected broker request: {requests:?}"
        );
        return;
    };
    assert_eq!(requests.len(), 1, "broker request count");
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].url,
        format!("{}/backup/credentials", crate::test_support::PORTAL_BASE)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        json!({"scope": expected_scope})
    );
}

#[cfg(feature = "full-tests")]
fn assert_runner_argv(runner: &ScriptRunner, expected: &[Vec<String>]) {
    let calls = runner.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|(_, argv)| argv.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(calls.iter().all(|(program, _)| {
        program
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("restic"))
    }));
    assert_eq!(runner.overrun_count(), 0, "scripted runner overrun");
}

#[cfg(feature = "full-tests")]
fn assert_no_temporary_credentials_in_journal(root: &Path) {
    for sentinel in ["ACCESS", "SECRET", "SESSION"] {
        assert!(
            !journal_contains(root, sentinel),
            "temporary credential sentinel persisted: {sentinel}"
        );
    }
}

#[cfg(feature = "full-tests")]
fn exact_poll_url(nonce: &str) -> String {
    format!(
        "{}/handoff/backup?nonce={nonce}",
        crate::test_support::PORTAL_BASE
    )
}

#[cfg(feature = "full-tests")]
fn argv_in_order(heads: &[String], expected: &[&str]) {
    let mut start = 0;
    for name in expected {
        let Some(offset) = heads[start..].iter().position(|head| head == *name) else {
            panic!("missing restic command in recorded sequence");
        };
        start += offset + 1;
    }
}

#[cfg(feature = "full-tests")]
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

#[cfg(all(test, feature = "full-tests"))]
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

#[cfg(feature = "full-tests")]
fn assert_handoff_refused(status: u16, body: &Value) {
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_operation_for_state");
    assert_eq!(
        body["error"],
        "that action isn't available in the current state."
    );
    assert_eq!(body["detail"], "");
}

#[cfg(feature = "full-tests")]
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

#[cfg(feature = "full-tests")]
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

#[cfg(feature = "full-tests")]
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

#[cfg(feature = "full-tests")]
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

#[cfg(all(test, feature = "full-tests"))]
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

#[cfg(all(test, feature = "full-tests"))]
fn poll_json(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: serde_json::to_vec(&body).unwrap(),
    }
}

#[cfg(feature = "full-tests")]
async fn wait_poll_get(http: &HttpScript) -> HttpRequest {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
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
        fixture_tick(deadline)
            .await
            .expect("handoff poll GET did not arrive");
    }
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

#[cfg(all(test, feature = "full-tests"))]
async fn wait_poll_gets(http: &HttpScript, min: usize) -> Vec<HttpRequest> {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        let gets = poll_gets(http);
        if gets.len() >= min {
            return gets;
        }
        fixture_tick(deadline)
            .await
            .expect("handoff poll GET did not arrive");
    }
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn hosted_poll_gets_handoff_backup_nonce_without_instance() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
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

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn hosted_poll_configured_portal_base_preflight_blocks_or_allows_transport() {
    struct Case {
        name: &'static str,
        portal_base: &'static str,
        poll_broker: &'static str,
        blocked: bool,
    }
    let cases = [
        Case {
            name: "http scheme",
            portal_base: "http://services.solstone.app",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "missing authority",
            portal_base: "https://",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "userinfo",
            portal_base: "https://user:pass@services.solstone.app",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "non-root path",
            portal_base: "https://services.solstone.app/backup",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "query",
            portal_base: "https://services.solstone.app?x=1",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "fragment",
            portal_base: "https://services.solstone.app#f",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "repeated trailing slash",
            portal_base: "https://services.solstone.app//",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "out-of-range port",
            portal_base: "https://services.solstone.app:99999",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: true,
        },
        Case {
            name: "canonical https",
            portal_base: crate::test_support::PORTAL_BASE,
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: false,
        },
        Case {
            name: "one trailing slash",
            portal_base: "https://services.solstone.app/",
            poll_broker: crate::test_support::PORTAL_BASE,
            blocked: false,
        },
        Case {
            name: "http base and http broker",
            portal_base: "http://services.solstone.app",
            poll_broker: "http://services.solstone.app",
            blocked: true,
        },
    ];
    fn approved_poll_with_broker(endpoint: &str) -> HttpResponse {
        let binding = crate::test_support::hosted_binding();
        HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&json!({
                "status": "approved",
                "broker_endpoint": endpoint,
                "account_id": binding.account_id,
                "instance_id": binding.instance_id,
                "bucket": binding.bucket,
                "prefix": binding.prefix,
                "broker_token": binding.broker_token
            }))
            .unwrap(),
        }
    }
    fn assert_no_sentinels(body: &Value) {
        let rendered = body.to_string();
        assert!(!rendered.contains("user:pass"), "{rendered}");
        assert!(!rendered.contains("broker-token-secret"), "{rendered}");
        assert!(
            !rendered.contains(crate::test_support::RECOVERY_KEY),
            "{rendered}"
        );
        assert!(!rendered.contains("ACCESS"), "{rendered}");
        assert!(!rendered.contains("SECRET"), "{rendered}");
        assert!(!rendered.contains("SESSION"), "{rendered}");
        assert!(!rendered.contains("subscribe_url"), "{rendered}");
    }
    for case in cases {
        let root = crate::test_support::root("healthy");
        if !case.blocked {
            disable_backup(root.path());
        }
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let runner = Arc::new(ScriptRunner::with_outputs(if case.blocked {
            vec![]
        } else {
            init_outputs()
        }));
        let http = Arc::new(
            HttpScript::with_responses(vec![Ok(credentials_response())])
                .with_poll_responses(vec![Ok(approved_poll_with_broker(case.poll_broker))]),
        );
        let mut deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            Some(restic.path().to_path_buf()),
        );
        deps.portal_base = case.portal_base.into();
        let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 200, "{}", case.name);
        assert_no_sentinels(&started);
        if case.blocked {
            assert_eq!(started["operation"]["phase"], "error", "{}", case.name);
            assert_eq!(
                started["operation"]["reason_code"], "failed",
                "{}",
                case.name
            );
            assert!(
                !crate::operation::is_busy(&deps.operations),
                "{}",
                case.name
            );
            assert!(
                solstone_core_backup::load_hosted_binding(root.path()).is_none(),
                "{}",
                case.name
            );
            assert_eq!(started["hosted"]["bound"], false, "{}", case.name);
            assert!(poll_gets(&http).is_empty(), "{}", case.name);
            assert_eq!(credentials_posts(&http), 0, "{}", case.name);
            assert_eq!(runner.calls.lock().unwrap().len(), 0, "{}", case.name);
            assert!(
                !started.to_string().contains(case.portal_base),
                "{}",
                case.name
            );
            let (_, status_body) = get_status_json(&deps).await;
            assert_eq!(status_body["operation"]["phase"], "error", "{}", case.name);
            assert_eq!(
                status_body["operation"]["reason_code"], "failed",
                "{}",
                case.name
            );
            assert_eq!(status_body["hosted"]["bound"], false, "{}", case.name);
            assert!(
                !status_body.to_string().contains(case.portal_base),
                "{}",
                case.name
            );
            assert_no_sentinels(&status_body);
            continue;
        }
        assert_eq!(started["operation"]["phase"], "setting_up", "{}", case.name);
        assert!(
            started["operation"]["portal_url"].as_str().is_some(),
            "{}",
            case.name
        );
        let witnessed = wait_poll_get(&http).await;
        let nonce = witnessed
            .url
            .split("nonce=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned();
        let expected_poll = exact_poll_url(&nonce);
        let gets = poll_gets(&http);
        assert!(!gets.is_empty(), "{}", case.name);
        assert!(
            gets.iter()
                .all(|request| request.method == "GET" && request.url == expected_poll),
            "{}",
            case.name
        );
        let done = wait_terminal(&deps).await;
        assert_eq!(done["operation"]["phase"], "done", "{}", case.name);
        assert_eq!(done["hosted"]["bound"], true, "{}", case.name);
        assert_eq!(
            solstone_core_backup::load_hosted_binding(root.path()),
            Some(crate::test_support::hosted_binding()),
            "{}",
            case.name
        );
        assert!(
            !crate::operation::is_busy(&deps.operations),
            "{}",
            case.name
        );
        assert_eq!(credentials_posts(&http), 1, "{}", case.name);
        argv_in_order(&runner.argv_heads(), &["cat", "init", "key"]);
        assert_no_sentinels(&done);
    }

    for portal_base in [
        crate::test_support::PORTAL_BASE,
        "https://services.solstone.app/",
    ] {
        let root = crate::test_support::root("healthy");
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
        let http = Arc::new(
            HttpScript::with_responses(vec![Ok(credentials_response())]).with_poll_responses(vec![
                Ok(needs_subscription_poll_body(&format!(
                    "{}/services/backup",
                    crate::test_support::PORTAL_BASE
                ))),
            ]),
        );
        let mut deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            Some(restic.path().to_path_buf()),
        );
        deps.portal_base = portal_base.into();
        let (status, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
        assert_eq!(status, 200, "{portal_base}");
        assert_no_sentinels(&started);
        let request = wait_poll_get(&http).await;
        let nonce = request
            .url
            .split("nonce=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(request.method, "GET", "{portal_base}");
        assert_eq!(request.url, exact_poll_url(&nonce), "{portal_base}");
        let done = wait_terminal(&deps).await;
        assert_eq!(
            done["operation"]["phase"], "needs_subscription",
            "{portal_base}"
        );
        assert_eq!(done["hosted"]["bound"], false, "{portal_base}");
        assert!(
            solstone_core_backup::load_hosted_binding(root.path()).is_none(),
            "{portal_base}"
        );
        assert!(
            !crate::operation::is_busy(&deps.operations),
            "{portal_base}"
        );
        assert_eq!(poll_gets(&http), vec![request], "{portal_base}");
        assert_credentials_requests(&http, None);
        assert_runner_argv(&runner, &[]);
        assert_no_sentinels(&done);
    }
}

#[cfg(all(test, feature = "full-tests"))]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
    argv_in_order(&runner.argv_heads(), &["cat", "init", "key"]);
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn hosted_poll_needs_subscription_is_terminal_without_binding() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
    let http = Arc::new(HttpScript::default().with_poll_responses(vec![Ok(
        needs_subscription_poll_body("https://services.solstone.app/services/backup"),
    )]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        runner.clone(),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    let (status, _started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    assert_eq!(status, 200);
    let request = wait_poll_get(&http).await;
    let nonce = request
        .url
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "needs_subscription");
    assert_eq!(done["hosted"]["bound"], false);
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
    assert!(!crate::operation::is_busy(&deps.operations));
    let expected_poll = exact_poll_url(&nonce);
    assert_eq!(request.method, "GET");
    assert_eq!(request.url, expected_poll);
    let gets = poll_gets(&http);
    assert!(!gets.is_empty());
    assert!(
        gets.iter()
            .all(|request| request.method == "GET" && request.url == expected_poll)
    );
    assert_eq!(credentials_posts(&http), 0);
    assert_eq!(runner.calls.lock().unwrap().len(), 0);
}

#[cfg(feature = "full-tests")]
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
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
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

#[cfg(all(test, feature = "full-tests"))]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
            name: "http other".into(),
            poll: vec![Err(HttpError::Other)],
            credentials: false,
            init: false,
            phase: "error",
            reason: Some("failed"),
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
        let http = Arc::new(
            HttpScript::with_responses(if case.credentials {
                vec![Ok(credentials_response())]
            } else {
                vec![]
            })
            .with_poll_responses(case.poll),
        );
        let runner = Arc::new(if case.init {
            ScriptRunner::with_outputs(init_outputs())
        } else {
            ScriptRunner::with_outputs(vec![])
        });
        let deps = engine_deps(
            root.path().to_path_buf(),
            runner.clone(),
            http.clone(),
            Some(restic.path().to_path_buf()),
        );
        let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
        let done = wait_terminal(&deps).await;
        assert_eq!(done["operation"]["phase"], case.phase, "{}", case.name);
        match case.reason {
            Some(reason) => assert_eq!(done["operation"]["reason_code"], reason, "{}", case.name),
            None => assert!(done["operation"]["reason_code"].is_null(), "{}", case.name),
        }
        assert!(
            !crate::operation::is_busy(&deps.operations),
            "{}",
            case.name
        );
        if case.phase == "error" {
            assert!(
                solstone_core_backup::load_hosted_binding(root.path()).is_none(),
                "{}",
                case.name
            );
            assert_eq!(credentials_posts(&http), 0, "{}", case.name);
            assert_eq!(runner.calls.lock().unwrap().len(), 0, "{}", case.name);
        }
    }
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn hosted_poll_watchdog_expires_while_get_is_held() {
    let root = crate::test_support::root("healthy");
    let journal_path = root.path().to_path_buf();
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let hold = Hold::new();
    let http = Arc::new(HttpScript::default().with_hold(hold.clone()));
    let deps = engine_deps(
        journal_path.as_path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone()],
        vec![root, restic],
        async {
            let (status, _) = post_json(&deps, "/app/backup/enable-hosted", None).await;
            assert_eq!(status, 200);
            wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                .await
                .expect("fixture did not enter Hold");
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
            wait_watchdog_expired(&deps.operations).await;
            assert!(crate::operation::is_busy(&deps.operations));
            assert_eq!(http.active_executions(), 1);
            hold.release().await;
            let done = wait_terminal(&deps).await;
            assert_eq!(done["operation"]["phase"], "error");
            assert_eq!(done["operation"]["reason_code"], "expired");
        },
    )
    .await;
}

#[cfg(feature = "full-tests")]
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
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert_eq!(done["hosted"]["bound"], false);
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
    let capability = prepare_unbound_restore(&deps).await;
    let keyed = key_unbound_restore(&deps, &capability).await;
    let nonce = keyed["portal_url"]
        .as_str()
        .unwrap()
        .split("nonce=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["mode"], "operated");
    assert_ne!(
        fs::read(root.path().join("config/journal.json")).unwrap(),
        before
    );
    assert_eq!(done["recovery_key_confirmed"], true);
    assert!(done["destination"]["repository"].as_str().is_some());
    assert_eq!(
        solstone_core_backup::load_hosted_binding(root.path()),
        Some(crate::test_support::hosted_binding())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(root.path().join("backup/hosted/binding.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
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

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn hosted_poll_restore_failure_records_without_publishing_destination_or_recovery_state() {
    let root = crate::test_support::root("fresh");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(
        HttpScript::with_responses(vec![Ok(credentials_response())])
            .with_poll_responses(vec![Ok(approved_poll_response())]),
    );
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![
            output(0, journal_catalog()),
            output(1, ""),
        ])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let capability = prepare_unbound_restore(&deps).await;
    let _ = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_ne!(done["operation"]["phase"], "degraded");
    assert_eq!(done["last_restore"]["reason"], "restore_failed");
    assert_eq!(done["destination"]["repository"], Value::Null);
    assert_eq!(done["recovery_key_confirmed"], false);
    assert_ne!(done["mode"], "operated");
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn hosted_poll_stale_generation_cannot_persist_binding() {
    let root = crate::test_support::root("healthy");
    let journal_path = root.path().to_path_buf();
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let hold = Hold::new();
    let http = Arc::new(
        HttpScript::default()
            .with_poll_responses(vec![Ok(approved_poll_response())])
            .with_hold(hold.clone()),
    );
    let deps = engine_deps(
        journal_path.as_path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone()],
        vec![root, restic],
        async {
            let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
            wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                .await
                .expect("fixture did not enter Hold");
            crate::operation::backdate_started(
                &deps.operations,
                crate::operation::HANDOFF_TTL + Duration::from_secs(1),
            );
            wait_watchdog_expired(&deps.operations).await;
            assert!(crate::operation::is_busy(&deps.operations));
            hold.release().await;
            wait_lease_released(&deps, &http).await;
            let expired = wait_terminal(&deps).await;
            assert_eq!(expired["operation"]["phase"], "error");
            assert_eq!(expired["operation"]["reason_code"], "expired");
            let (status, second) = post_json(&deps, "/app/backup/enable-hosted", None).await;
            assert_eq!(status, 200);
            assert_eq!(second["operation"]["kind"], "enable_hosted");
            assert_eq!(second["operation"]["phase"], "setting_up");
            assert_eq!(second["hosted"]["bound"], false);
            assert!(solstone_core_backup::load_hosted_binding(journal_path.as_path()).is_none());
            drain_hosted_wait(&deps).await;
            assert!(solstone_core_backup::load_hosted_binding(journal_path.as_path()).is_none());
        },
    )
    .await;
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn hosted_poll_get_timeout_is_bounded_by_remaining_ttl() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http = Arc::new(HttpScript::default());
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
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

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
        let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
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
        assert!(!crate::operation::is_busy(&deps.operations));
        assert_runner_argv(&runner, &[]);
        assert_credentials_requests(&http, Some("operated"));
        assert_no_temporary_credentials_in_journal(root.path());
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
        assert_eq!(body["hosted"]["bound"], false, "{label}");
        assert!(!body.to_string().contains("broker-token-secret"), "{label}");
    }
    assert_eq!(poll["operation"]["phase"], local["operation"]["phase"]);
    assert_eq!(
        poll["operation"]["reason_code"],
        local["operation"]["reason_code"]
    );
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
#[tokio::test]
async fn hosted_poll_broker_error_matches_local_handoff_broker_error_contract() {
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
        let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
        let mut script = HttpScript::with_responses(vec![Ok(HttpResponse {
            status: 500,
            headers: vec![],
            body: vec![],
        })]);
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
        assert_runner_argv(&runner, &[]);
        assert_credentials_requests(&http, Some("operated"));
        assert_no_temporary_credentials_in_journal(root.path());
        let restic_calls = runner.calls.lock().unwrap().len();
        (
            done,
            solstone_core_backup::load_hosted_binding(root.path()),
            credentials_posts(&http),
            restic_calls,
        )
    }

    let (poll, poll_binding, poll_creds, poll_restic) = run(true, false).await;
    let (local, local_binding, local_creds, local_restic) = run(false, true).await;
    for (label, body, binding, creds, restic) in [
        ("poll", &poll, poll_binding, poll_creds, poll_restic),
        ("local", &local, local_binding, local_creds, local_restic),
    ] {
        assert_eq!(body["operation"]["phase"], "error", "{label}");
        assert_eq!(body["operation"]["reason_code"], "broker_error", "{label}");
        assert_eq!(
            binding,
            Some(crate::test_support::hosted_binding()),
            "{label} persists binding before the broker call"
        );
        assert_ne!(body["mode"], "operated", "{label}");
        assert_eq!(body["enabled"], false, "{label}");
        assert_eq!(body["hosted"]["bound"], false, "{label}");
        assert!(!body.to_string().contains("broker-token-secret"), "{label}");
        assert_eq!(creds, 1, "{label}");
        assert_eq!(restic, 0, "{label}");
    }
    assert_eq!(poll["operation"]["phase"], local["operation"]["phase"]);
    assert_eq!(
        poll["operation"]["reason_code"],
        local["operation"]["reason_code"]
    );
}

#[cfg(feature = "full-tests")]
async fn wait_watchdog_expired(slot: &crate::operation::SharedOperationSlot) {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        let expired = match slot.try_lock() {
            Ok(guard) => guard.current.as_ref().is_some_and(|current| {
                current.view.phase == "cancelling"
                    && current.view.reason_code.as_deref() == Some("expired")
            }),
            Err(TryLockError::WouldBlock) => false,
            Err(TryLockError::Poisoned(_)) => panic!("operation slot poisoned"),
        };
        if expired {
            return;
        }
        fixture_tick(deadline)
            .await
            .expect("watchdog did not expire the slot");
    }
}

#[cfg(feature = "full-tests")]
async fn wait_lease_released(deps: &crate::BackupWebDeps, http: &HttpScript) {
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    loop {
        if http.active_executions() == 0
            && !deps.handoff_poll_lease.load(Ordering::Acquire)
            && !crate::operation::handoff_poll_active(&deps.operations)
        {
            return;
        }
        fixture_tick(deadline)
            .await
            .expect("poll lease did not release");
    }
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn hosted_poll_watchdog_expiry_is_proven_by_direct_slot_lock_not_observation() {
    let root = crate::test_support::root("fresh");
    let journal_path = root.path().to_path_buf();
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let hold = Hold::new();
    let http = Arc::new(
        HttpScript::default()
            .with_poll_responses(vec![Ok(approved_poll_response())])
            .with_hold(hold.clone()),
    );
    let deps = engine_deps(
        journal_path.as_path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone()],
        vec![root, restic],
        async {
            let capability = prepare_unbound_restore(&deps).await;
            let keyed = key_unbound_restore(&deps, &capability).await;
            let nonce = keyed["portal_url"]
                .as_str()
                .unwrap()
                .split("nonce=")
                .nth(1)
                .unwrap()
                .split('&')
                .next()
                .unwrap()
                .to_owned();
            let _ = arm_unbound_restore(&deps, &capability).await;
            let _ = activate_unbound_restore(&deps, &capability).await;
            wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                .await
                .expect("fixture did not enter Hold");
            let expected_poll = exact_poll_url(&nonce);
            let gets = poll_gets(&http);
            assert!(!gets.is_empty());
            assert!(
                gets.iter()
                    .all(|request| request.method == "GET" && request.url == expected_poll)
            );
            {
                let (view, has_nonce, has_key) = {
                    // The live watchdog takes this slot every tick; wait for it.
                    let guard = deps.operations.lock().expect("fixture slot");
                    let slot = guard.current.as_ref().expect("slot");
                    (
                        slot.view.clone(),
                        slot.nonce.is_some(),
                        slot.restore_key.is_some(),
                    )
                };
                assert!(view.portal_url.is_some());
                assert!(has_nonce);
                assert!(has_key);
                assert!(!crate::operation::is_terminal(&view.phase));
            }
            crate::operation::backdate_started(
                &deps.operations,
                crate::operation::HANDOFF_TTL + Duration::from_secs(1),
            );
            wait_watchdog_expired(&deps.operations).await;
            {
                let (view, has_nonce, has_key) = {
                    let guard = deps.operations.try_lock().expect("fixture slot");
                    let slot = guard.current.as_ref().expect("slot");
                    (
                        slot.view.clone(),
                        slot.nonce.is_some(),
                        slot.restore_key.is_some(),
                    )
                };
                assert_eq!(view.phase, "cancelling");
                assert_eq!(view.reason_code.as_deref(), Some("expired"));
                assert!(!crate::operation::is_terminal(&view.phase));
                assert!(view.portal_url.is_none());
                assert!(!has_nonce);
                assert!(!has_key);
            }
            assert_eq!(http.active_executions(), 1);
            hold.release().await;
            wait_lease_released(&deps, &http).await;
            {
                let (view, has_nonce, has_key) = {
                    let guard = deps.operations.try_lock().expect("fixture slot");
                    let slot = guard.current.as_ref().expect("slot");
                    (
                        slot.view.clone(),
                        slot.nonce.is_some(),
                        slot.restore_key.is_some(),
                    )
                };
                assert_eq!(view.phase, "error");
                assert_eq!(view.reason_code.as_deref(), Some("expired"));
                assert!(view.portal_url.is_none());
                assert!(!has_nonce);
                assert!(!has_key);
            }
            assert!(solstone_core_backup::load_hosted_binding(journal_path.as_path()).is_none());
        },
    )
    .await;
}

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn handoff_needs_subscription_missing_field_is_failed() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let nonce = portal_nonce(&started);
    let mut payload = hosted_needs_subscription_payload(&nonce);
    payload.as_object_mut().unwrap().remove("bucket");
    let (status, body) = post_json(&deps, "/app/backup/handoff", Some(payload)).await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "missing_required_field");
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
}

#[cfg(feature = "full-tests")]
#[tokio::test]
async fn hosted_poll_approved_wrong_origin_is_failed() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let http =
        Arc::new(HttpScript::default().with_poll_responses(vec![Ok(wrong_origin_poll_response())]));
    let deps = engine_deps(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
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

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn handoff_approved_wrong_origin_is_rejected() {
    let root = crate::test_support::root("healthy");
    let restic = tempfile::tempdir().unwrap();
    crate::test_support::write_ready_restic(restic.path());
    let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
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
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert!(solstone_core_backup::load_hosted_binding(root.path()).is_none());
    assert_eq!(credentials_posts(&http), 0);
    assert_eq!(runner.calls.lock().unwrap().len(), 0);
}

#[cfg(feature = "full-tests")]
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
            Arc::new(ScriptRunner::with_outputs(vec![])),
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

#[cfg(feature = "full-tests")]
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
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
    assert_eq!(done["hosted"]["bound"], false);
}

#[cfg(feature = "full-tests")]
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
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http,
        Some(restic.path().to_path_buf()),
    );
    let _ = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
}

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn handoff_needs_subscription_rejects_non_https_subscribe_url() {
    let root = crate::test_support::root("healthy");
    let (deps, _restic) = prepared(
        root.path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
    );
    let (_, started) = post_json(&deps, "/app/backup/enable-hosted", None).await;
    let nonce = portal_nonce(&started);
    let mut payload = hosted_needs_subscription_payload(&nonce);
    payload["subscribe_url"] = json!("http://services.solstone.app/services/backup");
    let (status, body) = post_json(&deps, "/app/backup/handoff", Some(payload)).await;
    assert_eq!(status, 400);
    assert_eq!(body["reason_code"], "invalid_request_value");
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(done["operation"]["reason_code"], "failed");
}

#[cfg(feature = "full-tests")]
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
            Arc::new(ScriptRunner::with_outputs(vec![])),
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
        let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
        while crate::operation::is_busy(&deps.operations) {
            fixture_tick(deadline)
                .await
                .expect("terminal operation stayed busy");
        }
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

#[cfg(all(test, feature = "full-tests"))]
#[tokio::test]
async fn hosted_poll_second_get_is_resource_bounded_and_lease_releases_on_completion() {
    let root = crate::test_support::root("healthy");
    let journal_path = root.path().to_path_buf();
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
        journal_path.as_path().to_path_buf(),
        Arc::new(ScriptRunner::with_outputs(vec![])),
        http.clone(),
        Some(restic.path().to_path_buf()),
    );
    held_fixture(
        deps.clone(),
        vec![hold.clone()],
        vec![http.clone()],
        vec![root, restic],
        async {
            let (status, _) = post_json(&deps, "/app/backup/enable-hosted", None).await;
            assert_eq!(status, 200);
            wait_started_or_terminal(&hold, &deps, fixture_generation(&deps).await)
                .await
                .expect("fixture did not enter Hold");
            assert_eq!(http.active_executions(), 1);
            assert_eq!(http.max_concurrency(), 1);
            assert_eq!(http.request_count(), 2);
            assert_eq!(poll_gets(&http).len(), 2);

            crate::operation::backdate_started(
                &deps.operations,
                crate::operation::HANDOFF_TTL + Duration::from_secs(1),
            );
            wait_watchdog_expired(&deps.operations).await;
            assert_eq!(http.active_executions(), 1);
            assert_eq!(http.request_count(), 2);

            let (status, second) = post_json(&deps, "/app/backup/enable-hosted", None).await;
            assert_eq!(status, 400);
            assert_eq!(second["reason_code"], "backup_busy");
            assert_eq!(http.active_executions(), 1);
            assert_eq!(http.max_concurrency(), 1);
            assert_eq!(http.request_count(), 2);
            let second_generation = {
                let (view, generation) = {
                    let guard = deps.operations.try_lock().expect("fixture slot");
                    let slot = guard.current.as_ref().expect("slot");
                    (slot.view.clone(), slot.generation)
                };
                assert_eq!(view.phase, "cancelling");
                assert_eq!(view.reason_code.as_deref(), Some("expired"));
                generation
            };

            hold.release().await;
            wait_lease_released(&deps, &http).await;
            assert_eq!(http.active_executions(), 0);
            assert!(solstone_core_backup::load_hosted_binding(journal_path.as_path()).is_none());
            {
                let (view, generation) = {
                    let guard = deps.operations.try_lock().expect("fixture slot");
                    let slot = guard.current.as_ref().expect("slot");
                    (slot.view.clone(), slot.generation)
                };
                assert_eq!(generation, second_generation);
                assert_eq!(view.phase, "error");
                assert_eq!(view.reason_code.as_deref(), Some("expired"));
            }

            let (status, third) = post_json(&deps, "/app/backup/enable-hosted", None).await;
            assert_eq!(status, 200);
            assert_eq!(third["operation"]["kind"], "enable_hosted");
            assert_eq!(third["operation"]["phase"], "setting_up");
            let _ = wait_poll_gets(&http, 3).await;
            assert_eq!(http.request_count(), 3);
            drain_hosted_wait(&deps).await;
            assert!(solstone_core_backup::load_hosted_binding(journal_path.as_path()).is_none());
        },
    )
    .await;
}

#[cfg(feature = "full-tests")]
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
        let runner = Arc::new(ScriptRunner::with_outputs(vec![]));
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
            assert_eq!(status, 200);
        }
        let done = wait_terminal(&deps).await;
        assert!(!crate::operation::is_busy(&deps.operations));
        assert_runner_argv(&runner, &[]);
        assert_credentials_requests(&http, None);
        assert_no_temporary_credentials_in_journal(root.path());
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

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
        assert_runner_argv(
            &runner,
            &[vec!["cat".into(), "config".into()], vec!["init".into()]],
        );
        assert_credentials_requests(&http, Some("operated"));
        assert_no_temporary_credentials_in_journal(root.path());
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
        assert_eq!(body["hosted"]["bound"], false, "{label}");
        assert_eq!(creds, 1, "{label}");
        assert_eq!(restic, 2, "{label}");
        assert_ne!(body["mode"], "operated", "{label}");
        assert_eq!(body["enabled"], false, "{label}");
        let rendered = body.to_string();
        assert!(!rendered.contains("broker-token-secret"), "{label}");
        assert!(!rendered.contains("ACCESS"), "{label}");
        assert!(!rendered.contains("SECRET"), "{label}");
        assert!(!rendered.contains("SESSION"), "{label}");
    }
}

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
        let restic = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(restic.path());
        let runner = Arc::new(ScriptRunner::with_outputs(vec![
            output(0, journal_catalog()),
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
        let capability = prepare_unbound_restore(&deps).await;
        let keyed = key_unbound_restore(&deps, &capability).await;
        let _ = arm_unbound_restore(&deps, &capability).await;
        let _ = activate_unbound_restore(&deps, &capability).await;
        if handoff_locally {
            let nonce = keyed["portal_url"]
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
        assert!(!crate::operation::is_busy(&deps.operations));
        assert_eq!(done["last_restore"]["status"], "error");
        assert_eq!(done["last_restore"]["reason"], "restore_failed");
        assert_runner_argv(
            &runner,
            &[
                vec!["snapshots".into(), "--json".into()],
                vec![
                    "restore".into(),
                    format!("{JOURNAL_SNAPSHOT_ID}:/original"),
                    "--target".into(),
                    root.path().display().to_string(),
                    "--exclude".into(),
                    "/health/supervisor.process_instance".into(),
                    "--exclude".into(),
                    "/health/callosum.pipe-*".into(),
                    #[cfg(target_os = "linux")]
                    "--exclude".into(),
                    #[cfg(target_os = "linux")]
                    "backup-activity.lock".into(),
                    "--json".into(),
                ],
            ],
        );
        assert_credentials_requests(&http, Some("maintenance"));
        assert_no_temporary_credentials_in_journal(root.path());
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
        assert_eq!(
            body["operation"]["reason_code"], "restore_failed",
            "{label}"
        );
        assert_ne!(body["operation"]["phase"], "degraded", "{label}");
        assert_eq!(
            binding,
            Some(crate::test_support::hosted_binding()),
            "{label}"
        );
        assert_eq!(creds, 1, "{label}");
        assert_eq!(restic, 2, "{label}");
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

#[cfg(feature = "full-tests")]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
    let capability = prepare_unbound_restore(&deps).await;
    let keyed = key_unbound_restore(&deps, &capability).await;
    let _ = arm_unbound_restore(&deps, &capability).await;
    let _ = activate_unbound_restore(&deps, &capability).await;
    let nonce = keyed["portal_url"]
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
    let done = wait_terminal(&deps).await;
    assert_eq!(done["operation"]["phase"], "done");
    assert_eq!(done["mode"], "operated");
    assert_eq!(done["recovery_key_confirmed"], true);
    assert_eq!(done["destination"]["backend"], "s3");
    assert_eq!(done["destination"]["credentials_set"], true);
    assert_eq!(
        done["destination"]["repository"],
        "s3:https://s3.example/bucket/owner/prefix"
    );
    let rendered = done.to_string();
    assert!(!rendered.contains(crate::test_support::RECOVERY_KEY));
    assert!(!rendered.contains("broker-token-secret"));
    argv_in_order(&runner.argv_heads(), &["snapshots", "restore", "check"]);
    let (status, body) = post_json(
        &deps,
        "/app/backup/handoff",
        Some(hosted_handoff_payload(&nonce)),
    )
    .await;
    assert_handoff_refused(status, &body);
}

#[cfg(all(test, feature = "full-tests"))]
// This fixture admits a Unix bin/lib payload; Windows requires verified native workers.
#[cfg(unix)]
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
            bound: false,
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
            ScriptRunner::with_outputs(vec![])
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

#[test]
fn invalidated_geometry_is_measured_again() {
    let root = tempfile::tempdir().unwrap();
    let cache = crate::measurement::new(root.path());
    assert!(
        crate::measurement::snapshot(&cache)["total_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    root.close().unwrap();
    // The cached reading remains stable until expiry or explicit invalidation.
    assert!(crate::measurement::snapshot(&cache)["total_bytes"].is_number());
    crate::measurement::invalidate(&cache);
    assert!(crate::measurement::snapshot(&cache)["total_bytes"].is_null());
}
