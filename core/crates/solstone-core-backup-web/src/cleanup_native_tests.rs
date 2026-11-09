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
use solstone_core_backup::archive_admission::ArchiveCancellation;
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
    "BACKUP_CLEANUP_SELECTIVE_RESTORE_HELD_PASS",
    "BACKUP_CLEANUP_ARCHIVE_QUERY_RELEASE_PASS",
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
        stdout_sink: None,
        cancellation: None,
    });
    let output = match result {
        Ok(output) => output,
        Err(failure) => {
            let settled = retry_bounded_helper_admission_until(Instant::now() + WAIT);
            panic!("native cleanup fixture failed: {failure}; cleanup={settled:?}");
        }
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        println!("native cleanup: {line}");
    }
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
        stdout_sink: None,
        cancellation: None,
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

struct ArchiveQueryFault(HelperCleanupObservationFault);

struct ArchiveQueryCleanup(BoundedHelperCleanup);

impl solstone_core_backup::archive_readback::WindowsArchiveCleanup for ArchiveQueryCleanup {
    fn quiescent(&self) -> bool {
        matches!(self.0.observe(), HelperCleanupStatus::Quiescent)
    }
}

impl solstone_core_backup::archive_readback::WindowsArchiveExecutor for ArchiveQueryFault {
    fn run_cancellable(
        &self,
        request: solstone_core_backup::archive_readback::WindowsArchiveRequest<'_>,
        _capture_limit: usize,
    ) -> solstone_core_backup::archive_readback::WindowsExecutionOutput {
        assert_eq!(request.argv, [OsString::from("version")]);
        let mut resources = BoundedHelperResources::new();
        resources.retain(Arc::new(request.retained));
        let failure =
            run_bounded_helper_with_observation_fault_for_test(child_request(resources), &self.0)
                .expect_err("query helper must initially retain its actual cleanup owner");
        let cleanup = failure
            .cleanup()
            .expect("launched native query owner")
            .clone();
        assert!(cleanup.identity().is_some());
        request
            .cancellation
            .expect("original page cancellation")
            .cancel();
        solstone_core_backup::archive_readback::WindowsExecutionOutput {
            returncode: 1,
            stdout: Vec::new(),
            stderr: Vec::new(),
            cleanup: Some(Arc::new(ArchiveQueryCleanup(cleanup))),
        }
    }

    fn run_dump(
        &self,
        _request: solstone_core_backup::archive_readback::WindowsArchiveRequest<'_>,
        _stage: std::fs::File,
        _byte_bound: u64,
    ) -> solstone_core_backup::archive_readback::WindowsExecutionOutput {
        panic!("refused admission must never submit a dump writer")
    }
}

