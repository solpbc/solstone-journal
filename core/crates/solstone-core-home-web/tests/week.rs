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
    solstone_core_home_web::routes(
        root.to_path_buf(),
        clock,
        solstone_core_convey_shell::source_link::reference_is_moment,
    )
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
    let (status, content_type, _, body) = get(router, "/app/home/week/20260809").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/html; charset=utf-8");
    let html = String::from_utf8(body).expect("utf8");
    assert!(html.contains("<!DOCTYPE html>"));
}

#[tokio::test]
async fn week_api_serves_valid_page_model() {
    let journal = seeded_root();
    let router = test_router(journal.path());
    let (status, content_type, _, body) = get(router, "/app/home/api/week/20260809").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    let model: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(model["day"], "20260809");
    assert_eq!(model["title"], "week of august 9");
    assert_eq!(model["cells"].as_array().expect("cells").len(), 7);
    assert_eq!(model["from_line"], "from monday.");
    let rows = model["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["day_label"], "mon 10");
}

#[tokio::test]
async fn week_containment_and_absence_on_nonexistent_root() {
    let temp = TempDir::new().unwrap();
    let nonexistent_journal = temp.path().join("nonexistent_journal_root");
    let router = test_router(&nonexistent_journal);

    for stem in [
        "%2E%2E",
        "%2E%2E%5Cx",
        "%252e%252e",
        "%25252e%25252e",
        "20260231",
        "99999999",
    ] {
        let shell_url = format!("/app/home/week/{stem}");
        let (status, _, _, body) = get(router.clone(), &shell_url).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{shell_url}");
        assert!(
            String::from_utf8_lossy(&body).contains("your journal won't follow this link."),
            "{shell_url}"
        );
        assert!(!nonexistent_journal.exists());

        let api_url = format!("/app/home/api/week/{stem}");
        let (status, _, _, body) = get(router.clone(), &api_url).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{api_url}");
        assert!(
            String::from_utf8_lossy(&body).contains("your journal won't follow this link."),
            "{api_url}"
        );
        assert!(!nonexistent_journal.exists());

        let leave_out_url = format!("/app/home/api/week/{stem}/leave-out");
        let (status, _, body) = post(
            router.clone(),
            &leave_out_url,
            json!({ "key": "k1", "undo": false }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{leave_out_url}");
        assert!(
            String::from_utf8_lossy(&body).contains("your journal won't follow this link."),
            "{leave_out_url}"
        );
        assert!(!nonexistent_journal.exists());
    }

    // Sunday 20260308 on nonexistent root -> Absent
    let (status, _, _, body) = get(router.clone(), "/app/home/week/20260308").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("it isn't in your journal."));
    assert!(!nonexistent_journal.exists());

    let (status, _, _, body) = get(router.clone(), "/app/home/api/week/20260308").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("it isn't in your journal."));
    assert!(!nonexistent_journal.exists());

    let (status, _, body) = post(
        router,
        "/app/home/api/week/20260308/leave-out",
        json!({ "key": "k1", "undo": false }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "couldn't leave this out. nothing changed.");
    assert!(!nonexistent_journal.exists());
}

#[tokio::test]
async fn week_untrusted_files_matrix() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 8, 14, 12, 0, 0).unwrap();
    let sun = "20260308";

    let test_cases = [
        // 1. Broken JSON syntax -> CouldntCheck (500)
        (
            "not valid json {",
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 2. Missing version -> CouldntCheck (500)
        (
            r#"{"days":[]}"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 3. Version string "1" -> CantShow (404)
        (
            r#"{"version":"1","days":[]}"#,
            StatusCode::NOT_FOUND,
            "your journal can't show this kind of source.",
            "the week of march 8 couldn't be read.",
        ),
        // 4. Version integer 2 -> CantShow (404)
        (
            r#"{"version":2,"days":[]}"#,
            StatusCode::NOT_FOUND,
            "your journal can't show this kind of source.",
            "the week of march 8 couldn't be read.",
        ),
        // 5. Non-briefing source kind -> CantShow (404)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"memory","memory_id":"m0"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": [
                    {"id":"m0","key":"k0","day":"20260308","text":"T","source":{"kind":"external","uri":"sol://ext"}}
                ]
            }"#,
            StatusCode::NOT_FOUND,
            "your journal can't show this kind of source.",
            "the week of march 8 couldn't be read.",
        ),
        // 6. 6 days in days array -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"nothing_shared"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"}
                ],
                "memories": []
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 7. 8 days in days array -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"nothing_shared"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"},
                    {"day":"20260315","state":"nothing_shared"}
                ],
                "memories": []
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 8. Unknown day state -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"unknown_state"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": []
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 9. Day date mismatch / non-consecutive / day outside week -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"},
                    {"day":"20260315","state":"nothing_shared"}
                ],
                "memories": []
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 10. Shuffled days -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260308","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": []
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 11. Duplicate day in days array -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"nothing_shared"},
                    {"day":"20260308","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": []
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 12. Memory day outside week -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"memory","memory_id":"m0"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": [
                    {"id":"m0","key":"k0","day":"20260315","text":"T","source":{"kind":"briefing","uri":"sol://chronicle/20260315/talents/morning_briefing"}}
                ]
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 13. Memory day != day day -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"memory","memory_id":"m0"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": [
                    {"id":"m0","key":"k0","day":"20260309","text":"T","source":{"kind":"briefing","uri":"sol://chronicle/20260309/talents/morning_briefing"}}
                ]
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 14. Dangling memory ID -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"memory","memory_id":"m_dangling"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": []
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 15. Duplicate memory ID in memories -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"memory","memory_id":"m0"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": [
                    {"id":"m0","key":"k0","day":"20260308","text":"T1","source":{"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}},
                    {"id":"m0","key":"k1","day":"20260308","text":"T2","source":{"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}}
                ]
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
        // 16. Empty key field -> CouldntCheck (500)
        (
            r#"{
                "version": 1,
                "days": [
                    {"day":"20260308","state":"memory","memory_id":"m0"},
                    {"day":"20260309","state":"nothing_shared"},
                    {"day":"20260310","state":"nothing_shared"},
                    {"day":"20260311","state":"nothing_shared"},
                    {"day":"20260312","state":"nothing_shared"},
                    {"day":"20260313","state":"nothing_shared"},
                    {"day":"20260314","state":"nothing_shared"}
                ],
                "memories": [
                    {"id":"m0","key":"","day":"20260308","text":"T","source":{"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}}
                ]
            }"#,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            "the week of march 8 couldn't be read.",
        ),
    ];

    for (idx, (json_str, expected_status, expected_msg, expected_card_line)) in
        test_cases.into_iter().enumerate()
    {
        let temp = TempDir::new().unwrap();
        let weekly_dir = temp.path().join("reflections/weekly");
        fs::create_dir_all(&weekly_dir).unwrap();
        fs::write(weekly_dir.join(format!("{sun}.json")), json_str).unwrap();

        let router = test_router(temp.path());

        // 1. week_shell
        let (status, _, _, body) = get(router.clone(), &format!("/app/home/week/{sun}")).await;
        assert_eq!(status, expected_status, "case {idx} week_shell status");
        assert!(
            String::from_utf8_lossy(&body).contains(expected_msg),
            "case {idx} week_shell body: {}",
            String::from_utf8_lossy(&body)
        );

        // 2. week_api
        let (status, _, _, body) = get(router.clone(), &format!("/app/home/api/week/{sun}")).await;
        assert_eq!(status, expected_status, "case {idx} week_api status");
        assert!(
            String::from_utf8_lossy(&body).contains(expected_msg),
            "case {idx} week_api body: {}",
            String::from_utf8_lossy(&body)
        );

        // 3. card
        let ctx = solstone_core_home::HomeContext::new(temp.path(), now);
        let card_val = solstone_core_home::weekly::card(&ctx);
        assert!(
            card_val["state"] != "week",
            "case {idx} card safely degraded to non-week state: {:?}",
            card_val
        );
        assert_eq!(
            card_val["line"], expected_card_line,
            "case {idx} card line: {:?}",
            card_val
        );
    }

    // 17. Self-symlink .md (no .json present) -> 500 / CouldntCheck & card line: "your week couldn't be checked."
    #[cfg(unix)]
    {
        let temp = TempDir::new().unwrap();
        let weekly_dir = temp.path().join("reflections/weekly");
        fs::create_dir_all(&weekly_dir).unwrap();
        let symlink_path = weekly_dir.join(format!("{sun}.md"));
        std::os::unix::fs::symlink(format!("{sun}.md"), &symlink_path).unwrap();

        let router = test_router(temp.path());
        let (status, _, _, body) = get(router.clone(), &format!("/app/home/week/{sun}")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            String::from_utf8_lossy(&body).contains("your journal couldn't check this source.")
        );

        let (status, _, _, body) = get(router.clone(), &format!("/app/home/api/week/{sun}")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            String::from_utf8_lossy(&body).contains("your journal couldn't check this source.")
        );

        let ctx = solstone_core_home::HomeContext::new(temp.path(), now);
        let card_val = solstone_core_home::weekly::card(&ctx);
        assert_eq!(card_val["state"], "unchecked");
        assert_eq!(card_val["line"], "your week couldn't be checked.");
    }

    // 18. Unreadable reflections/weekly directory -> card line: "your week couldn't be checked."
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let weekly_dir = temp.path().join("reflections/weekly");
        fs::create_dir_all(&weekly_dir).unwrap();
        fs::set_permissions(&weekly_dir, fs::Permissions::from_mode(0o000)).unwrap();

        let ctx = solstone_core_home::HomeContext::new(temp.path(), now);
        let card_val = solstone_core_home::weekly::card(&ctx);
        let _ = fs::set_permissions(&weekly_dir, fs::Permissions::from_mode(0o755));

        assert_eq!(card_val["state"], "unchecked");
        assert_eq!(card_val["line"], "your week couldn't be checked.");
    }
}

#[tokio::test]
async fn week_leave_out_and_undo_cycle_updates_disk_atomically() {
    let journal = seeded_root();
    let router = test_router(journal.path());

    // First fetch the model to get the memory key
    let (status, _, _, body) = get(router.clone(), "/app/home/api/week/20260809").await;
    assert_eq!(status, StatusCode::OK);
    let model: Value = serde_json::from_slice(&body).expect("json");
    let key = model["rows"][0]["key"].as_str().expect("key").to_owned();

    // Leave out
    let (status, _, body) = post(
        router.clone(),
        "/app/home/api/week/20260809/leave-out",
        json!({ "key": key, "undo": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let updated: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(updated["rows"][0]["left_out"], true);
    assert!(updated["left_out_notice"].is_null());

    // Verify disk state
    let left_out_path = journal.path().join("health/week-left-out.json");
    assert!(left_out_path.exists());
    assert!(
        journal
            .path()
            .join("health/week-left-out.json.lock")
            .exists()
    );
    let left_out_disk: Value =
        serde_json::from_str(&fs::read_to_string(&left_out_path).unwrap()).unwrap();
    assert_eq!(left_out_disk["keys"][0], key);

    // Undo leave out
    let (status, _, body) = post(
        router.clone(),
        "/app/home/api/week/20260809/leave-out",
        json!({ "key": key, "undo": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let reverted: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(reverted["rows"][0]["left_out"], false);
    assert!(reverted["left_out_notice"].is_null());
}

#[tokio::test]
async fn week_leave_out_disk_invariants() {
    let journal = seeded_root();
    let router = test_router(journal.path());

    let (status, _, _, body) = get(router.clone(), "/app/home/api/week/20260809").await;
    assert_eq!(status, StatusCode::OK);
    let model: Value = serde_json::from_slice(&body).unwrap();
    let key = model["rows"][0]["key"].as_str().unwrap();

    let week_path = journal.path().join("reflections/weekly/20260809.json");
    let week_bytes_before = fs::read(&week_path).expect("read week json before");

    fn list_files(root: &Path, current: &Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = fs::read_dir(current) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    files.extend(list_files(root, &p));
                } else {
                    files.push(p.strip_prefix(root).unwrap().to_path_buf());
                }
            }
        }
        files.sort();
        files
    }

    let files_before = list_files(journal.path(), journal.path());

    // Perform leave-out
    let (status, _, _) = post(
        router.clone(),
        "/app/home/api/week/20260809/leave-out",
        json!({ "key": key, "undo": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let week_bytes_after = fs::read(&week_path).expect("read week json after");
    assert_eq!(
        week_bytes_before, week_bytes_after,
        "week json bytes unchanged"
    );

    let files_after = list_files(journal.path(), journal.path());
    let new_files: Vec<_> = files_after
        .into_iter()
        .filter(|p| !files_before.contains(p))
        .collect();

    let expected_new = vec![
        std::path::PathBuf::from("health/week-left-out.json"),
        std::path::PathBuf::from("health/week-left-out.json.lock"),
    ];
    assert_eq!(
        new_files, expected_new,
        "only health/week-left-out.json and lock created"
    );

    let health_dir = journal.path().join("health");
    let lock_path = health_dir.join("week-left-out.json.lock");
    let data_path = health_dir.join("week-left-out.json");

    assert!(lock_path.exists(), "lock file exists");
    assert!(data_path.exists(), "data file exists");
    assert!(!health_dir.join("week-left-out.json.lock.lock").exists());

    // Perform undo
    let (status, _, _) = post(
        router,
        "/app/home/api/week/20260809/leave-out",
        json!({ "key": key, "undo": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let week_bytes_after_undo = fs::read(&week_path).expect("read week json after undo");
    assert_eq!(
        week_bytes_before, week_bytes_after_undo,
        "week json bytes unchanged after undo"
    );
    assert!(lock_path.exists(), "lock file remains present after undo");
    assert!(data_path.exists(), "data file remains present after undo");
}

#[tokio::test]
async fn week_peek_moment_and_caption_resolution() {
    let temp = TempDir::new().unwrap();
    let weekly_dir = temp.path().join("reflections/weekly");
    fs::create_dir_all(&weekly_dir).unwrap();

    let json_str = r#"{
        "version": 1,
        "days": [
            {"day":"20260308","state":"memory","memory_id":"m_seg"},
            {"day":"20260309","state":"memory","memory_id":"m_act"},
            {"day":"20260310","state":"memory","memory_id":"m_fallback"},
            {"day":"20260311","state":"nothing_shared"},
            {"day":"20260312","state":"nothing_shared"},
            {"day":"20260313","state":"nothing_shared"},
            {"day":"20260314","state":"nothing_shared"}
        ],
        "memories": [
            {
                "id": "m_seg",
                "key": "k_seg",
                "day": "20260308",
                "text": "Segment text.",
                "source": {
                    "kind": "briefing",
                    "uri": "sol://chronicle/20260308/talents/morning_briefing",
                    "briefing_day": "20260308",
                    "refs": ["sol://facets/work/news/20260308", "sol://20260308/100000_300"]
                }
            },
            {
                "id": "m_act",
                "key": "k_act",
                "day": "20260309",
                "text": "Activity text.",
                "source": {
                    "kind": "briefing",
                    "uri": "sol://chronicle/20260309/talents/morning_briefing",
                    "briefing_day": "20260309",
                    "refs": ["sol://facets/work/activities/20260309#act1"]
                }
            },
            {
                "id": "m_fallback",
                "key": "k_fallback",
                "day": "20260310",
                "text": "Fallback text.",
                "source": {
                    "kind": "briefing",
                    "uri": "sol://chronicle/20260310/talents/morning_briefing",
                    "briefing_day": "20260310",
                    "refs": []
                }
            }
        ]
    }"#;
    fs::write(weekly_dir.join("20260308.json"), json_str).unwrap();

    let model = solstone_core_home::weekly::page_model(
        temp.path(),
        "20260308",
        2026,
        &solstone_core_convey_shell::source_link::reference_is_moment,
    )
    .unwrap();
    let rows = model["rows"].as_array().unwrap();

    // 1. Segment reference selected as moment
    assert_eq!(rows[0]["peek_label"], "open that moment →");
    assert_eq!(
        rows[0]["peek_href"],
        "/source?ref=sol%3A%2F%2F20260308%2F100000_300"
    );
    assert_eq!(
        rows[0]["peek_caption"],
        "from the morning briefing on mon 9"
    );

    // 2. Activity reference selected as moment
    assert_eq!(rows[1]["peek_label"], "open that moment →");
    assert_eq!(
        rows[1]["peek_href"],
        "/source?ref=sol%3A%2F%2Ffacets%2Fwork%2Factivities%2F20260309%23act1"
    );
    assert_eq!(
        rows[1]["peek_caption"],
        "from the morning briefing on tue 10"
    );

    // 3. Fallback when refs is empty -> fallback to source.uri
    assert_eq!(rows[2]["peek_label"], "open it in thinking →");
    assert_eq!(
        rows[2]["peek_href"],
        "/source?ref=sol%3A%2F%2Fchronicle%2F20260310%2Ftalents%2Fmorning_briefing"
    );
    assert_eq!(
        rows[2]["peek_caption"],
        "from the morning briefing on wed 11"
    );
}

#[tokio::test]
async fn week_concurrent_leave_out_requests_persist_all_keys() {
    let journal = TempDir::new().expect("temp journal");
    let weekly_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&weekly_dir).expect("weekly dir");
    fs::write(
        weekly_dir.join("20260809.json"),
        r#"{
            "version": 1,
            "days": [
                {"day": "20260809", "state": "nothing_shared"},
                {"day": "20260810", "state": "memory", "memory_id": "m1"},
                {"day": "20260811", "state": "memory", "memory_id": "m2"},
                {"day": "20260812", "state": "memory", "memory_id": "m3"},
                {"day": "20260813", "state": "nothing_shared"},
                {"day": "20260814", "state": "nothing_shared"},
                {"day": "20260815", "state": "nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m1",
                    "key": "k1_concurrent",
                    "day": "20260810",
                    "text": "Meeting with team.",
                    "source": {
                        "kind": "briefing",
                        "uri": "sol://chronicle/20260810/talents/morning_briefing",
                        "briefing_day": "20260810",
                        "refs": ["sol://20260810/100000_300"]
                    }
                },
                {
                    "id": "m2",
                    "key": "k2_concurrent",
                    "day": "20260811",
                    "text": "Code review.",
                    "source": {
                        "kind": "briefing",
                        "uri": "sol://chronicle/20260811/talents/morning_briefing",
                        "briefing_day": "20260811",
                        "refs": ["sol://20260811/100000_300"]
                    }
                },
                {
                    "id": "m3",
                    "key": "k3_concurrent",
                    "day": "20260812",
                    "text": "Design doc.",
                    "source": {
                        "kind": "briefing",
                        "uri": "sol://chronicle/20260812/talents/morning_briefing",
                        "briefing_day": "20260812",
                        "refs": ["sol://20260812/100000_300"]
                    }
                }
            ],
            "source": {
                "kind": "briefing",
                "uri": "sol://chronicle/20260809/talents/morning_briefing",
                "briefing_day": "20260809",
                "refs": []
            }
        }"#,
    )
    .expect("write week json");

    let router = test_router(journal.path());
    let (status, _, _, body) = get(router.clone(), "/app/home/api/week/20260809").await;
    assert_eq!(status, StatusCode::OK);
    let model: Value = serde_json::from_slice(&body).expect("json");
    let rows = model["rows"].as_array().unwrap();
    let k1 = rows[0]["key"].as_str().unwrap();
    let k2 = rows[1]["key"].as_str().unwrap();
    let k3 = rows[2]["key"].as_str().unwrap();

    let mut handles = Vec::new();
    for key in [k1, k2, k3] {
        let r = router.clone();
        let key = key.to_string();
        handles.push(tokio::spawn(async move {
            post(
                r,
                "/app/home/api/week/20260809/leave-out",
                json!({ "key": key, "undo": false }),
            )
            .await
        }));
    }

    for h in handles {
        let (status, _, _) = h.await.unwrap();
        assert_eq!(status, StatusCode::OK);
    }

    let left_out_path = journal.path().join("health/week-left-out.json");
    let left_out_disk: Value =
        serde_json::from_str(&fs::read_to_string(&left_out_path).unwrap()).unwrap();
    let keys = left_out_disk["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(keys.contains(&k1.to_string()));
    assert!(keys.contains(&k2.to_string()));
    assert!(keys.contains(&k3.to_string()));
}

#[tokio::test]
#[cfg(unix)]
async fn week_leave_out_fails_safely_when_left_out_file_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let journal = seeded_root();
    let router = test_router(journal.path());

    // Fetch model to get valid memory key
    let (status, _, _, body) = get(router.clone(), "/app/home/api/week/20260809").await;
    assert_eq!(status, StatusCode::OK);
    let model: Value = serde_json::from_slice(&body).expect("json");
    let key = model["rows"][0]["key"].as_str().expect("key").to_owned();

    let health_dir = journal.path().join("health");
    fs::create_dir_all(&health_dir).expect("health dir");
    let left_out_path = health_dir.join("week-left-out.json");
    fs::write(&left_out_path, b"{}").expect("write");
    fs::set_permissions(&left_out_path, fs::Permissions::from_mode(0o000)).expect("mode 000");

    let (status, _, body) = post(
        router,
        "/app/home/api/week/20260809/leave-out",
        json!({ "key": key, "undo": false }),
    )
    .await;

    // Reset permissions so TempDir cleanup succeeds
    let _ = fs::set_permissions(&left_out_path, fs::Permissions::from_mode(0o644));

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let json: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["error"], "couldn't leave this out. nothing changed.");

    let lock_path = health_dir.join("week-left-out.json.lock");
    assert!(lock_path.exists(), "lock file exists");
    let record_bytes = fs::read_to_string(&left_out_path).unwrap();
    assert_eq!(record_bytes, "{}", "record bytes still empty object");
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

    let clock = solstone_core_home_web::Clock::fixed(
        chrono::Utc.with_ymd_and_hms(2026, 1, 1, 7, 30, 0).unwrap(),
    );
    let router = solstone_core_home_web::routes(
        journal.path().to_path_buf(),
        clock,
        solstone_core_convey_shell::source_link::reference_is_moment,
    );

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

    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let fixture_json =
        include_str!("../../../../tests/fixtures/journal/reflections/weekly/20260308.json");
    let weekly_dir = root.join("reflections/weekly");
    fs::create_dir_all(&weekly_dir).unwrap();

    let week_0301 = r#"{
        "version": 1,
        "days": [
            {"day": "20260301", "state": "nothing_shared"},
            {"day": "20260302", "state": "nothing_shared"},
            {"day": "20260303", "state": "nothing_shared"},
            {"day": "20260304", "state": "nothing_shared"},
            {"day": "20260305", "state": "nothing_shared"},
            {"day": "20260306", "state": "nothing_shared"},
            {"day": "20260307", "state": "nothing_shared"}
        ],
        "memories": []
    }"#;
    let week_0315 = r#"{
        "version": 1,
        "days": [
            {"day": "20260315", "state": "nothing_shared"},
            {"day": "20260316", "state": "nothing_shared"},
            {"day": "20260317", "state": "nothing_shared"},
            {"day": "20260318", "state": "nothing_shared"},
            {"day": "20260319", "state": "nothing_shared"},
            {"day": "20260320", "state": "nothing_shared"},
            {"day": "20260321", "state": "nothing_shared"}
        ],
        "memories": []
    }"#;
    let week_all_states = r#"{
        "version": 1,
        "days": [
            {"day": "20260322", "state": "memory", "memory_id": "m_active"},
            {"day": "20260323", "state": "memory", "memory_id": "m_left_out"},
            {"day": "20260324", "state": "nothing_shared"},
            {"day": "20260325", "state": "unreadable"},
            {"day": "20260326", "state": "not_ready"},
            {"day": "20260327", "state": "nothing_shared"},
            {"day": "20260328", "state": "nothing_shared"}
        ],
        "memories": [
            {
                "id": "m_active",
                "key": "k_active",
                "day": "20260322",
                "text": "Active memory text with [sol://20260322/100000_300](sol://20260322/100000_300)",
                "source": {
                    "kind": "briefing",
                    "uri": "sol://chronicle/20260322/talents/morning_briefing",
                    "briefing_day": "20260322",
                    "refs": ["sol://20260322/100000_300"]
                }
            },
            {
                "id": "m_left_out",
                "key": "k_left_out",
                "day": "20260323",
                "text": "Left out memory text",
                "source": {
                    "kind": "briefing",
                    "uri": "sol://chronicle/20260323/talents/morning_briefing",
                    "briefing_day": "20260323",
                    "refs": ["sol://20260323/100000_300"]
                }
            }
        ]
    }"#;
    fs::write(weekly_dir.join("20260301.json"), week_0301).unwrap();
    fs::write(weekly_dir.join("20260308.json"), fixture_json).unwrap();
    fs::write(weekly_dir.join("20260315.json"), week_0315).unwrap();
    fs::write(weekly_dir.join("20260322.json"), week_all_states).unwrap();

    let model_primary = solstone_core_home::weekly::page_model(
        root,
        "20260308",
        2026,
        &solstone_core_convey_shell::source_link::reference_is_moment,
    )
    .unwrap();

    let mut left_out_keys = std::collections::BTreeSet::new();
    left_out_keys.insert("b359922341f75193".to_string());
    left_out_keys.insert("k_left_out".to_string());
    fs::create_dir_all(root.join("health")).unwrap();
    fs::write(
        root.join("health/week-left-out.json"),
        &String::from_utf8(solstone_core_home::weekly::left_out_bytes(&left_out_keys)).unwrap(),
    )
    .unwrap();
    let model_left_out = solstone_core_home::weekly::page_model(
        root,
        "20260308",
        2026,
        &solstone_core_convey_shell::source_link::reference_is_moment,
    )
    .unwrap();
    let model_all_states = solstone_core_home::weekly::page_model(
        root,
        "20260322",
        2026,
        &solstone_core_convey_shell::source_link::reference_is_moment,
    )
    .unwrap();

    let models = json!({
        "primary": model_primary,
        "left_out": model_left_out,
        "all_states": model_all_states,
    });

    let models_path = root.join("models.json");
    fs::write(&models_path, serde_json::to_string(&models).unwrap()).unwrap();

    let output = Command::new("node")
        .arg(format!("{}/tests/week_dom.js", env!("CARGO_MANIFEST_DIR")))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .arg(&models_path)
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
