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
use solstone_core_journal_io::whole_journal_restore::install_target_use_wait_hook;
use solstone_core_transcripts_web::{
    Clock, install_calendar_identity_barrier, release_calendar_identity_barrier,
    reset_calendar_while_held,
};
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

fn write_analyzed_audio(dir: &Path, text: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("mic_audio.jsonl"),
        format!(
            "{{\"_solstone_processing\":{{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}}}\n{{\"start\":\"10:00:00\",\"end\":\"10:00:01\",\"text\":\"{text}\"}}\n"
        ),
    )
    .unwrap();
}

fn write_browser_segment(journal: &Path, day: &str, key: &str) {
    let seg = journal
        .join("chronicle")
        .join(day)
        .join("workstation.browser")
        .join(key);
    fs::create_dir_all(&seg).unwrap();
    fs::write(
        seg.join("stream.json"),
        b"{\"stream\":\"workstation.browser\",\"seq\":1}\n",
    )
    .unwrap();
    fs::write(
        seg.join("browser_page.jsonl"),
        b"{\"t\":\"segment_start\",\"ts\":1783006230000,\"rel\":30000,\"site\":\"example.com\",\"url\":\"https://example.com\",\"title\":\"Test\",\"adapter\":\"docs\",\"ctx\":\"doc\",\"n\":1,\"blocks\":[]}\n",
    )
    .unwrap();
}

#[tokio::test]
async fn test_barrier_restore_ordering() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config/journal.json"),
        br#"{"identity":{"timezone":"UTC"},"convey":{"password_hash":"","secret":"sec","trust_localhost":true}}"#,
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let identity = root.identity();
    let day = "20260101";
    write_analyzed_audio(
        &temp
            .path()
            .join("chronicle")
            .join(day)
            .join("field/100000_60"),
        "seed",
    );

    let now_ref = Arc::new(Mutex::new(
        Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap(),
    ));
    let now_c = now_ref.clone();
    let clock = Clock::new(move || *now_c.lock().unwrap());
    let app = solstone_core_transcripts_web::router(temp.path().to_path_buf(), clock, || {
        axum::response::Response::new(Body::empty())
    });

    let seeded = app
        .clone()
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
    let old_count = seeded_body["months"]["202601"].as_u64().unwrap();

    let _recovery_hooks = register_recovery_hooks(
        "test-barrier-restore-ordering",
        Arc::clone(&root),
        |_| Ok(()),
        |_| Ok(()),
    );

    let (park_tx, park_rx) = std::sync::mpsc::channel();
    let park_tx = Arc::new(Mutex::new(Some(park_tx)));
    let day_s = day.to_string();
    install_calendar_identity_barrier(identity, move |folded| {
        if folded == day_s {
            if let Some(tx) = park_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            true
        } else {
            false
        }
    });

    struct WaitHookGuard;
    impl Drop for WaitHookGuard {
        fn drop(&mut self) {
            install_target_use_wait_hook(None);
        }
    }
    let _hook_guard = WaitHookGuard;

    // The parked read is itself a full pass, so a reset-before-drain can stamp.
    *now_ref.lock().unwrap() += chrono::Duration::hours(2);

    let root_coord = Arc::clone(&root);
    let (paused_tx, paused_rx) = std::sync::mpsc::channel();
    let coordinator = std::thread::spawn(move || {
        park_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("fold parked after its identity read");
        let (wait_tx, wait_rx) = std::sync::mpsc::channel();
        let wait_tx = Arc::new(Mutex::new(Some(wait_tx)));
        install_target_use_wait_hook(Some(Arc::new(move |_| {
            if let Some(tx) = wait_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        })));
        let pause_thread = std::thread::spawn(move || pause_admitted_work(root_coord));
        wait_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("restore is waiting on the parked fold");
        release_calendar_identity_barrier(identity);
        let paused = pause_thread.join().expect("pause thread").expect("pause");
        paused_tx.send(paused).expect("paused result");
    });

    let parked = tokio::time::timeout(Duration::from_secs(5), async {
        app.clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    })
    .await
    .expect("parked index read finished");
    assert_eq!(parked.status(), StatusCode::OK);

    let paused = paused_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("restore drained the parked fold");
    coordinator.join().unwrap();

    // The fold stored the pre-swap count. Swap only after that store and the reset.
    write_browser_segment(temp.path(), day, "235000_60");
    install_calendar_identity_barrier(identity, |_| false);
    paused.resume(&|| false).unwrap();

    let next = app
        .clone()
        .oneshot(
            Request::get("/app/transcripts/api/index")
                .extension(root.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(next.status(), StatusCode::OK);
    let next_body: Value =
        serde_json::from_slice(&to_bytes(next.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(next_body["freshness"]["complete"], true);
    let new_count = next_body["months"]["202601"].as_u64().unwrap();
    assert_ne!(new_count, old_count);
    assert_eq!(new_count, old_count + 1);

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
