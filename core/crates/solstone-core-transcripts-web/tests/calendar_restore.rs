// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
// Cargo reason: the --lib harness links a second copy through convey-shell, so a restore applied in convey-shell does not touch the harness copy.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use solstone_core_convey_shell::recovery_drain::{pause_admitted_work, register_recovery_hooks};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::whole_journal_restore::{
    begin_target_use_bound, install_target_use_wait_hook,
};
use solstone_core_transcripts_web::{Clock, reset_calendar_while_held};
use tower::ServiceExt;

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path);
        } else if ty.is_file() {
            fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

fn setup_fixture_journal() -> (tempfile::TempDir, Arc<JournalRoot>) {
    let temp = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/journal");
    copy_dir_all(&fixture, temp.path());
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    (temp, root)
}

#[tokio::test]
async fn test_barrier_restore_ordering() {
    let (temp, root) = setup_fixture_journal();

    let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
    let clock = Clock::fixed(now);
    let app = solstone_core_transcripts_web::router(temp.path().to_path_buf(), clock, || {
        axum::response::Response::new(Body::empty())
    });

    // Old day with no stream.updated
    let target_day = "20260101";
    let marker = temp
        .path()
        .join("chronicle")
        .join(target_day)
        .join("health/stream.updated");
    let _ = fs::remove_file(&marker);

    // Seed with complete read first
    let req = Request::get("/app/transcripts/api/index")
        .extension(root.clone())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["freshness"]["complete"], true);
    let old_count = body["months"]["202601"].as_u64().unwrap();

    let _recovery_hooks = register_recovery_hooks(
        "test-barrier-restore-ordering",
        Arc::clone(&root),
        |_| Ok(()),
        |_| Ok(()),
    );

    // Hold admitted use to simulate in-flight read/fold
    let admission = begin_target_use_bound(Arc::clone(&root)).unwrap();

    struct WaitHookGuard;
    impl Drop for WaitHookGuard {
        fn drop(&mut self) {
            install_target_use_wait_hook(None);
        }
    }
    let _hook_guard = WaitHookGuard;

    let (wait_tx, wait_rx) = std::sync::mpsc::channel();
    let wait_tx = Arc::new(Mutex::new(Some(wait_tx)));
    install_target_use_wait_hook(Some(Arc::new(move |_| {
        if let Some(tx) = wait_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    })));

    let root_drain = Arc::clone(&root);
    let pause_thread = std::thread::spawn(move || pause_admitted_work(root_drain));

    // Wait until pause_admitted_work is confirmed waiting for in-flight admitted use
    wait_rx.recv().unwrap();

    // Drop admission so pause_admitted_work proceeds to reset_calendar_while_held
    drop(admission);
    let paused = pause_thread.join().unwrap().unwrap();

    // While paused, restore different files on target day
    let seg = temp.path().join("chronicle/20260101/field/180000_60");
    fs::create_dir_all(&seg).unwrap();
    fs::write(
        seg.join("mic_audio.jsonl"),
        b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"18:00:00\",\"end\":\"18:00:01\",\"text\":\"c_new\"}\n",
    )
    .unwrap();

    paused.resume(&|| false).unwrap();

    let req2 = Request::get("/app/transcripts/api/index")
        .extension(root.clone())
        .body(Body::empty())
        .unwrap();
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let bytes2 = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
    let body2: Value = serde_json::from_slice(&bytes2).unwrap();
    assert_eq!(body2["freshness"]["complete"], true);
    let new_count = body2["months"]["202601"].as_u64().unwrap();
    assert_ne!(new_count, old_count);

    reset_calendar_while_held(temp.path());
}

#[tokio::test]
async fn test_restore_notification() {
    let (temp, root) = setup_fixture_journal();
    let socket = temp.path().join("health").join("callosum.sock");
    fs::create_dir_all(socket.parent().unwrap()).unwrap();
    #[cfg(windows)]
    let _namespace = solstone_core_callosum::test_fixture::WindowsPipeNamespace::register(&socket);
    let server = CallosumSocketServer::bind(&socket).await.unwrap();

    let mut client = CallosumSocketConnection::new(&socket, serde_json::Map::new());
    client.start();

    tokio::time::timeout(Duration::from_secs(5), async {
        while server.client_count() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("client connected to callosum socket server");

    let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
    let app =
        solstone_core_transcripts_web::router(temp.path().to_path_buf(), Clock::fixed(now), || {
            axum::response::Response::new(Body::empty())
        });
    let seeded = app
        .oneshot(
            Request::get("/app/transcripts/api/index")
                .extension(root.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(seeded.status(), StatusCode::OK);
    let seeded_body: Value =
        serde_json::from_slice(&to_bytes(seeded.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(seeded_body["freshness"]["complete"], true);

    let _recovery_hooks = register_recovery_hooks(
        "test-restore-notification",
        Arc::clone(&root),
        |_| Ok(()),
        |_| Ok(()),
    );

    let paused = pause_admitted_work(Arc::clone(&root)).unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), client.next_message())
        .await
        .expect("message should arrive within timeout")
        .expect("callosum message is some");

    assert_eq!(msg.tract, "change");
    assert_eq!(msg.event, "changed");
    assert_eq!(msg.extra.get("kind"), Some(&serde_json::json!("system")));
    assert_eq!(msg.extra.get("key"), Some(&serde_json::json!("calendar")));

    paused.resume(&|| false).unwrap();
    client.stop().await;
    server.stop().await;
    reset_calendar_while_held(temp.path());
}
