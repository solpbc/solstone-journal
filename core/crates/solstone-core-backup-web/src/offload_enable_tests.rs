// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use super::*;
use axum::body::to_bytes;
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixture() -> (tempfile::TempDir, BackupWebDeps) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/journal.json"),
        serde_json::to_vec(&json!({
            "owner_choice":"retained",
            "backup":{
                "enabled":true,"confirmed_recovery_key":true,
                "daily_key":"fixture-daily","recovery_key":"fixture-recovery",
                "offload":{"enabled":false,"budget_bytes":1234,"floor_bytes":567},
                "last_backup":{"status":"error","error_reason":"incomplete_copy","time":42},
                "last_offload":{"status":"error","last_ok_time":17},
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(dir.path()).unwrap());
    let deps = BackupWebDeps::with_root(root).unwrap();
    (dir, deps)
}

fn config_at(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("config/journal.json")).unwrap()).unwrap()
}

async fn payload(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn enabling_submits_after_write_reservation_and_preserves_observations() {
    let (dir, mut deps) = fixture();
    let submitted = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&submitted);
    let operations = Arc::clone(&deps.operations);
    deps.verification_submitter = Some(Arc::new(move || {
        let _maintenance = admit_write(&operations, AdmitWrite::Maintenance)
            .expect("verification can acquire its maintenance reservation");
        observed.fetch_add(1, Ordering::SeqCst);
        Ok("accepted-fixed-verify".into())
    }));
    let before = config_at(dir.path());
    assert_eq!(
        crate::get_offload(deps.clone()).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        submitted.load(Ordering::SeqCst),
        0,
        "GET must not submit work"
    );
    let response = crate::offload_enable(deps).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = payload(response).await;
    assert_eq!(
        body["verification_request"],
        json!({"state":"accepted","ref":"accepted-fixed-verify"})
    );
    assert_eq!(submitted.load(Ordering::SeqCst), 1);
    let after = config_at(dir.path());
    assert_eq!(after["owner_choice"], before["owner_choice"]);
    assert_eq!(
        after["backup"]["last_backup"],
        before["backup"]["last_backup"]
    );
    assert_eq!(
        after["backup"]["last_offload"],
        before["backup"]["last_offload"]
    );
    assert_eq!(
        after["backup"]["offload"],
        json!({"enabled":true,"budget_bytes":1234,"floor_bytes":567})
    );
    assert!(
        after["backup"].get("last_verification").is_none(),
        "admission is not a verification result"
    );
}

