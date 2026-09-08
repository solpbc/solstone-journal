// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use solstone_core_backup_runtime::{
    AdmittedDestination, BackupServices, ToolOutput, ToolRequest, ToolRunner, restore_journal,
    run_restic, test_support::RestoreRecorderSpy, windows_cleanup::RetainingToolRunner,
};
use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperCleanup, BoundedHelperError, BoundedHelperRequest,
    BoundedHelperResources, HelperAdmissionStatus, HelperCleanupObservationFault,
    HelperCleanupStatus, observe_bounded_helper_admission, retry_bounded_helper_admission_until,
    run_bounded_helper, run_bounded_helper_with_observation_fault_for_test,
};
use tower::ServiceExt;

use crate::{BackupWebDeps, operation, restore_prepare};

const WAIT: Duration = Duration::from_secs(10);
const SELECTOR: &str = "cleanup::native_tests::backup_native_cleanup_ownership";
const INNER_MARKERS: &[&str] = &[
    "BACKUP_CLEANUP_CANCEL_BEFORE_PUBLICATION_PASS",
    "BACKUP_CLEANUP_RETAINED_ROOT_STAGING_PASS",
    "BACKUP_CLEANUP_MUTATION_FENCE_PASS",
    "BACKUP_CLEANUP_ORIGINAL_RESULT_RECOVERY_PASS",
    "BACKUP_CLEANUP_GLOBAL_RESULT_PRESERVED_PASS",
    "BACKUP_CLEANUP_WHOLE_RECORDING_GUARD_PASS",
    "BACKUP_CLEANUP_SELECTIVE_RECORDING_GUARD_PASS",
];

fn run_owned_fixture() {
    let temporary = Arc::new(tempfile::tempdir().unwrap());
    let executable = std::env::current_exe().unwrap();
    let package_root = executable.parent().unwrap().parent().unwrap().to_path_buf();
    let mut environment = BTreeMap::new();
    for name in [
        "SystemRoot",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Some(value) = std::env::var_os(name) {
            environment.insert(name.into(), value);
        }
    }
    for name in ["TEMP", "TMP"] {
        environment.insert(name.into(), temporary.path().as_os_str().to_owned());
    }
    environment.insert("SOLSTONE_BACKUP_CLEANUP_FIXTURE".into(), "1".into());
    let result = run_bounded_helper(BoundedHelperRequest {
        executable,
        current_directory: package_root.clone(),
        package_root,
        arguments: ["--exact", SELECTOR, "--ignored", "--show-output"]
            .map(str::to_owned)
            .to_vec(),
        environment,
        stdin: Vec::new(),
        budget: BoundedHelperBudget {
            timeout: Duration::from_secs(120),
            stdin_limit_bytes: 1,
            stdout_limit_bytes: 1024 * 1024,
            stderr_limit_bytes: 1024 * 1024,
        },
        resource_limits: None,
        resources: {
            let mut resources = BoundedHelperResources::new();
            resources.retain(temporary.clone());
            resources
        },
    });
    let output = match result {
        Ok(output) => output,
        Err(failure) => {
            let settled = retry_bounded_helper_admission_until(Instant::now() + WAIT);
            panic!("native cleanup fixture failed: {failure}; cleanup={settled:?}");
        }
    };
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.quiescent,
        "outer Job must have no remaining descendants"
    );
    assert_eq!(output.exit_code, 0, "nested cleanup fixture failed");
    let stdout = String::from_utf8(output.stdout).expect("native libtest UTF-8 output");
    let named_pass = format!("test {SELECTOR} ... ok");
    assert_eq!(
        stdout.lines().filter(|line| *line == named_pass).count(),
        1,
        "the named inner cleanup fixture must actually pass"
    );
    assert_eq!(
        stdout
            .matches("test result: ok. 1 passed; 0 failed; 0 ignored;")
            .count(),
        1
    );
    let prefix = format!("test {SELECTOR} ... ");
    for marker in INNER_MARKERS {
        assert_eq!(
            stdout
                .lines()
                .filter(|line| { line.strip_prefix(&prefix).unwrap_or(line).trim() == *marker })
                .count(),
            1,
            "missing or repeated native control: {marker}"
        );
    }
    println!("BACKUP_CLEANUP_OUTER_QUIESCENT_PASS");
}

