// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, Read};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::{Value, json};
use tower::ServiceExt;

const IO_DEADLINE: Duration = Duration::from_secs(2);
const RUN_LINE: &[u8] = b"{\"tract\":\"supervisor\",\"event\":\"request\",\"cmd\":[\"journal\",\"maintenance\",\"run\",\"backup:run\"]}\n";
const VERIFY_LINE: &[u8] = b"{\"tract\":\"supervisor\",\"event\":\"request\",\"cmd\":[\"journal\",\"maintenance\",\"run\",\"backup:verify\"]}\n";

fn write_config(root: &Path, last_verification_status: Value) {
    let config = root.join("config");
    fs::create_dir_all(&config).expect("create config directory");
    fs::write(
        config.join("journal.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "setup": {"completed_at": 1_767_225_600_i64},
                "backup": {
                    "enabled": true,
                    "daily_key": "daily-key",
                    "recovery_key": "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ",
                    "confirmed_recovery_key": true,
                    "offload": {"enabled": false, "budget_bytes": 100, "floor_bytes": 10},
                    "last_verification": {
                        "time": Value::Null,
                        "status": last_verification_status,
                        "reason": Value::Null,
                        "checked_subset": Value::Null,
                        "last_ok_time": Value::Null
                    }
                }
            }))
            .expect("serialize journal config")
        ),
    )
    .expect("write journal config");
}

fn listener(root: &Path) -> UnixListener {
    let health = root.join("health");
    fs::create_dir_all(&health).expect("create health directory");
    let socket = health.join("callosum.sock");
    assert!(
        socket.as_os_str().len() < 100,
        "private Callosum fixture path exceeds the Unix socket path budget: {}",
        socket.display()
    );
    UnixListener::bind(socket).expect("bind private Callosum listener")
}

fn accept_exact(listener: &UnixListener, expected: &[u8]) {
    listener
        .set_nonblocking(true)
        .expect("make the already-ready accept bounded");
    let (mut stream, _) = listener.accept().expect("accept queued request");
    stream
        .set_nonblocking(false)
        .expect("make accepted request stream blocking");
    stream
        .set_read_timeout(Some(IO_DEADLINE))
        .expect("bound request read");
    let mut received = Vec::new();
    stream
        .read_to_end(&mut received)
        .expect("read request through sender EOF");
    assert_eq!(received, expected);
    let mut after_eof = [0_u8; 1];
    assert_eq!(stream.read(&mut after_eof).expect("re-read EOF"), 0);
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

fn assert_no_request(listener: &UnixListener) {
    listener
        .set_nonblocking(true)
        .expect("make absence check bounded");
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

async fn post(root: &Path, route: &str) -> (u16, Value) {
    let response = solstone_core_backup_web::routes(root.to_path_buf())
        .oneshot(
            Request::post(route)
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("route response");
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    (
        status,
        serde_json::from_slice(&body).expect("JSON response body"),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn backup_and_conditional_verify_are_write_only_newline_requests() {
    let run_root = tempfile::tempdir().expect("run journal");
    write_config(run_root.path(), json!("ok"));
    let run_listener = listener(run_root.path());
    let (status, body) = post(run_root.path(), "/app/backup/backup-now").await;
    assert_eq!(status, 200);
    assert_eq!(body["success"], true);
    accept_exact(&run_listener, RUN_LINE);

    let verify_root = tempfile::tempdir().expect("verify journal");
    write_config(verify_root.path(), Value::Null);
    let verify_listener = listener(verify_root.path());
    let (status, body) = post(verify_root.path(), "/app/backup/offload/enable").await;
    assert_eq!(status, 200);
    assert_eq!(body["success"], true);
    accept_exact(&verify_listener, VERIFY_LINE);

    let already_verified = tempfile::tempdir().expect("already verified journal");
    write_config(already_verified.path(), json!("ok"));
    let quiet_listener = listener(already_verified.path());
    let (status, body) = post(already_verified.path(), "/app/backup/offload/enable").await;
    assert_eq!(status, 200);
    assert_eq!(body["success"], true);
    assert_no_request(&quiet_listener);

    let best_effort_verify = tempfile::tempdir().expect("best-effort verify journal");
    write_config(best_effort_verify.path(), Value::Null);
    let (status, body) = post(best_effort_verify.path(), "/app/backup/offload/enable").await;
    assert_eq!(status, 200, "verify transport remains best-effort");
    assert_eq!(body["success"], true);

    let unavailable = tempfile::tempdir().expect("unavailable journal");
    write_config(unavailable.path(), json!("ok"));
    let (status, body) = post(unavailable.path(), "/app/backup/backup-now").await;
    assert_eq!(status, 503);
    assert_eq!(body["reason_code"], "backup_unavailable");
}