#[tokio::test]
async fn historical_verification_is_preserved_without_an_automatic_new_attempt() {
    let (dir, mut deps) = fixture();
    let mut config = config_at(dir.path());
    config["backup"]["last_verification"] =
        json!({"status":"error","reason":"read_data_mismatch","last_ok_time":12});
    std::fs::write(
        dir.path().join("config/journal.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    deps.verification_submitter =
        Some(Arc::new(|| panic!("history must not trigger verification")));
    let response = crate::offload_enable(deps).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = payload(response).await;
    assert_eq!(body["verification_request"]["state"], "not_needed");
    assert_eq!(
        config_at(dir.path())["backup"]["last_verification"],
        config["backup"]["last_verification"]
    );
}

#[tokio::test]
async fn refused_submission_keeps_the_actual_config_publication_explicit() {
    let (dir, mut deps) = fixture();
    deps.verification_submitter = Some(Arc::new(|| Err("queue unavailable")));
    let response = crate::offload_enable(deps).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = payload(response).await;
    assert_eq!(body["configuration_publication"]["state"], "published");
    assert_eq!(body["configuration_publication"]["confirmed"], true);
    assert_eq!(body["verification_request"]["state"], "refused");
    assert_eq!(config_at(dir.path())["backup"]["offload"]["enabled"], true);
}

#[test]
fn config_drift_refuses_without_overwriting_a_new_owner_choice() {
    let (dir, deps) = fixture();
    let snapshot = EnableSnapshot::read(deps.retained_root().unwrap()).unwrap();
    let mut next = config_at(dir.path());
    next["owner_choice"] = json!("newer");
    std::fs::write(
        dir.path().join("config/journal.json"),
        serde_json::to_vec(&next).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        snapshot.commit(&Value::Null, &|| Ok(())),
        Err(BoundConfigRefusal::Conflict)
    ));
    assert_eq!(config_at(dir.path()), next);
}

#[test]
fn replacing_config_parent_after_read_cannot_redirect_the_commit() {
    let (dir, deps) = fixture();
    let snapshot = EnableSnapshot::read(deps.retained_root().unwrap()).unwrap();
    let original = config_at(dir.path());
    std::fs::rename(
        dir.path().join("config"),
        dir.path().join("original-config"),
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/journal.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    assert!(snapshot.commit(&Value::Null, &|| Ok(())).is_err());
    assert_eq!(config_at(dir.path()), original);
    let old: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("original-config/journal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(old, original);
}

#[test]
fn replacing_root_after_read_cannot_redirect_the_commit() {
    let parent = tempfile::tempdir().unwrap();
    let root_path = parent.path().join("owner");
    std::fs::create_dir(&root_path).unwrap();
    let (fixture, _deps) = fixture();
    std::fs::rename(fixture.path().join("config"), root_path.join("config")).unwrap();
    let root = Arc::new(JournalRoot::open(&root_path).unwrap());
    let snapshot = EnableSnapshot::read(root).unwrap();
    let original = config_at(&root_path);
    std::fs::rename(&root_path, parent.path().join("original")).unwrap();
    std::fs::create_dir(&root_path).unwrap();
    assert!(snapshot.commit(&Value::Null, &|| Ok(())).is_err());
    assert!(std::fs::read_dir(&root_path).unwrap().next().is_none());
    assert_eq!(config_at(&parent.path().join("original")), original);
}

#[test]
fn stop_checkpoint_refuses_before_the_first_config_effect() {
    let (dir, deps) = fixture();
    let snapshot = EnableSnapshot::read(deps.retained_root().unwrap()).unwrap();
    let before = std::fs::read(dir.path().join("config/journal.json")).unwrap();
    deps.operations.request_stop();
    assert!(
        snapshot
            .commit(&Value::Null, &|| {
                if deps.operations.is_stopping() {
                    Err("stopped".into())
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    assert_eq!(
        std::fs::read(dir.path().join("config/journal.json")).unwrap(),
        before
    );
}

#[test]
fn queued_enable_retains_admission_after_http_future_is_dropped() {
    use std::{future::Future, sync::mpsc, task::Poll, time::Duration};
    let (dir, mut deps) = fixture();
    deps.verification_submitter = Some(Arc::new(|| Err("must not submit during restore")));
    let operations = Arc::clone(&deps.operations);
    let before = std::fs::read(dir.path().join("config/journal.json")).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (entered, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        entered.send(()).unwrap();
        released.recv().unwrap();
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut request = Box::pin(crate::offload_enable(deps));
    runtime.block_on(std::future::poll_fn(|cx| {
        assert!(request.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    }));
    let admitted_before_dispatch = operations.lock().unwrap().writes_in_flight != 0;
    whole_journal_restore::hold_target_for_restore(dir.path());
    let (drained, drainage) = mpsc::channel();
    let journal = dir.path().to_owned();
    let drainer = std::thread::spawn(move || {
        whole_journal_restore::wait_for_target_use_to_end(&journal);
        drained.send(()).unwrap();
    });
    // The queued owner, rather than the HTTP future, must retain admission.
    drop(request);
    let premature_drain = drainage.recv_timeout(Duration::from_millis(100)).is_ok();
    release.send(()).unwrap();
    runtime.block_on(blocker).unwrap();
    if !premature_drain {
        drainage.recv_timeout(Duration::from_secs(3)).unwrap();
    }
    drainer.join().unwrap();
    runtime.shutdown_timeout(Duration::from_secs(3));
    whole_journal_restore::release_target_for_restore(dir.path());
    assert_eq!(
        std::fs::read(dir.path().join("config/journal.json")).unwrap(),
        before
    );
    eprintln!(
        "admitted_before_dispatch={admitted_before_dispatch} premature_drain_after_http_loss={premature_drain}"
    );
    assert!(
        admitted_before_dispatch,
        "write admission must precede blocking queue dispatch"
    );
    assert!(
        !premature_drain,
        "restore drained while the accepted native frame was still queued after HTTP loss"
    );
}
