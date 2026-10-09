// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_system::process::{LaunchError, ProcessBirth};
use std::sync::atomic::AtomicUsize;

fn scripted_owner(fail: bool) -> (Arc<Owner>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let invoked = Arc::clone(&calls);
    let child = LaunchAuthority::scripted(
        123,
        || Ok(Some(0)),
        move |timeout| {
            assert!(timeout <= Duration::from_secs(2));
            invoked.fetch_add(1, Ordering::SeqCst);
            if fail {
                Err(LaunchError::Admission("fixture cleanup failure".into()))
            } else {
                Ok(())
            }
        },
    );
    (
        Arc::new(Owner {
            instance: ProcessInstance {
                pid: 123,
                birth: ProcessBirth::windows(1234),
            },
            journal: PathBuf::from("fixture-journal"),
            child: Mutex::new(Some(child)),
            completed: AtomicBool::new(false),
            cleanup_pending: AtomicBool::new(false),
        }),
        calls,
    )
}

#[test]
fn failed_retirement_retains_authority_and_never_becomes_absent_root_success() {
    let (owner, calls) = scripted_owner(true);
    assert!(
        retire(&owner)
            .unwrap_err()
            .contains("fixture cleanup failure")
    );
    assert!(owner.child.lock().unwrap().is_some());
    assert!(owner.cleanup_pending.load(Ordering::Acquire));
    assert!(!owner.completed.load(Ordering::Acquire));
    assert!(retire(&owner).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn successful_retirement_is_idempotent_and_contended_lock_is_bounded() {
    let (owner, calls) = scripted_owner(false);
    let guard = owner.child.lock().unwrap();
    assert!(lock_child(&owner, Instant::now()).is_err());
    drop(guard);
    retire(&owner).unwrap();
    retire(&owner).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(owner.completed.load(Ordering::Acquire));
}

#[test]
fn completion_cache_never_evicts_failed_or_active_authority() {
    let (failed, _) = scripted_owner(true);
    let (active, _) = scripted_owner(false);
    let mut owners = vec![Arc::clone(&failed), Arc::clone(&active)];
    for _ in 0..COMPLETED_LIMIT + 5 {
        let (completed, _) = scripted_owner(false);
        retire(&completed).unwrap();
        owners.push(completed);
    }
    prune_completed(&mut owners);
    assert_eq!(owners.len(), COMPLETED_LIMIT + 2);
    assert!(owners.iter().any(|owner| Arc::ptr_eq(owner, &failed)));
    assert!(owners.iter().any(|owner| Arc::ptr_eq(owner, &active)));
}

#[cfg(feature = "full-tests")]
mod native {
    use super::*;
    use solstone_core_system::process::{
        InstanceVerdict, ManagedLaunchRequest, ProcessInstanceSource, SpawnOptions,
        SystemProcessInstanceSource, current_process_identity,
    };

    const ROOT: &str = "JOURNAL_TEST_INSTALLER_ROOT";
    const MODE: &str = "JOURNAL_TEST_INSTALLER_MODE";
    const FIXTURE: &str = "thinking_install::windows::tests::native::installer_fixture";
    const DESCENDANT: &str =
        "thinking_install::windows::tests::native::installer_descendant_fixture";

    #[test]
    #[ignore = "child fixture selected only by the native installer receipt"]
    fn installer_descendant_fixture() {
        let root = PathBuf::from(std::env::var_os(ROOT).expect("fixture root"));
        std::fs::write(
            root.join("descendant.json"),
            serde_json::to_vec(&current_process_identity().unwrap()).unwrap(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(120));
    }

    #[test]
    #[ignore = "child fixture selected only by the native installer receipt"]
    #[allow(clippy::zombie_processes)] // Root-exit fixture deliberately leaves its Job descendant alive.
    #[cfg(all(test, feature = "full-tests"))]
    fn installer_fixture() {
        let root = PathBuf::from(std::env::var_os(ROOT).expect("fixture root"));
        if std::env::var(MODE).unwrap() == "slow-preflight" {
            // Installed signature/readiness checks precede the lease/status.
            std::thread::sleep(Duration::from_secs(6));
        }
        if std::env::var(MODE).unwrap() == "no-status" {
            std::fs::write(
                root.join("root-instance.json"),
                serde_json::to_vec(&current_process_identity().unwrap()).unwrap(),
            )
            .unwrap();
        }
        let Some(_held) = lease::acquire(&root, "local").unwrap() else {
            return;
        };
        let _descendant = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", DESCENDANT, "--ignored", "--nocapture"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("descendant.json").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        if std::env::var(MODE).unwrap() != "no-status" {
            status::begin(
                &root,
                "{}".into(),
                "target".into(),
                Some(serde_json::json!(current_process_identity().unwrap())),
                "downloading",
            )
            .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while !root.join("root-exit").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn launch_fixture(root: &Path, mode: &str, timeout: Duration) -> Result<Value, String> {
        crate::thinking_install::launch_installer(
            root,
            "local/qwen3.5-4b",
            ManagedLaunchRequest {
                read_file_grants: Vec::new(),
                command: vec![
                    std::env::current_exe()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    "--exact".into(),
                    FIXTURE.into(),
                    "--ignored".into(),
                    "--nocapture".into(),
                ],
                options: SpawnOptions {
                    journal_root: root.to_owned(),
                    reference: "installer-fixture".into(),
                    day: None,
                    sink: None,
                    environment: [
                        (ROOT.into(), root.as_os_str().to_owned()),
                        (MODE.into(), mode.into()),
                    ]
                    .into(),
                },
            },
            timeout,
        )
    }

    fn wait_retired(instance: ProcessInstance) {
        let owner = find(instance).unwrap().expect("registered original Job");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !owner.completed.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "original Job cleanup did not complete"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn descendant(root: &Path) -> ProcessInstance {
        serde_json::from_slice(&std::fs::read(root.join("descendant.json")).unwrap()).unwrap()
    }

    fn gone(instance: ProcessInstance) {
        assert_eq!(
            SystemProcessInstanceSource.observe(&instance),
            InstanceVerdict::NotSameOrExited
        );
    }

    #[test]
    #[ignore = "native Windows installer admission and Job-tree cancellation receipt"]
    fn windows_installer_job_receipt() {
        let root = tempfile::tempdir().unwrap();
        let admitted = launch_fixture(root.path(), "hold", ADMISSION_TIMEOUT);
        let admitted = admitted.unwrap();
        let current = status::read_status(root.path(), "local").unwrap();
        let instance: ProcessInstance = serde_json::from_value(current.owner.unwrap()).unwrap();
        let descendant_identity = descendant(root.path());
        let duplicate = crate::thinking_install::start(root.path(), "local/qwen3.5-4b").unwrap();
        assert_eq!(duplicate["attempt_id"], admitted["attempt_id"]);
        let loser = launch_fixture(root.path(), "hold", ADMISSION_TIMEOUT);
        assert_eq!(loser.unwrap()["attempt_id"], admitted["attempt_id"]);
        assert!(matches!(
            SystemProcessInstanceSource.observe(&instance),
            InstanceVerdict::SameLive { .. }
        ));
        // Managed disposition timeout is a stop budget, not a download deadline.
        std::thread::sleep(Duration::from_millis(2200));
        assert!(lease::is_held(root.path(), "local").unwrap());
        let stale = ProcessInstance {
            pid: instance.pid,
            birth: ProcessBirth::windows(instance.birth.windows_filetime().unwrap() + 1),
        };
        assert!(stop(stale).is_err());
        assert!(matches!(
            SystemProcessInstanceSource.observe(&instance),
            InstanceVerdict::SameLive { .. }
        ));
        let attempt = admitted["attempt_id"].as_str().unwrap().to_owned();
        let second_root = root.path().to_owned();
        let second_attempt = attempt.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let second_barrier = Arc::clone(&barrier);
        let other = std::thread::spawn(move || {
            second_barrier.wait();
            crate::thinking_install::cancel(&second_root, &second_attempt)
        });
        barrier.wait();
        crate::thinking_install::cancel(root.path(), &attempt).unwrap();
        other.join().unwrap().unwrap();
        wait_retired(instance);
        gone(instance);
        gone(descendant_identity);
        assert!(!lease::is_held(root.path(), "local").unwrap());

        std::fs::remove_file(root.path().join("descendant.json")).unwrap();
        let next = launch_fixture(root.path(), "hold", ADMISSION_TIMEOUT);
        assert_ne!(next.unwrap()["attempt_id"], admitted["attempt_id"]);
        let descendant_identity = descendant(root.path());
        std::fs::write(root.path().join("root-exit"), b"exit").unwrap();
        let next_owner = serde_json::from_value(
            status::read_status(root.path(), "local")
                .unwrap()
                .owner
                .unwrap(),
        )
        .unwrap();
        wait_retired(next_owner);
        gone(descendant_identity);
        // The root left its attempt in flight; the monitor records it as
        // interrupted instead of leaving `downloading` with nothing running.
        let ended = status::read_status(root.path(), "local").unwrap();
        assert_eq!(ended.install_state, "failed");
        assert_eq!(ended.error_code.as_deref(), Some("install_interrupted"));
        reconcile(root.path()).unwrap();

        let timeout_root = tempfile::tempdir().unwrap();
        let timed_out = launch_fixture(timeout_root.path(), "no-status", Duration::from_secs(4));
        assert!(timed_out.unwrap_err().contains("admission timed out"));
        gone(
            serde_json::from_slice(
                &std::fs::read(timeout_root.path().join("root-instance.json")).unwrap(),
            )
            .unwrap(),
        );
        gone(descendant(timeout_root.path()));
        assert!(!lease::is_held(timeout_root.path(), "local").unwrap());

        let slow_root = tempfile::tempdir().unwrap();
        let slow = launch_fixture(
            slow_root.path(),
            "slow-preflight",
            crate::thinking_install::INSTALLER_STARTUP_TIMEOUT,
        )
        .unwrap();
        let slow_instance = serde_json::from_value(
            status::read_status(slow_root.path(), "local")
                .unwrap()
                .owner
                .unwrap(),
        )
        .unwrap();
        let slow_descendant = descendant(slow_root.path());
        crate::thinking_install::cancel(slow_root.path(), slow["attempt_id"].as_str().unwrap())
            .unwrap();
        wait_retired(slow_instance);
        gone(slow_instance);
        gone(slow_descendant);
        assert!(!lease::is_held(slow_root.path(), "local").unwrap());
        println!("JOURNAL_WIN_CI_INSTALLER_JOB=executed/pass");
    }

    #[tokio::test]
    async fn cleanup_failure_after_lease_release_blocks_cancel_and_replacement() {
        use axum::body::{Body, to_bytes};
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        let root = tempfile::tempdir().unwrap();
        let (mut owner, calls) = scripted_owner(true);
        Arc::get_mut(&mut owner).unwrap().journal = root.path().to_owned();
        let instance = owner.instance;
        let current = status::begin(
            root.path(),
            "{}".into(),
            "target".into(),
            Some(serde_json::json!(instance)),
            "downloading",
        )
        .unwrap();
        OWNERS.lock().unwrap().push(Arc::clone(&owner));
        assert!(!lease::is_held(root.path(), "local").unwrap());
        assert!(
            crate::thinking_install::cancel(root.path(), current.attempt_id.as_deref().unwrap())
                .unwrap_err()
                .contains("fixture cleanup failure")
        );
        assert!(
            crate::thinking_install::start(root.path(), "local/qwen3.5-4b")
                .unwrap_err()
                .contains("fixture cleanup failure")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            status::read_status(root.path(), "local")
                .unwrap()
                .install_state,
            "downloading"
        );
        for state in ["downloading", "installed"] {
            let mut terminal = status::read_status(root.path(), "local").unwrap();
            terminal.install_state = state.into();
            status::write_status(root.path(), terminal).unwrap();
            let app = crate::thinking::router(Arc::new(crate::JournalRoot(root.path().to_owned())));
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/app/thinking/api/local/bootstrap")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = to_bytes(response.into_body(), 65536).await.unwrap();
            assert!(String::from_utf8_lossy(&body).contains("fixture cleanup failure"));
            assert!(
                crate::thinking_install::cancel(
                    root.path(),
                    current.attempt_id.as_deref().unwrap()
                )
                .is_err()
            );
        }
        OWNERS
            .lock()
            .unwrap()
            .retain(|entry| !Arc::ptr_eq(entry, &owner));
    }

    #[test]
    fn monitor_and_canceller_share_one_retirement() {
        let (owner, calls) = scripted_owner(false);
        let second = Arc::clone(&owner);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let other_barrier = Arc::clone(&barrier);
        let other = std::thread::spawn(move || {
            other_barrier.wait();
            retire(&second)
        });
        barrier.wait();
        retire(&owner).unwrap();
        other.join().unwrap().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