fn archive_query_cleanup_releases_original_page_owner() {
    use crate::archive_worker::{self, ArchivePayload};
    use solstone_core_backup::archive_activity::OwnedArchivePreparationInput;
    use solstone_core_backup_runtime::OwnedArchiveAdmissionFailure;
    use solstone_core_journal_io::JournalRoot;

    let directory = crate::test_support::root("healthy");
    let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
    let deps = BackupWebDeps::production(
        directory.path().to_path_buf(),
        crate::measurement::new(directory.path()),
    );
    let begin = archive_worker::begin_owned(
        &deps.operations,
        "offload_restore",
        root.clone(),
        deps.clock.clone(),
    )
    .unwrap();
    let (ticket, root, budget, clock) = begin.into_parts();
    assert!(archive_worker::start_owned_worker(
        &deps.operations,
        &ticket
    ));
    let (input, custody) = OwnedArchivePreparationInput::capture(root, budget, clock)
        .unwrap_or_else(|_| panic!("original query preparation inputs"));
    let prepared = input
        .prepare()
        .unwrap_or_else(|_| panic!("original query preparation"));
    let carrier = Arc::downgrade(prepared.carrier());
    let destination = prepared.configuration().destination().unwrap();
    let password = prepared.configuration().keys().unwrap().unwrap().daily_key;
    let fault = FaultGuard(HelperCleanupObservationFault::new());
    let refusal = match prepared
        .capture_current_read(destination, password, PathBuf::from("unused-restic.exe"))
        .admit(Some(Arc::new(ArchiveQueryFault(fault.0.clone()))))
    {
        Ok(_) => panic!("pending query must refuse admission"),
        Err(refusal) => refusal,
    };
    assert_eq!(
        refusal.reason(),
        solstone_core_backup::archive_readback::ReadbackError::Admission(
            solstone_core_backup::archive_admission::AdmissionError::Cancelled,
        ),
    );
    archive_worker::return_owned(
        &deps.operations,
        ticket,
        Some(custody),
        ArchivePayload::AdmissionFailure(
            OwnedArchiveAdmissionFailure::from_readback_refusal_for_test(refusal),
        ),
        operation::Terminal::error("cancelled"),
    );
    assert!(operation::is_busy(&deps.operations));
    assert_eq!(deps.operations.lock().unwrap().archive_returns().count(), 1);
    assert!(
        archive_worker::begin_owned(
            &deps.operations,
            "offload_restore",
            Arc::new(JournalRoot::open(directory.path()).unwrap()),
            deps.clock.clone(),
        )
        .is_err()
    );
    #[cfg(feature = "full-tests")]
    let lifetime = deps.lifetime();
    #[cfg(feature = "full-tests")]
    {
        lifetime.request_stop();
        assert_eq!(
            lifetime.drain(crate::test_support::join_executor(), Instant::now()),
            crate::route_lifetime::BackupRouteDrain::Pending
        );
        assert_eq!(
            deps.lifetime()
                .drain(crate::test_support::join_executor(), Instant::now()),
            crate::route_lifetime::BackupRouteDrain::Pending
        );
    }

    fault.0.release();
    assert!(matches!(
        retry_bounded_helper_admission_until(Instant::now() + WAIT),
        HelperAdmissionStatus::Ready
    ));
    // Helper admission turns Ready before the retained return is applied, so
    // wait (bounded) for the terminal phase instead of reading it at once.
    wait_for_phase(&deps, "error");
    let current = operation::observed_current(&deps.operations)
        .unwrap()
        .unwrap();
    assert_eq!(current.phase, "error");
    assert_eq!(current.reason_code.as_deref(), Some("cancelled"));
    assert_eq!(deps.operations.lock().unwrap().archive_returns().count(), 0);
    assert!(
        carrier.upgrade().is_none(),
        "the original carrier must be released"
    );
    #[cfg(feature = "full-tests")]
    assert_eq!(
        lifetime.drain(crate::test_support::join_executor(), Instant::now() + WAIT),
        crate::route_lifetime::BackupRouteDrain::Drained
    );
    assert!(
        archive_worker::begin_owned(
            &deps.operations,
            "offload_restore",
            Arc::new(JournalRoot::open(directory.path()).unwrap()),
            deps.clock.clone(),
        )
        .is_err()
    );
    println!("BACKUP_CLEANUP_ARCHIVE_QUERY_RELEASE_PASS");
}

impl ToolRunner for FaultRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        if let Some(report) = solstone_core_backup_runtime::test_support::dry_run_report(request) {
            return Ok(report);
        }
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

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _cancellation: &ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        self.run(request)
    }
}

struct RestoreWriterFault {
    writer: FaultRunner,
    expected_resources: usize,
}

impl ToolRunner for RestoreWriterFault {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        if let Some(report) = solstone_core_backup_runtime::test_support::dry_run_report(request) {
            return Ok(report);
        }
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

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _cancellation: &ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        self.run(request)
    }
}

fn restore_engines_preserve_configuration_while_writer_cleanup_is_pending() {
    let fault = FaultGuard(HelperCleanupObservationFault::new());
    let root = crate::test_support::root("fresh");
    let partial = root.path().join("partial-restore.bin");
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
        expected_resources: 1,
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
        restic: Some((&executable).into()),
        rclone_path: None,
        version: deps.version,
        journal_maintenance: deps.journal_maintenance.as_ref(),
    };
    let recorder = RestoreRecorderSpy::new();
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
    println!("BACKUP_CLEANUP_WHOLE_RECORDING_GUARD_PASS");
    drop(fault);
    held_selective_restore_keeps_owner_state_without_a_writer();
}

#[derive(Default)]
struct HeldSelectiveRunner {
    calls: AtomicUsize,
}