fn child_request(resources: BoundedHelperResources) -> BoundedHelperRequest {
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("solstone-system-test-child.exe");
    assert!(
        executable.is_file(),
        "build the native system test child first"
    );
    let package_root = executable.parent().unwrap().to_path_buf();
    BoundedHelperRequest {
        executable,
        current_directory: package_root.clone(),
        package_root,
        arguments: vec!["sleep".into()],
        environment: BTreeMap::from([(
            OsString::from("SystemRoot"),
            std::env::var_os("SystemRoot").expect("native SystemRoot"),
        )]),
        stdin: Vec::new(),
        budget: BoundedHelperBudget {
            timeout: Duration::from_secs(2),
            stdin_limit_bytes: 1024,
            stdout_limit_bytes: 1024,
            stderr_limit_bytes: 1024,
        },
        resource_limits: None,
        resources,
    }
}

// Releasing the latch on every unwind lets the original registry owner clean
// its Job and streams. The enclosing native driver also bounds the test process.
struct FaultGuard(HelperCleanupObservationFault);

impl Drop for FaultGuard {
    fn drop(&mut self) {
        self.0.release();
        let state = retry_bounded_helper_admission_until(Instant::now() + WAIT);
        if !std::thread::panicking() {
            assert!(matches!(state, HelperAdmissionStatus::Ready));
        }
    }
}

struct FaultRunner {
    fault: HelperCleanupObservationFault,
    entered: mpsc::Sender<BoundedHelperCleanup>,
    publish: Mutex<mpsc::Receiver<()>>,
    calls: AtomicUsize,
    partial_output: Option<PathBuf>,
}

impl ToolRunner for FaultRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut native = child_request(request.resources.clone());
        if let Some(path) = &self.partial_output {
            native.arguments = vec![
                "ready-sleep".into(),
                path.display().to_string(),
                "30000".into(),
            ];
        }
        let failure = run_bounded_helper_with_observation_fault_for_test(native, &self.fault)
            .expect_err("the latch must retain a genuinely launched owner");
        let owner = failure.cleanup().expect("own launched helper").clone();
        assert!(
            owner.identity().is_some(),
            "normal launch identity must be real"
        );
        if let Some(path) = &self.partial_output {
            let deadline = Instant::now() + WAIT;
            while !std::fs::read(path).is_ok_and(|bytes| bytes.starts_with(b"ready:")) {
                assert!(
                    Instant::now() < deadline,
                    "native child did not publish its partial file"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.entered.send(owner).unwrap();
        self.publish.lock().unwrap().recv_timeout(WAIT).unwrap();
        Err(io::Error::other(failure))
    }
}

struct RestoreWriterFault {
    writer: FaultRunner,
    expected_resources: usize,
}

impl ToolRunner for RestoreWriterFault {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        assert_eq!(request.resources.len(), self.expected_resources);
        match request.argv.first().and_then(|arg| arg.to_str()) {
            Some("snapshots") => Ok(ToolOutput {
                returncode: 0,
                stdout: serde_json::to_vec(&serde_json::json!([{
                    "id": "a".repeat(64),
                    "time": "2026-01-01T00:00:00Z", "paths": ["/journal"]
                }]))
                .unwrap(),
                stderr: Vec::new(),
            }),
            Some("restore") => self.writer.run(request),
            other => panic!("unexpected engine helper: {other:?}"),
        }
    }
}

