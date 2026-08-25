// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use axum::body::Body;
#[cfg(target_os = "linux")]
use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use solstone_core_transcripts_web::{Clock, router_with_delete_window};
use tempfile::TempDir;
use tower::ServiceExt;

#[cfg(unix)]
fn assert_process_group_gone(process_group: i32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(process_group),
            None::<nix::sys::signal::Signal>,
        ) {
            Err(nix::errno::Errno::ESRCH) => return,
            Ok(()) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            result => panic!("copied driver process group remains after cleanup: {result:?}"),
        }
    }
}

#[test]
fn workspace_asset_matches_pinned_native_source() {
    let source = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/transcripts/workspace.html"
    ));
    let ground = std::process::Command::new("git")
        .args([
            "show",
            "2b6fba3ea68944662751eb1f6e47edb38647b2c0:core/crates/solstone-core-transcripts-web/assets/transcripts/workspace.html",
        ])
        .output()
        .unwrap();
    assert!(ground.status.success());
    assert_eq!(source, ground.stdout.as_slice());
}

#[cfg(unix)]
#[test]
fn sense_spawn_resolution_uses_only_the_current_executable_sibling() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    let root = TempDir::new().expect("shims");
    let adjacent = root.path().join("adjacent");
    fs::create_dir_all(&adjacent).unwrap();
    let journal = root.path().join("journal");
    let receipt = root.path().join("sense-argv");
    let driver = adjacent.join("transcript-deadline-driver");
    let sibling = adjacent.join("solstone-core");
    fs::copy(std::env::current_exe().expect("test executable"), &driver).expect("driver copy");
    fs::write(
        &sibling,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$SOLSTONE_SENSE_RECEIPT\"\n",
    )
    .unwrap();
    for executable in [&driver, &sibling] {
        let mut permissions = fs::metadata(executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }

    let mut command = std::process::Command::new(driver);
    command
        .args([
            "--exact",
            "sense_spawn_resolution_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SOLSTONE_TRANSCRIPT_COMPONENT_ROOT", &journal)
        .env("SOLSTONE_SENSE_RECEIPT", &receipt)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command.process_group(0);
    let mut child = command.spawn().expect("copied test driver starts");
    let process_group = i32::try_from(child.id()).expect("driver pid fits i32");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("driver status") {
            break status;
        }
        if Instant::now() >= deadline {
            let killed = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(process_group),
                nix::sys::signal::Signal::SIGKILL,
            );
            if killed.is_err() {
                let _ = child.kill();
            }
            let output = child.wait_with_output().expect("timed-out driver reaps");
            assert_process_group_gone(process_group);
            panic!(
                "copied test driver exceeded its deadline\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let output = child.wait_with_output().expect("driver output");
    assert_process_group_gone(process_group);
    assert!(
        status.success(),
        "copied test driver failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(receipt).expect("sibling argument receipt"),
        "sense\n--day\n20260731\n--segment\n090000_300\n--stream\nfield\n--reprocess\naudio\n"
    );
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "executed by the sibling-resolution parent witness"]
async fn sense_spawn_resolution_child() {
    let root = std::path::PathBuf::from(
        std::env::var_os("SOLSTONE_TRANSCRIPT_COMPONENT_ROOT")
            .expect("component child journal root"),
    );
    write(
        &root,
        "chronicle/20260731/field/090000_300/audio.flac",
        b"raw",
    );
    let response = router_with_delete_window(
        root.clone(),
        Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap()),
        shell,
        Duration::from_secs(1),
    )
    .oneshot(
        Request::builder()
            .method(Method::POST)
            .uri("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"modality":"audio"}"#))
            .unwrap(),
    )
    .await
    .expect("reprocess response");
    assert_eq!(response.status(), StatusCode::OK);
    let failed = root.join("chronicle/20260731/field/090000_300/.analyze_failed_audio");
    tokio::time::timeout(Duration::from_secs(2), async {
        while !failed.is_file() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("production watcher reaps the helper before the deadline");
    let failure: Value =
        serde_json::from_slice(&fs::read(failed).expect("completion marker")).unwrap();
    assert_eq!(failure["reason"], "no_output");
}

#[cfg(target_os = "linux")]
fn this_process_started_at() -> f64 {
    let pid = std::process::id();
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("proc stat");
    let close = stat.rfind(')').expect("comm close");
    let ticks: f64 = stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .expect("start ticks")
        .parse()
        .expect("numeric ticks");
    let boot: f64 = fs::read_to_string("/proc/stat")
        .expect("proc stat")
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .expect("boot time")
        .parse()
        .expect("numeric boot");
    let ticks_per_second: f64 = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .expect("getconf")
        .stdout
        .iter()
        .map(|byte| *byte as char)
        .collect::<String>()
        .trim()
        .parse()
        .expect("clock ticks");
    boot + ticks / ticks_per_second
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn search_index_warning_tracks_the_native_supervisor_identity_contract() {
    let down = deletion_root();
    let down_response = delete_request(delete_app(down.path(), Duration::from_secs(1))).await;
    assert_eq!(down_response["search_index_warning"], true);

    let up = deletion_root();
    write(
        up.path(),
        "health/supervisor.pid",
        std::process::id().to_string().as_bytes(),
    );
    write(
        up.path(),
        "health/supervisor.start_time",
        this_process_started_at().to_string().as_bytes(),
    );
    assert!(solstone_core_system::lifecycle::is_supervisor_up(up.path()));
    let up_response = delete_request(delete_app(up.path(), Duration::from_secs(1))).await;
    assert!(up_response.get("search_index_warning").is_none());
    assert_eq!(
        up_response
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "commit_at_ms".into(),
            "deleted".into(),
            "pending".into(),
            "success".into(),
            "ttl_seconds".into(),
        ])
    );
}

fn shell() -> axum::response::Response {
    axum::response::Response::new(Body::from("shell"))
}

fn write(root: &Path, relative: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(path, contents).expect("file");
}

#[cfg(target_os = "linux")]
fn deletion_root() -> TempDir {
    let root = TempDir::new().expect("journal");
    write(
        root.path(),
        "config/journal.json",
        br#"{"setup":{"completed_at":1700000000000}}"#,
    );
    for (name, contents) in [
        ("audio.flac", b"raw".as_slice()),
        ("audio.jsonl", b"{}\n".as_slice()),
        ("stream.json", b"{}".as_slice()),
        ("talents/sense.json", b"{}".as_slice()),
    ] {
        write(
            root.path(),
            &format!("chronicle/20260731/field/090000_300/{name}"),
            contents,
        );
    }
    root
}

#[cfg(target_os = "linux")]
fn delete_app(root: &Path, window: Duration) -> axum::Router {
    router_with_delete_window(
        root.to_path_buf(),
        Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap()),
        shell,
        window,
    )
}

#[cfg(target_os = "linux")]
async fn delete_request(app: axum::Router) -> Value {
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/app/transcripts/api/segment/20260731/field/090000_300")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
        .expect("delete response")
}
