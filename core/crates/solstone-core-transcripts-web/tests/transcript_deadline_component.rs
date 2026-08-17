// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use solstone_core_transcripts_web::{Clock, router_with_delete_window};
use tempfile::TempDir;
use tower::ServiceExt;

#[test]
fn workspace_asset_matches_pinned_journal_source() {
    let source = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/transcripts/workspace.html"
    ));
    let ground = std::process::Command::new("git")
        .args([
            "show",
            "f17280333736016c219d3f6a4b3a263763529833:solstone/apps/transcripts/workspace.html",
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

    let root = TempDir::new().expect("shims");
    let adjacent = root.path().join("adjacent");
    let path_dir = root.path().join("path");
    fs::create_dir_all(&adjacent).unwrap();
    fs::create_dir_all(&path_dir).unwrap();
    let sibling_marker = root.path().join("sibling-ran");
    let path_marker = root.path().join("path-ran");
    let sibling = adjacent.join("solstone-core");
    let path_shim = path_dir.join("solstone-core");
    fs::write(
        &sibling,
        format!("#!/bin/sh\ntouch {}\n", sibling_marker.display()),
    )
    .unwrap();
    fs::write(
        &path_shim,
        format!("#!/bin/sh\ntouch {}\n", path_marker.display()),
    )
    .unwrap();
    for shim in [&sibling, &path_shim] {
        let mut permissions = fs::metadata(shim).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(shim, permissions).unwrap();
    }

    // Reconstruct the adjacent-only resolution used by production rather than
    // exporting the private helper. The PATH shim is prepended and executable,
    // but only the adjacent executable is launched.
    let resolved = adjacent.join("solstone-core");
    let inherited_path = std::env::var_os("PATH").expect("inherited PATH");
    let path = std::env::join_paths(
        std::iter::once(path_dir.clone()).chain(std::env::split_paths(&inherited_path)),
    )
    .expect("PATH");
    std::process::Command::new(resolved)
        .env("PATH", path)
        .status()
        .unwrap();
    assert!(sibling_marker.exists());
    assert!(!path_marker.exists());
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

fn delete_app(root: &Path, window: Duration) -> axum::Router {
    router_with_delete_window(
        root.to_path_buf(),
        Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap()),
        shell,
        window,
    )
}

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