fn restore_engines_preserve_configuration_while_writer_cleanup_is_pending() {
    for selective in [false, true] {
        let fault = FaultGuard(HelperCleanupObservationFault::new());
        let root = if selective {
            crate::test_support::offload_inventory_root()
        } else {
            crate::test_support::root("fresh")
        };
        let partial = if selective {
            root.path()
                .join("chronicle")
                .join("20260102")
                .join("020000_001")
                .join("backup.webm")
        } else {
            root.path().join("partial-restore.bin")
        };
        assert!(!partial.exists());
        let (entered, waiting) = mpsc::channel();
        let (publish, publication) = mpsc::channel();
        publish.send(()).unwrap();
        let runner = RestoreWriterFault {
            writer: FaultRunner {
                fault: fault.0.clone(),
                entered,
                publish: Mutex::new(publication),
                calls: AtomicUsize::new(0),
                partial_output: Some(partial.clone()),
            },
            expected_resources: if selective { 2 } else { 1 },
        };
        let deps = BackupWebDeps::production(
            root.path().to_path_buf(),
            crate::measurement::new(root.path()),
        );
        let config = root.path().join("config/journal.json");
        let before = std::fs::read(&config).unwrap();
        let executable = child_request(BoundedHelperResources::new()).executable;
        let services = BackupServices {
            runner: &runner,
            http: deps.http.as_ref(),
            clock: deps.clock.as_ref(),
            restic_path: Some(&executable),
            rclone_path: None,
            version: deps.version,
            journal_maintenance: deps.journal_maintenance.as_ref(),
        };
        let recorder = RestoreRecorderSpy::new();
        if selective {
            let result =
                solstone_core_offload::restore_offload_day(root.path(), &services, "20260102");
            assert_eq!(result.status, "error");
            assert_eq!(result.reason.as_deref(), Some("failed"));
        } else {
            let destination = solstone_core_backup::Destination {
                repository: "s3:s3.example.invalid/cleanup".into(),
                backend: "s3".into(),
                credentials: serde_json::json!({
                    "access_key_id":"SYNTHETIC", "secret_access_key":"synthetic-secret"
                })
                .as_object()
                .unwrap()
                .clone(),
            };
            let result = restore_journal(
                root.path(),
                &services,
                &recorder,
                destination,
                &solstone_core_backup::generate_recovery_key().unwrap(),
            );
            assert_eq!(result.status, "error");
            assert_eq!(result.reason_code.as_deref(), Some("restore_io_failed"));
            assert!(
                recorder.calls().is_empty(),
                "no recorder call while the writer retains cleanup"
            );
        }
        let owner = waiting.recv_timeout(WAIT).unwrap();
        assert_eq!(owner.observe(), HelperCleanupStatus::Pending);
        assert_eq!(runner.writer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read(&config).unwrap(), before);
        let partial_bytes =
            std::fs::read(&partial).expect("rollback must not remove a pending writer's file");
        assert!(partial_bytes.starts_with(b"ready:"));
        fault.0.release();
        assert_eq!(
            owner.retry_until(Instant::now() + WAIT),
            HelperCleanupStatus::Quiescent
        );
        assert_eq!(
            std::fs::read(&config).unwrap(),
            before,
            "cleanup must not replay bookkeeping"
        );
        assert!(recorder.calls().is_empty());
        assert_eq!(
            std::fs::read(&partial).unwrap(),
            partial_bytes,
            "recovery must not replay rollback"
        );
        println!(
            "{}",
            if selective {
                "BACKUP_CLEANUP_SELECTIVE_RECORDING_GUARD_PASS"
            } else {
                "BACKUP_CLEANUP_WHOLE_RECORDING_GUARD_PASS"
            }
        );
    }
}

fn wait_for_phase(deps: &BackupWebDeps, phase: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        if operation::observed_current(&deps.operations)
            .unwrap()
            .is_some_and(|current| current.phase == phase)
        {
            return;
        }
        assert!(Instant::now() < deadline, "worker did not publish {phase}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn json_response(response: axum::response::Response) -> (StatusCode, Value) {
    let code = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap())
}

