// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::TimeZone;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

fn seeded_root() -> TempDir {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/convey_home_seeded_journal");
    let copy = TempDir::new().expect("temporary journal");
    copy_tree(&source, copy.path());
    copy
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination creates");
    for entry in fs::read_dir(source).expect("source reads") {
        let entry = entry.expect("directory entry");
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().expect("entry type");
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("file copies");
        }
    }
}

fn test_router(root: &Path) -> Router {
    let clock = solstone_core_home_web::Clock::fixed(
        chrono::Utc
            .with_ymd_and_hms(2026, 8, 14, 22, 28, 35)
            .unwrap(),
    );
    solstone_core_home_web::routes(root.to_path_buf(), clock, |_| false)
}

async fn get(router: Router, path: &str) -> (StatusCode, String, Option<String>, Vec<u8>) {
    let response = router
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    (status, content_type, location, body)
}

async fn post(router: Router, path: &str, body: Value) -> (StatusCode, String, Vec<u8>) {
    let response = router
        .oneshot(
            Request::post(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json bytes")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    (status, content_type, body)
}

#[tokio::test]
async fn week_shell_serves_shell_html_for_trusted_week() {
    let journal = seeded_root();
    let router = test_router(journal.path());
    let (status, content_type, _, body) = get(router, "/app/home/week/20260810").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/html; charset=utf-8");
    let html = String::from_utf8(body).expect("utf8");
    assert!(html.contains("<!DOCTYPE html>"));
}

#[tokio::test]
async fn week_api_serves_valid_page_model() {
    let journal = seeded_root();
    let router = test_router(journal.path());
    let (status, content_type, _, body) = get(router, "/app/home/api/week/20260810").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    let model: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(model["day"], "20260810");
    assert_eq!(model["title"], "week of august 10");
    assert_eq!(model["cells"].as_array().expect("cells").len(), 7);
    assert_eq!(model["from_line"], "from monday.");
    let rows = model["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["day_label"], "mon 10");
}

#[tokio::test]
async fn week_routes_reject_bad_stem_or_hostile_path() {
    let journal = seeded_root();
    for bad_path in [
        "/app/home/week/notadate",
        "/app/home/week/20269999",
        "/app/home/week/2026",
        "/app/home/api/week/notadate",
        "/app/home/api/week/20269999",
    ] {
        let router = test_router(journal.path());
        let (status, content_type, _, body) = get(router, bad_path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_path}");
        assert_eq!(content_type, "text/html; charset=utf-8", "{bad_path}");
        let html = String::from_utf8(body).expect("utf8");
        assert!(
            html.contains("your journal won't follow this link."),
            "{bad_path}"
        );
    }
}

#[tokio::test]
async fn week_routes_return_cant_open_when_absent_or_invalid() {
    let journal = seeded_root();

    // Absent week
    let router = test_router(journal.path());
    let (status, content_type, _, body) = get(router, "/app/home/api/week/20260101").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(content_type, "text/html; charset=utf-8");
    let html = String::from_utf8(body).expect("utf8");
    assert!(html.contains("it isn't in your journal."));

    // .md only (no .json)
    let md_path = journal.path().join("reflections/weekly/20260706.md");
    fs::create_dir_all(md_path.parent().unwrap()).expect("dir");
    fs::write(&md_path, "# Weekly\nSome text").expect("write md");

    let router = test_router(journal.path());
    let (status, content_type, _, body) = get(router, "/app/home/api/week/20260706").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(content_type, "text/html; charset=utf-8");
    let html = String::from_utf8(body).expect("utf8");
    assert!(html.contains("your journal can't show this kind of source."));
}

#[tokio::test]
async fn week_leave_out_and_undo_cycle_updates_disk_atomically() {
    let journal = seeded_root();
    let router = test_router(journal.path());

    // First fetch the model to get the memory key
    let (status, _, _, body) = get(router.clone(), "/app/home/api/week/20260810").await;
    assert_eq!(status, StatusCode::OK);
    let model: Value = serde_json::from_slice(&body).expect("json");
    let key = model["rows"][0]["key"].as_str().expect("key").to_owned();

    // Leave out
    let (status, _, body) = post(
        router.clone(),
        "/app/home/api/week/20260810/leave-out",
        json!({ "key": key, "undo": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let updated: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(updated["rows"][0]["left_out"], true);
    assert!(updated["left_out_notice"].is_null());

    // Verify disk state
    let left_out_path = journal.path().join("reflections/weekly/left-out.json");
    assert!(left_out_path.exists());
    assert!(
        !journal
            .path()
            .join("reflections/weekly/left-out.json.lock")
            .exists()
    );
    assert!(
        !journal
            .path()
            .join("reflections/weekly/left-out.json.lock.lock")
            .exists()
    );
    let left_out_disk: Value =
        serde_json::from_str(&fs::read_to_string(&left_out_path).unwrap()).unwrap();
    assert_eq!(left_out_disk["keys"][0], key);

    // Undo leave out
    let (status, _, body) = post(
        router.clone(),
        "/app/home/api/week/20260810/leave-out",
        json!({ "key": key, "undo": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let reverted: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(reverted["rows"][0]["left_out"], false);
    assert!(reverted["left_out_notice"].is_null());
}

#[tokio::test]
#[cfg(unix)]
async fn week_leave_out_fails_safely_when_left_out_file_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let journal = seeded_root();
    let router = test_router(journal.path());

    // Fetch model to get valid memory key
    let (status, _, _, body) = get(router.clone(), "/app/home/api/week/20260810").await;
    assert_eq!(status, StatusCode::OK);
    let model: Value = serde_json::from_slice(&body).expect("json");
    let key = model["rows"][0]["key"].as_str().expect("key").to_owned();

    let weekly_dir = journal.path().join("reflections/weekly");
    let left_out_path = weekly_dir.join("left-out.json");
    fs::write(&left_out_path, b"{}").expect("write");
    fs::set_permissions(&left_out_path, fs::Permissions::from_mode(0o000)).expect("mode 000");

    let (status, _, body) = post(
        router,
        "/app/home/api/week/20260810/leave-out",
        json!({ "key": key, "undo": false }),
    )
    .await;

    // Reset permissions so TempDir cleanup succeeds
    let _ = fs::set_permissions(&left_out_path, fs::Permissions::from_mode(0o644));

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let json: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["error"], "couldn't leave this out. nothing changed.");
}

#[tokio::test]
async fn week_api_uses_owner_local_year_for_title() {
    let journal = TempDir::new().expect("temp journal");
    let config_dir = journal.path().join("config");
    fs::create_dir_all(&config_dir).expect("config dir");
    fs::write(
        config_dir.join("journal.json"),
        r#"{"identity":{"timezone":"America/Los_Angeles"}}"#,
    )
    .expect("write config");

    let weekly_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&weekly_dir).expect("weekly dir");
    fs::write(
        weekly_dir.join("20251228.json"),
        r#"{
            "version": 1,
            "days": [
                {"day": "20251228", "state": "nothing_shared"},
                {"day": "20251229", "state": "nothing_shared"},
                {"day": "20251230", "state": "nothing_shared"},
                {"day": "20251231", "state": "nothing_shared"},
                {"day": "20260101", "state": "nothing_shared"},
                {"day": "20260102", "state": "nothing_shared"},
                {"day": "20260103", "state": "nothing_shared"}
            ],
            "memories": [],
            "source": {
                "kind": "briefing",
                "uri": "chronicle/20251228/briefing.json",
                "briefing_day": "20251228",
                "refs": []
            },
            "intro": "no recorded activity this week"
        }"#,
    )
    .expect("write week json");

    // 2026-01-01 07:30:00 UTC is 2025-12-31 23:30:00 in America/Los_Angeles (UTC-8) -> owner year is 2025.
    // Since reflection start year is 2025, it matches owner year -> title: "week of december 28" without year suffix!
    let clock = solstone_core_home_web::Clock::fixed(
        chrono::Utc.with_ymd_and_hms(2026, 1, 1, 7, 30, 0).unwrap(),
    );
    let router = solstone_core_home_web::routes(journal.path().to_path_buf(), clock, |_| false);

    let (status, _, _, body) = get(router, "/app/home/api/week/20251228").await;
    assert_eq!(status, StatusCode::OK);
    let model: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(model["title"], "week of december 28");
}

#[test]
fn week_dom_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("week DOM harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }
    let output = Command::new("node")
        .arg(format!("{}/tests/week_dom.js", env!("CARGO_MANIFEST_DIR")))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("week DOM harness starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "week DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
        "week DOM harness did not report its internal case count:\n{stdout}"
    );
    println!("{}", stdout.trim());
}