impl ToolRunner for HeldSelectiveRunner {
    fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::other(
            "held selective restore must not issue a tool",
        ))
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _cancellation: &ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        self.run(request)
    }
}

fn selective_fixture_snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if kind.is_dir() {
                snapshot.insert(relative, None);
                visit(root, &path, snapshot);
            } else {
                assert!(kind.is_file(), "fixture entry must be a plain file");
                snapshot.insert(relative, Some(std::fs::read(path).unwrap()));
            }
        }
    }
    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn held_selective_restore_keeps_owner_state_without_a_writer() {
    let root = crate::test_support::offload_inventory_root();
    let expected_absent = root
        .path()
        .join("chronicle/20260102/020000_001/backup.webm");
    assert!(!expected_absent.exists());
    let before = selective_fixture_snapshot(root.path());
    let runner = HeldSelectiveRunner::default();
    let deps = BackupWebDeps::production(
        root.path().to_path_buf(),
        crate::measurement::new(root.path()),
    );
    let services = BackupServices {
        runner: &runner,
        http: deps.http.as_ref(),
        clock: deps.clock.as_ref(),
        restic: None,
        rclone_path: None,
        version: deps.version,
        journal_maintenance: deps.journal_maintenance.as_ref(),
    };
    let result = solstone_core_offload::restore_offload_day(root.path(), &services, "20260102");
    assert_eq!(result.status, "refused");
    assert_eq!(result.reason.as_deref(), Some("archive_unbound"));
    assert_eq!(
        (
            result.segments_selected,
            result.files_expected,
            result.bytes_expected
        ),
        (1, 1, 7)
    );
    assert_eq!(
        (
            result.segments_restored,
            result.files_restored,
            result.bytes_restored
        ),
        (0, 0, 0)
    );
    assert_eq!(result.details.len(), 1);
    let detail = &result.details[0];
    assert_eq!(detail.status, "refused");
    assert_eq!(detail.reason.as_deref(), Some("archive_unbound"));
    assert_eq!(detail.day, "20260102");
    assert_eq!(detail.stream, "_default");
    assert_eq!(detail.segment, "020000_001");
    assert_eq!(detail.snapshot_id.as_deref(), Some("snapshot-b"));
    assert_eq!((detail.files_expected, detail.bytes_expected), (1, 7));
    assert_eq!((detail.files_restored, detail.bytes_restored), (0, 0));
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    assert!(!expected_absent.exists());
    assert_eq!(selective_fixture_snapshot(root.path()), before);
    println!("BACKUP_CLEANUP_SELECTIVE_RESTORE_HELD_PASS");
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
        let slot = slot.current.as_ref().unwrap();
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
    archive_query_cleanup_releases_original_page_owner();
}

#[cfg(feature = "full-tests")]
#[test]
fn windows_pending_cleanup_holds_drain_until_quiescent() {
    let root = tempfile::tempdir().unwrap();
    let deps = BackupWebDeps::new(root.path().to_path_buf());
    let lifetime = deps.lifetime();
    let began = operation::begin(&deps.operations, "restore", None, None, None).unwrap();
    assert!(operation::start_worker(&deps.operations, began.generation));

    let fault = FaultGuard(HelperCleanupObservationFault::new());
    let failure = run_bounded_helper_with_observation_fault_for_test(
        child_request(BoundedHelperResources::new()),
        &fault.0,
    )
    .unwrap_err();
    let owner = failure.cleanup().expect("cleanup owner").clone();

    operation::finish_worker(
        &deps.operations,
        began.generation,
        operation::Terminal::done(),
        vec![owner.clone()],
    );

    lifetime.request_stop();
    assert_eq!(
        lifetime.drain(crate::test_support::join_executor(), Instant::now()),
        crate::route_lifetime::BackupRouteDrain::Pending
    );

    fault.0.release();
    assert_eq!(
        owner.retry_until(Instant::now() + WAIT),
        HelperCleanupStatus::Quiescent
    );

    assert_eq!(
        lifetime.drain(
            crate::test_support::join_executor(),
            Instant::now() + Duration::from_secs(5)
        ),
        crate::route_lifetime::BackupRouteDrain::Drained
    );
}