async fn post(router: &axum::Router, path: &str) -> (StatusCode, Value) {
    json_response(
        router
            .clone()
            .oneshot(Request::post(path).body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await
}

async fn assert_mutations_refused(router: &axum::Router, config: &Path, before: &[u8]) {
    for path in super::CONFLICTING_MUTATIONS {
        let (code, value) = post(router, path).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{path}: {value}");
        assert_eq!(value["reason_code"], "backup_busy", "{path}: {value}");
        assert_eq!(std::fs::read(config).unwrap(), before, "{path}");
    }
}

// Run alone: this fixture intentionally fences process-wide helper admission.
// The native driver must build solstone-system-test-child and impose an overall
// deadline with owned process cleanup; a filtered zero-test result is not proof.
#[tokio::test]
#[ignore = "requires the isolated native Windows cleanup driver"]
async fn backup_native_cleanup_ownership() {
    if std::env::var_os("SOLSTONE_BACKUP_CLEANUP_FIXTURE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        run_owned_fixture();
        return;
    }
    assert!(matches!(
        observe_bounded_helper_admission(),
        HelperAdmissionStatus::Ready
    ));
    let root = crate::test_support::root("fresh");
    let mut deps = BackupWebDeps::production(
        root.path().to_path_buf(),
        crate::measurement::new(root.path()),
    );
    let config = root.path().join("config/journal.json");
    let before = std::fs::read(&config).unwrap();
    let fault = FaultGuard(HelperCleanupObservationFault::new());
    let (entered, waiting) = mpsc::channel();
    let (publish, publication) = mpsc::channel();
    let runner = Arc::new(FaultRunner {
        fault: fault.0.clone(),
        entered,
        publish: Mutex::new(publication),
        calls: AtomicUsize::new(0),
        partial_output: None,
    });
    deps.runner = runner.clone();
    let prepared = restore_prepare::prepare(&deps.restore_prepare, &deps.operations).unwrap();
    restore_prepare::key(
        &deps.restore_prepare,
        &deps.operations,
        &prepared.capability,
        "synthetic-cleanup-recovery-key".into(),
        || {
            Ok((
                "synthetic-cleanup-nonce".into(),
                "https://example.invalid/consent".into(),
            ))
        },
    )
    .unwrap();
    restore_prepare::arm(
        &deps.restore_prepare,
        &deps.operations,
        &prepared.capability,
    )
    .unwrap();
    let restore_prepare::Activation::Spawn { generation, .. } = restore_prepare::activate(
        &deps.restore_prepare,
        &deps.operations,
        &prepared.capability,
    )
    .unwrap() else {
        panic!("fresh activation must own its worker");
    };

    let staging = Arc::new(tempfile::tempdir().unwrap());
    let staging_path = staging.path().to_path_buf();
    let admitted = Arc::new(AdmittedDestination::admit(staging.path()).unwrap());
    let weak_staging = Arc::downgrade(&staging);
    let weak_admitted = Arc::downgrade(&admitted);
    let mut resources = BoundedHelperResources::new();
    resources.retain(admitted);
    resources.retain(staging);
    super::spawn_worker(deps.clone(), generation, move |worker| {
        let retaining = RetainingToolRunner::new(worker.runner.as_ref(), resources);
        let path = child_request(BoundedHelperResources::new()).executable;
        let result = run_restic(
            &retaining,
            &["snapshots".into()],
            "synthetic-repository",
            "synthetic-password",
            &path,
            None,
            false,
            None,
            Some(Duration::from_secs(2)),
            &[],
        );
        assert!(result.is_err());
        operation::Terminal::error("snapshot_list_io_failed")
    });
    let owner = waiting.recv_timeout(WAIT).unwrap();
    assert_eq!(owner.observe(), HelperCleanupStatus::Pending);
    assert_eq!(
        restore_prepare::cancel(
            &deps.restore_prepare,
            &deps.operations,
            &prepared.capability
        )
        .unwrap_err()
        .status(),
        StatusCode::BAD_REQUEST
    );
    operation::mark_cancelled(&deps.operations, generation);
    operation::mark_expired(&deps.operations, generation);
    operation::backdate_started(
        &deps.operations,
        operation::HANDOFF_TTL + Duration::from_secs(1),
    );
    assert!(operation::worker_or_cleanup_active(
        &deps.operations,
        generation
    ));
    assert!(!operation::is_terminal(
        &operation::current(&deps.operations).unwrap().phase
    ));
    publish.send(()).unwrap();
    wait_for_phase(&deps, "cleanup_pending");
    assert_eq!(
        weak_admitted.strong_count(),
        1,
        "only the helper resource bag owns the root"
    );
    assert_eq!(
        weak_staging.strong_count(),
        1,
        "only the helper resource bag owns staging"
    );
    assert!(staging_path.is_dir());
    {
        let slot = deps.operations.lock().unwrap();
        let slot = slot.as_ref().unwrap();
        assert!(slot.nonce.is_none() && slot.restore_key.is_none());
        assert!(slot.view.portal_url.is_none());
    }
    println!("BACKUP_CLEANUP_CANCEL_BEFORE_PUBLICATION_PASS");
    println!("BACKUP_CLEANUP_RETAINED_ROOT_STAGING_PASS");

    let blocked = run_bounded_helper(child_request(BoundedHelperResources::new())).unwrap_err();
    assert_eq!(blocked.cause(), &BoundedHelperError::CleanupPending);
    assert!(blocked.cleanup().is_none());
    assert!(!blocked.blockers().is_empty());
    let router = crate::routes_with_deps(deps.clone());
    assert_mutations_refused(&router, &config, &before).await;
    let current = crate::status::status(root.path(), &deps.operations).unwrap();
    assert_eq!(current["operation"]["phase"], "cleanup_pending");
    assert_eq!(current["cleanup_admission"], "pending");
    assert!(!current.to_string().contains("synthetic-cleanup"));
    let started = Instant::now();
    let (code, value) = post(&router, "/app/backup/api/cleanup/retry").await;
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        matches!(code, StatusCode::ACCEPTED | StatusCode::BAD_REQUEST),
        "{value}"
    );
    assert_eq!(owner.observe(), HelperCleanupStatus::Pending);
    assert_eq!(
        operation::current(&deps.operations).unwrap().phase,
        "cleanup_pending"
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    println!("BACKUP_CLEANUP_MUTATION_FENCE_PASS");

    fault.0.release();
    let (code, value) = post(&router, "/app/backup/api/cleanup/retry").await;
    assert_eq!(code, StatusCode::OK, "{value}");
    assert_eq!(value["cleanup_admission"], "ready");
    assert_eq!(value["operation"]["phase"], "error");
    assert_eq!(value["operation"]["reason_code"], "snapshot_list_io_failed");
    assert_eq!(owner.observe(), HelperCleanupStatus::Quiescent);
    assert!(weak_admitted.upgrade().is_none());
    assert!(weak_staging.upgrade().is_none());
    assert!(!staging_path.exists());
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert!(operation::begin(&deps.operations, "restore", None, None, None).is_ok());
    println!("BACKUP_CLEANUP_ORIGINAL_RESULT_RECOVERY_PASS");
    drop(fault);

    // A different helper's pending cleanup must not replace an earlier success.
    let generation = operation::generation_of(&deps.operations).unwrap();
    operation::finish(&deps.operations, generation, "done", None, None);
    let fault = FaultGuard(HelperCleanupObservationFault::new());
    let failure = run_bounded_helper_with_observation_fault_for_test(
        child_request(BoundedHelperResources::new()),
        &fault.0,
    )
    .unwrap_err();
    assert!(failure.cleanup().is_some());
    let current = crate::status::status(root.path(), &deps.operations).unwrap();
    assert_eq!(current["operation"]["phase"], "done");
    assert_eq!(current["cleanup_admission"], "pending");
    assert_mutations_refused(&router, &config, &before).await;
    let _ = post(&router, "/app/backup/api/cleanup/retry").await;
    assert_eq!(operation::current(&deps.operations).unwrap().phase, "done");
    fault.0.release();
    let (code, value) = post(&router, "/app/backup/api/cleanup/retry").await;
    assert_eq!(code, StatusCode::OK, "{value}");
    assert_eq!(value["operation"]["phase"], "done");
    assert_eq!(value["cleanup_admission"], "ready");
    assert_eq!(std::fs::read(&config).unwrap(), before);
    println!("BACKUP_CLEANUP_GLOBAL_RESULT_PRESERVED_PASS");
    drop(fault);
    restore_engines_preserve_configuration_while_writer_cleanup_is_pending();
}
