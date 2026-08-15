// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, io::Read, os::unix::net::UnixListener, path::Path, thread};

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
            // The fixture hashes the Python-identical backup.js it predates. the retained compatibility boundary
            // intentionally serves crate-local backup.js bytes, so this is the corpus-level
            // counterpart to assets.rs's deliberate JS byte-identity divergence.
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
    assert_eq!(body.as_object().expect("object").len(), 12);
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

fn listen(root: &Path) -> thread::JoinHandle<String> {
    let health = root.join("health");
    fs::create_dir_all(&health).expect("health");
    let socket = health.join("callosum.sock");
    // The production client looks for the socket at exactly this path, so the
    // test cannot relocate it. But a Unix socket path is capped at SUN_LEN
    // (108 bytes on Linux) and this one is rooted in TMPDIR, so a long TMPDIR
    // makes `bind` fail with a message that names neither TMPDIR nor the cap.
    // Fail with the cause instead: a cryptic red gets attributed to the code
    // under test, which is exactly the wrong conclusion.
    assert!(
        socket.as_os_str().len() < 100,
        "callosum socket path is {} bytes, which will exceed SUN_LEN (108) once bound: {}\n\
         This is the harness, not the code under test. Re-run with a shorter TMPDIR.",
        socket.as_os_str().len(),
        socket.display(),
    );
    let listener = UnixListener::bind(&socket).expect("listener");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection");
        let mut line = String::new();
        stream.read_to_string(&mut line).expect("line");
        line
    })
}

#[tokio::test]
async fn callosum_requests_are_newline_framed_and_verify_is_conditional() {
    let root = crate::test_support::root("fresh");
    let receive = listen(root.path());
    let (status, _) = response_json(
        crate::routes(root.path().to_path_buf()),
        Request::post("/app/backup/backup-now")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let line = receive.join().expect("listener");
    assert!(line.ends_with('\n'));
    assert_eq!(
        serde_json::from_str::<Value>(line.trim()).unwrap(),
        json!({"tract":"supervisor","event":"request","cmd":["journal","maintenance","run","backup:run"]})
    );

    let verify_root = crate::test_support::root("enabled_never_run");
    let receive = listen(verify_root.path());
    let (status, _) = response_json(
        crate::routes_with_cache(verify_root.path().to_path_buf(), corpus_cache()),
        Request::post("/app/backup/offload/enable")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let line = receive.join().expect("listener");
    assert!(line.ends_with('\n'));
    assert_eq!(
        serde_json::from_str::<Value>(line.trim()).unwrap(),
        json!({"tract":"supervisor","event":"request","cmd":["journal","maintenance","run","backup:verify"]})
    );

    let discarded_verify = crate::test_support::root("enabled_never_run");
    let (status, _) = response_json(
        crate::routes_with_cache(discarded_verify.path().to_path_buf(), corpus_cache()),
        Request::post("/app/backup/offload/enable")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status, 200,
        "a missing verify socket is deliberately non-fatal"
    );

    let unavailable = crate::test_support::root("fresh");
    let (status, body) = response_json(
        crate::routes(unavailable.path().to_path_buf()),
        Request::post("/app/backup/backup-now")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(body["reason_code"], "backup_unavailable");
}

#[tokio::test]
async fn engine_routes_validate_then_return_distinct_native_refusals() {
    let destination = json!({"repository":"s3:bucket","backend":"s3","credentials":{"access_key_id":"key","secret_access_key":"secret"}});
    let cases = [
        (
            "/app/backup/enable",
            None,
            crate::refuse::BACKUP_ENABLE_NOT_IMPLEMENTED_NATIVE,
        ),
        (
            "/app/backup/enable-hosted",
            None,
            crate::refuse::BACKUP_ENABLE_HOSTED_NOT_IMPLEMENTED_NATIVE,
        ),
        (
            "/app/backup/destination",
            Some(destination.clone()),
            crate::refuse::BACKUP_DESTINATION_NOT_IMPLEMENTED_NATIVE,
        ),
        (
            "/app/backup/recovery-key/rotate",
            None,
            crate::refuse::BACKUP_RECOVERY_KEY_ROTATE_NOT_IMPLEMENTED_NATIVE,
        ),
        (
            "/app/backup/teardown",
            None,
            crate::refuse::BACKUP_TEARDOWN_NOT_IMPLEMENTED_NATIVE,
        ),
        (
            "/app/backup/restore",
            Some(
                json!({"recovery_key":crate::test_support::RECOVERY_KEY,"repository":"s3:bucket","backend":"s3","credentials":{"access_key_id":"key","secret_access_key":"secret"}}),
            ),
            crate::refuse::BACKUP_RESTORE_NOT_IMPLEMENTED_NATIVE,
        ),
        (
            "/app/backup/restore-hosted",
            Some(json!({"recovery_key":crate::test_support::RECOVERY_KEY})),
            crate::refuse::BACKUP_RESTORE_HOSTED_NOT_IMPLEMENTED_NATIVE,
        ),
        (
            "/app/backup/offload/restore",
            Some(json!({"all":true})),
            crate::refuse::BACKUP_OFFLOAD_RESTORE_NOT_IMPLEMENTED_NATIVE,
        ),
    ];
    for (path, body, reason_code) in cases {
        let root = crate::test_support::root("healthy");
        let (status, response) = response_json(
            crate::routes(root.path().to_path_buf()),
            Request::post(path)
                .body(Body::from(
                    body.map(|value| serde_json::to_vec(&value).unwrap())
                        .unwrap_or_default(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, 501, "{path}");
        assert_eq!(response["reason_code"], reason_code, "{path}");
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
