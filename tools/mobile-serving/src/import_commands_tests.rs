// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::super::{ForegroundProcessingHost, TEST_LOCK};
use super::*;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use solstone_core_backup_web::BackupWebDeps;
use solstone_core_import_sources::document::{PdfPayload, PdfWorkerRequest, WorkerFailure};
use solstone_core_system::request::{BusTaskRequest, ExecutionRequest, WireTaskRequest};
use solstone_core_system::schedule::ScheduleNow;

fn fixture_directory() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let payload = root.path().join("payload");
    for relative in [
        "solstone/talent",
        "solstone/apps",
        "solstone/think/templates",
    ] {
        fs::create_dir_all(payload.join(relative)).unwrap();
    }
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/journal.json"),
        r#"{"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true}}"#,
    )
    .unwrap();
    root
}

fn fixture() -> (tempfile::TempDir, ForegroundProcessingHost) {
    let root = fixture_directory();
    let payload = root.path().join("payload");
    let host = ForegroundProcessingHost::open_at(
        root.path(),
        &payload,
        ScheduleNow::in_zone(chrono::Utc::now(), chrono_tz::UTC),
        BackupWebDeps::new(root.path().to_path_buf()),
    )
    .unwrap();
    host.stop_discovery_for_test();
    (root, host)
}

fn open_import_fixture(
    root: &Path,
    imports: Arc<ImportSeamFactory>,
) -> Result<ForegroundProcessingHost, String> {
    open_import_fixture_with_rebuild(root, imports, false)
}

fn open_import_fixture_with_rebuild(
    root: &Path,
    imports: Arc<ImportSeamFactory>,
    rebuild_recovery: bool,
) -> Result<ForegroundProcessingHost, String> {
    let bound = Arc::new(JournalRoot::open(root).map_err(|error| error.to_string())?);
    let host = ForegroundProcessingHost::open_with_imports(
        bound,
        &root.join("payload"),
        Arc::new(|| false),
        Arc::new(AtomicBool::new(false)),
        BackupWebDeps::new(root.to_path_buf()),
        imports,
        rebuild_recovery,
        false,
    )?;
    host.stop_discovery_for_test();
    Ok(host)
}

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn submit(host: &ForegroundProcessingHost, reference: &str, args: Vec<String>) {
    let request = BusTaskRequest::decode(
        WireTaskRequest {
            cmd: Some(args),
            reference: Some(reference.to_owned()),
            ..Default::default()
        },
        reference.to_owned(),
    )
    .unwrap();
    assert!(!matches!(
        host.queue().submit(ExecutionRequest::Bus(request)),
        solstone_core_system::queue::SubmitOutcome::Rejected
    ));
}

fn finished(host: &ForegroundProcessingHost, reference: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(record) = host
            .queue()
            .history()
            .iter()
            .find(|row| row.reference == reference)
        {
            return record.exit_status.clone();
        }
        assert!(Instant::now() < deadline, "queued import did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn contains(root: &Path, needle: &str) -> bool {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if fs::read_to_string(path).is_ok_and(|text| text.contains(needle)) {
                return true;
            }
        }
    }
    false
}

fn due_import_fixture() -> (tempfile::TempDir, tempfile::TempDir) {
    let root = fixture_directory();
    let source = tempfile::tempdir().unwrap();
    fs::write(
        source.path().join("startup.md"),
        "fc3 constructor catch-up native import sentinel\n",
    )
    .unwrap();
    fs::write(
        root.path().join("config/schedules.json"),
        serde_json::to_vec(&serde_json::json!({
            "owner:startup-import": {
                "cmd": ["solstone", "journal", "importer", "--sync", "obsidian", "--save",
                    "--path", source.path().to_str().unwrap()],
                "every": "1m",
                "enabled": true
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(root.path().join("health")).unwrap();
    fs::write(
        root.path().join("health/scheduler.json"),
        br#"{"owner:startup-import":{"last_run":1.0}}"#,
    )
    .unwrap();
    (root, source)
}

fn startup_import_finished(host: &ForegroundProcessingHost) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(record) = host
            .queue()
            .history()
            .iter()
            .find(|row| row.scheduler_name.as_deref() == Some("owner:startup-import"))
        {
            return record.exit_status.clone();
        }
        assert!(
            Instant::now() < deadline,
            "startup scheduled import did not settle"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn due_import_at_constructor_start_writes_real_notes_without_start_workers() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for rebuild_recovery in [false, true] {
        let (root, source) = due_import_fixture();
        let host = open_import_fixture_with_rebuild(
            root.path(),
            Arc::new(|request| {
                let seams =
                    request.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
                Ok(PreparedImport::new(request.arguments().to_vec(), seams))
            }),
            rebuild_recovery,
        )
        .unwrap();
        let terminal = startup_import_finished(&host);
        host.shutdown().unwrap();
        let scheduler = fs::read_to_string(root.path().join("health/scheduler.json")).unwrap();
        assert_eq!(
            terminal, "ok",
            "actual startup scheduler state: {scheduler}"
        );
        assert!(contains(
            &root.path().join("chronicle"),
            "fc3 constructor catch-up native import sentinel"
        ));
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("imports/obsidian.json")).unwrap())
                .unwrap();
        assert_eq!(state["files"]["startup.md"]["status"], "imported");
        assert_eq!(
            fs::read_to_string(source.path().join("startup.md")).unwrap(),
            "fc3 constructor catch-up native import sentinel\n"
        );
    }
}

#[test]
fn constructor_error_and_stop_join_due_import_then_reopen_for_real_work() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    struct ResetStartupHook;
    impl Drop for ResetStartupHook {
        fn drop(&mut self) {
            super::super::STARTUP_AFTER_CATCH_UP
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
        }
    }
    for (rebuild_recovery, stop_after_start) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let _reset = ResetStartupHook;
        let (root, source) = due_import_fixture();
        let bound = Arc::new(JournalRoot::open(root.path()).unwrap());
        let retained = Arc::downgrade(&bound);
        let stop = Arc::new(AtomicBool::new(false));
        let constructor_stop = Arc::clone(&stop);
        let (entered_send, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));
        let resources_released = Arc::new(AtomicBool::new(false));
        let resource_state = Arc::clone(&resources_released);
        let observed_stop = Arc::new(AtomicBool::new(false));
        let native_stop = Arc::clone(&observed_stop);
        let factory: Arc<ImportSeamFactory> = Arc::new(move |context| {
            let resource = NativeGuard {
                owner: std::thread::current().id(),
                released: Arc::clone(&resource_state),
            };
            entered_send.send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            native_stop.store(context.stop().is_requested(), Ordering::Release);
            let seams =
                context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
            Ok(PreparedImport::new(context.arguments().to_vec(), seams).with_resources(resource))
        });
        let (at_failure_send, at_failure) = mpsc::channel();
        let hook_stop = Arc::clone(&stop);
        *super::super::STARTUP_AFTER_CATCH_UP.lock().unwrap() = Some(Box::new(move || {
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            at_failure_send.send(()).unwrap();
            if stop_after_start {
                hook_stop.store(true, Ordering::Release);
                Ok(())
            } else {
                Err("synthetic constructor failure after due admission".into())
            }
        }));
        let payload = root.path().join("payload");
        let backup = BackupWebDeps::new(root.path().to_path_buf());
        let (settled_send, settled) = mpsc::channel();
        let constructor = std::thread::spawn(move || {
            let result = ForegroundProcessingHost::open_with_imports(
                bound,
                &payload,
                Arc::new(|| false),
                constructor_stop,
                backup,
                factory,
                rebuild_recovery,
                false,
            );
            let result = match result {
                Ok(host) => {
                    host.shutdown().unwrap();
                    Ok(())
                }
                Err(error) => Err(error),
            };
            settled_send.send(result).unwrap();
        });
        let reached = at_failure.recv_timeout(Duration::from_secs(10));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let stop_seen = stop.load(Ordering::Acquire);
        let premature = settled.recv_timeout(Duration::from_millis(50));
        let root_retained = retained.upgrade().is_some();
        let resources_retained = !resources_released.load(Ordering::Acquire);
        let _ = release.send(());
        let result = match &premature {
            Ok(result) => result.clone(),
            Err(_) => settled.recv_timeout(Duration::from_secs(10)).unwrap(),
        };
        constructor.join().unwrap();
        let root_retired_on_return = retained.upgrade().is_none();
        // Retire a deliberately broken asynchronous-constructor control before
        // asserting its captured early acknowledgment, so failure leaves no work.
        let deadline = Instant::now() + Duration::from_secs(10);
        while retained.upgrade().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            reached.is_ok(),
            "actual due preparation must precede the constructor failure"
        );
        assert!(
            stop_seen,
            "constructor failure must signal the original host Stop"
        );
        assert!(
            premature.is_err(),
            "constructor returned before native preparation retired"
        );
        assert!(root_retained);
        assert!(resources_retained);
        assert!(resources_released.load(Ordering::Acquire));
        assert!(observed_stop.load(Ordering::Acquire));
        assert!(root_retired_on_return);
        assert!(retained.upgrade().is_none());
        assert_eq!(
            result,
            Err(if stop_after_start {
                "thinking cancelled"
            } else {
                "synthetic constructor failure after due admission"
            }
            .to_owned())
        );
        assert!(!root.path().join("chronicle").exists());
        assert!(!root.path().join("imports/obsidian.json").exists());

        // Error/Stop releases the original generation only after its native
        // frame joins. Reopening preserves the stored due job and really runs it.
        let reopened = open_import_fixture(
            root.path(),
            Arc::new(|context| {
                let seams =
                    context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
                Ok(PreparedImport::new(context.arguments().to_vec(), seams))
            }),
        )
        .unwrap();
        let terminal = startup_import_finished(&reopened);
        reopened.shutdown().unwrap();
        assert_eq!(terminal, "ok");
        assert!(contains(
            &root.path().join("chronicle"),
            "fc3 constructor catch-up native import sentinel"
        ));
        assert_eq!(
            fs::read_to_string(source.path().join("startup.md")).unwrap(),
            "fc3 constructor catch-up native import sentinel\n"
        );
    }
}

#[test]
fn preinstalled_imports_preserve_the_existing_rebuild_recovery_choice() {
    use solstone_core_system::{work_intent, work_inventory};
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for rebuild_recovery in [false, true] {
        let root = fixture_directory();
        let day = "20260104";
        fs::create_dir_all(root.path().join("chronicle").join(day)).unwrap();
        let bound = Arc::new(JournalRoot::open(root.path()).unwrap());
        let intent = work_intent::register(root.path(), day, true).unwrap();
        let lease = work_intent::begin_rebuild(
            root.path(),
            day,
            &intent.id,
            &bound.identity().observation_id(),
        )
        .unwrap();
        work_intent::update_rebuild(root.path(), day, &intent.id, &|| Ok(()), |progress| {
            progress.segments = Some(work_intent::SegmentSeal {
                generation: 1,
                raw_fingerprint: "retained fixture inputs".into(),
                receipts_digest: "retained fixture receipts".into(),
            });
            Ok(())
        })
        .unwrap();
        drop(lease);
        let retained = work_intent::read(root.path()).unwrap()[day].clone();
        assert!(work_intent::can_resume(root.path(), day, &retained).unwrap());
        // No thinking provider is configured. Discovery must distinguish the
        // caller's recovery choice before the existing spend refusal applies.
        let host = ForegroundProcessingHost::open_with_imports(
            bound,
            &root.path().join("payload"),
            Arc::new(|| false),
            Arc::new(AtomicBool::new(false)),
            BackupWebDeps::new(root.path().to_path_buf()),
            Arc::new(|_| panic!("no importer work was requested")),
            rebuild_recovery,
            false,
        )
        .unwrap();
        host.discovery.lock().unwrap().as_ref().unwrap().request();
        let deadline = Instant::now() + Duration::from_secs(10);
        let reason = loop {
            if let Some(report) = work_inventory::read(root.path()).unwrap()
                && let Some(hold) = report.holds.iter().find(|hold| hold.day == day)
            {
                break hold.reason.clone();
            }
            assert!(
                Instant::now() < deadline,
                "recovery choice was not inspected"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        host.shutdown().unwrap();
        assert_eq!(
            reason,
            if rebuild_recovery {
                "thinking_unavailable"
            } else {
                "interrupted_from_scratch"
            },
            "the native caller's existing recovery mode controls discovery"
        );
        assert_eq!(work_intent::read(root.path()).unwrap()[day], retained);
        assert_eq!(
            fs::read_dir(root.path().join("chronicle").join(day))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn queued_import_writes_real_notes_and_unresolved_sync_cannot_borrow_retained_path() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (root, host) = fixture();
    let source = tempfile::tempdir().unwrap();
    let note = "# native import\n\nfc3 explicit native source sentinel\n";
    fs::write(source.path().join("note.md"), note).unwrap();
    assert_eq!(
        host.execute_worker_command(&owned(&["importer", "--list-importers"])),
        UNCOMPLETED_HOLD_EXIT
    );
    host.shutdown().unwrap();
    drop(host);
    let host = open_import_fixture(
        root.path(),
        Arc::new(|request| {
            let seams =
                request.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
            Ok(PreparedImport::new(request.arguments().to_vec(), seams))
        }),
    )
    .unwrap();
    submit(
        &host,
        "explicit-import",
        owned(&[
            "solstone",
            "journal",
            "importer",
            "--sync",
            "obsidian",
            "--save",
            "--path",
            source.path().to_str().unwrap(),
        ]),
    );
    assert_eq!(finished(&host, "explicit-import"), "ok");
    let state_path = root.path().join("imports/obsidian.json");
    let before = fs::read(&state_path).unwrap();
    let state: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(state["files"]["note.md"]["status"], "imported");
    assert!(contains(
        &root.path().join("chronicle"),
        "fc3 explicit native source sentinel"
    ));
    assert_eq!(
        fs::read_to_string(source.path().join("note.md")).unwrap(),
        note
    );

    // The real explicit run established valid retained source B. A new note
    // would be eligible there if the missing-connection request fell through.
    fs::write(
        source.path().join("later.md"),
        "fc3 retained source must stay unread\n",
    )
    .unwrap();
    for source in ["obsidian", "audio"] {
        assert_eq!(
            host.execute_worker_command(&owned(&["importer", "--sync", source, "--save"])),
            UNCOMPLETED_HOLD_EXIT
        );
    }
    assert!(!root.path().join("imports/audio.json").exists());
    assert_eq!(fs::read(state_path).unwrap(), before);
    assert!(!contains(
        &root.path().join("chronicle"),
        "fc3 retained source must stay unread"
    ));
    assert!(!admit(&owned(&["--connect", "oura"])));
    assert!(!admit(&owned(&["--sync", "oura", "--save"])));
    assert!(admit(&owned(&["--sync", "oura", "--save", "--scheduled"])));
    host.shutdown().unwrap();
    assert!(
        host.import_preparation(WorkStop::new(), Default::default())
            .is_err()
    );
}

struct NativeGuard {
    owner: std::thread::ThreadId,
    released: Arc<AtomicBool>,
}
impl Drop for NativeGuard {
    fn drop(&mut self) {
        assert_eq!(self.owner, std::thread::current().id());
        self.released.store(true, Ordering::Release);
    }
}

struct SharedPdf {
    entered: mpsc::Sender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
    observed_stop: Arc<AtomicBool>,
}
impl PdfWorker for SharedPdf {
    fn execute(&self, _: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        self.observed_stop.store(
            !solstone_core_import::work_stop::work_continues(),
            Ordering::Release,
        );
        Err(WorkerFailure::Cancelled)
    }
}

fn held_import(preparation: bool) {
    let root = fixture_directory();
    let pdf = root.path().join("held.pdf");
    fs::write(&pdf, b"%PDF-1.1\n").unwrap();
    let (entered_send, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    let resources_released = Arc::new(AtomicBool::new(false));
    let resource_state = Arc::clone(&resources_released);
    let observed_stop = Arc::new(AtomicBool::new(false));
    let native_stop = Arc::clone(&observed_stop);
    let host = open_import_fixture(
        root.path(),
        Arc::new(move |request| {
            let guard = NativeGuard {
                owner: std::thread::current().id(),
                released: Arc::clone(&resource_state),
            };
            let worker: Box<dyn PdfWorker + Send> = if preparation {
                entered_send.send(()).unwrap();
                released.lock().unwrap().recv().unwrap();
                Box::new(solstone_core_import_sources::NullPdfWorker)
            } else {
                // Only one native entry is expected, and it remains behind this
                // gate until the test releases the original operation.
                Box::new(SharedPdf {
                    entered: entered_send.clone(),
                    release: Arc::clone(&released),
                    observed_stop: Arc::clone(&native_stop),
                })
            };
            let seams = request.seams_with_pdf(worker);
            Ok(PreparedImport::new(request.arguments().to_vec(), seams).with_resources(guard))
        }),
    )
    .unwrap();
    submit(
        &host,
        "held-import",
        owned(&[
            "importer",
            "--source",
            "document",
            pdf.to_str().unwrap(),
            "--timestamp",
            "20260809_090000",
        ]),
    );
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let bound_root = Arc::downgrade(&host.root);
    host.request_stop();
    let (drained_send, drained) = mpsc::channel();
    let retirement = std::thread::spawn(move || {
        let result = host.shutdown();
        drained_send.send(result).unwrap();
    });
    assert!(drained.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(bound_root.upgrade().is_some());
    assert!(!resources_released.load(Ordering::Acquire));
    release.send(()).unwrap();
    drained
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    retirement.join().unwrap();
    assert!(resources_released.load(Ordering::Acquire));
    if !preparation {
        assert!(observed_stop.load(Ordering::Acquire));
    }
    assert!(bound_root.upgrade().is_none());
    assert!(!root.path().join("chronicle").exists());
    assert_eq!(fs::read(pdf).unwrap(), b"%PDF-1.1\n");
}

#[test]
fn stop_joins_blocked_preparation_before_releasing_native_resources() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    held_import(true);
}

#[test]
fn stop_joins_native_import_before_releasing_original_worker_resources() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    held_import(false);
}

#[test]
fn owner_request_holds_admitted_use_and_same_owner_previews_and_runs_after_resume() {
    use solstone_core_convey_shell::recovery_drain::{
        register_recovery_hooks, relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    fn can_queue<T: Send>() {}
    can_queue::<ImportRequest>();
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = fixture_directory();
    let vault = tempfile::tempdir().unwrap();
    fs::write(
        vault.path().join("resumed.md"),
        "fc3 resumed owner import sentinel\n",
    )
    .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let factory_calls = Arc::clone(&calls);
    let host = open_import_fixture(
        root.path(),
        Arc::new(move |request| {
            factory_calls.fetch_add(1, Ordering::AcqRel);
            let seams =
                request.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
            Ok(PreparedImport::new(request.arguments().to_vec(), seams))
        }),
    )
    .unwrap();
    let args = owned(&[
        "--sync",
        "obsidian",
        "--save",
        "--path",
        vault.path().to_str().unwrap(),
    ]);
    let queued = host
        .import_request(args.clone(), WorkStop::new(), Default::default())
        .unwrap();
    let (pause_send, paused) = mpsc::channel();
    let resumes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let resume_count = Arc::clone(&resumes);
    let hooks = register_recovery_hooks(
        "fc3-import-request-proof",
        Arc::clone(&host.root),
        move |_| {
            pause_send.send(()).unwrap();
            Ok(())
        },
        move |_| {
            resume_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        },
    );
    let target = root.path().to_path_buf();
    let (drain_send, drained) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        let result = relinquish_admitted_work(&target);
        drain_send.send(result).unwrap();
    });
    paused.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(drained.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(
        host.import_request(args.clone(), WorkStop::new(), Default::default())
            .is_err()
    );
    assert!(queued.prepare().is_err());
    assert_eq!(calls.load(Ordering::Acquire), 0);
    drained
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    assert!(!root.path().join("chronicle").exists());
    resume_after_whole_journal_restore(root.path());
    assert_eq!(resumes.load(Ordering::Acquire), 1);
    let prepared = host
        .import_request(args, WorkStop::new(), Default::default())
        .unwrap()
        .prepare()
        .unwrap();
    let preview = prepared.preview();
    assert_eq!(preview.exit_code, 0, "{}", preview.stderr);
    assert!(!root.path().join("chronicle").exists());
    let (run, _) = prepared.run().unwrap();
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(contains(
        &root.path().join("chronicle"),
        "fc3 resumed owner import sentinel"
    ));
    drop(hooks);
    host.shutdown().unwrap();
}

#[test]
fn opaque_keepalive_retains_outer_preview_work_after_worker_local_resources_retire() {
    use solstone_core_convey_shell::recovery_drain::{
        register_recovery_hooks, relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    fn outer_handle<T: Clone + Send + Sync>() {}
    outer_handle::<ImportKeepalive>();
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = fixture_directory();
    let source = tempfile::tempdir().unwrap();
    fs::write(
        source.path().join("preview.md"),
        "retained preview source\n",
    )
    .unwrap();
    let released = Arc::new(AtomicBool::new(false));
    let resource_state = Arc::clone(&released);
    let host = open_import_fixture(
        root.path(),
        Arc::new(move |request| {
            let seams =
                request.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
            let guard = NativeGuard {
                owner: std::thread::current().id(),
                released: Arc::clone(&resource_state),
            };
            Ok(PreparedImport::new(request.arguments().to_vec(), seams)
                .with_resources((std::rc::Rc::new(()), guard)))
        }),
    )
    .unwrap();
    let args = owned(&[
        "--sync",
        "obsidian",
        "--save",
        "--path",
        source.path().to_str().unwrap(),
    ]);
    let request = host
        .import_request(args.clone(), WorkStop::new(), Default::default())
        .unwrap();
    let outer = request.keepalive();
    let final_metadata_owner = outer.clone();
    let preview = std::thread::spawn(move || {
        let prepared = request.prepare().unwrap();
        let result = prepared.preview();
        drop(prepared);
        result
    })
    .join()
    .unwrap();
    assert_eq!(preview.exit_code, 0, "{}", preview.stderr);
    assert!(
        released.load(Ordering::Acquire),
        "non-Send resources retired on the preparation worker"
    );
    assert!(!root.path().join("chronicle").exists());
    let (pause_send, paused) = mpsc::channel();
    let hooks = register_recovery_hooks(
        "fc3-import-outer-preview-proof",
        Arc::clone(&host.root),
        move |_| {
            pause_send.send(()).unwrap();
            Ok(())
        },
        |_| Ok(()),
    );
    let target = root.path().to_path_buf();
    let (drain_send, drained) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        drain_send.send(relinquish_admitted_work(&target)).unwrap();
    });
    paused.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(drained.recv_timeout(Duration::from_millis(50)).is_err());
    drop(outer);
    assert!(
        drained.recv_timeout(Duration::from_millis(50)).is_err(),
        "outer metadata ownership outlives the already-joined native preview"
    );
    assert!(
        host.import_request(args.clone(), WorkStop::new(), Default::default())
            .is_err()
    );
    drop(final_metadata_owner);
    drained
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    resume_after_whole_journal_restore(root.path());
    drop(
        host.import_request(args, WorkStop::new(), Default::default())
            .unwrap(),
    );
    drop(hooks);
    host.shutdown().unwrap();
}

enum Interrupt {
    OriginalRequest,
    Deadline,
    Recovery,
}

#[test]
fn pre_argv_keepalive_drains_cancelled_copy_cleanup_before_restore_can_write() {
    use solstone_core_convey_shell::recovery_drain::{
        register_recovery_hooks, relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    fn movable<T: Send + Sync>() {}
    movable::<ImportPreparation>();
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = fixture_directory();
    let factory_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = Arc::clone(&factory_calls);
    let host = open_import_fixture(
        root.path(),
        Arc::new(move |context| {
            calls.fetch_add(1, Ordering::AcqRel);
            let seams =
                context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
            Ok(PreparedImport::new(context.arguments().to_vec(), seams))
        }),
    )
    .unwrap();
    let original = WorkStop::new();
    let preparation = host
        .import_preparation(original.clone(), Default::default())
        .unwrap();
    let cleanup_use = preparation.keepalive();
    let source = tempfile::NamedTempFile::new().unwrap();
    fs::write(source.path(), b"cancelled snapshot bytes").unwrap();
    let destination = root.path().join("health/pre-argv-copy");
    let copied_path = destination.clone();
    let source_path = source.path().to_path_buf();
    let (copied_send, copied) = mpsc::channel();
    let (cleanup_send, cleanup) = mpsc::channel();
    let copy_cleanup = std::thread::spawn(move || {
        let _use_until_cleanup_retires = cleanup_use;
        fs::copy(source_path, &copied_path).unwrap();
        copied_send.send(()).unwrap();
        // A disconnected caller's cleanup may retire after its preparation.
        cleanup.recv_timeout(Duration::from_secs(5)).unwrap();
        fs::remove_file(copied_path).unwrap();
    });
    copied.recv_timeout(Duration::from_secs(5)).unwrap();
    original.request();
    assert!(preparation.stop().is_requested());
    assert!(preparation.checkpoint().is_err());
    assert!(preparation.bind(owned(&["--list-importers"])).is_err());
    assert_eq!(factory_calls.load(Ordering::Acquire), 0);
    assert!(!root.path().join("imports").exists());
    let (pause_send, paused) = mpsc::channel();
    let hooks = register_recovery_hooks(
        "fc3-pre-argv-cleanup-proof",
        Arc::clone(&host.root),
        move |_| {
            pause_send.send(()).unwrap();
            Ok(())
        },
        |_| Ok(()),
    );
    let target = root.path().to_path_buf();
    let (drain_send, drained) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        drain_send.send(relinquish_admitted_work(&target)).unwrap();
    });
    paused.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(drained.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(
        destination.exists(),
        "cleanup is still a live admitted effect"
    );
    assert!(
        host.import_preparation(WorkStop::new(), Default::default())
            .is_err()
    );
    cleanup_send.send(()).unwrap();
    copy_cleanup.join().unwrap();
    drained
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    assert!(!destination.exists());
    resume_after_whole_journal_restore(root.path());
    drop(
        host.import_preparation(WorkStop::new(), Default::default())
            .unwrap(),
    );
    drop(hooks);
    host.shutdown().unwrap();
}

#[test]
fn pre_argv_snapshot_binds_once_then_shared_preview_and_execution_write_actual_notes() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = fixture_directory();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let factory_calls = Arc::clone(&calls);
    let host = open_import_fixture(
        root.path(),
        Arc::new(move |context| {
            factory_calls.fetch_add(1, Ordering::AcqRel);
            let seams =
                context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
            Ok(PreparedImport::new(context.arguments().to_vec(), seams))
        }),
    )
    .unwrap();
    let preparation = host
        .import_preparation(WorkStop::new(), Default::default())
        .unwrap();
    let source = tempfile::NamedTempFile::new().unwrap();
    fs::write(source.path(), "fc3 pre-argv snapshot native output\n").unwrap();
    let snapshot = root.path().join("health/prepared-input");
    preparation.checkpoint().unwrap();
    fs::create_dir(&snapshot).unwrap();
    fs::copy(source.path(), snapshot.join("prepared.md")).unwrap();
    let request = preparation
        .bind(owned(&[
            "--sync",
            "obsidian",
            "--save",
            "--path",
            snapshot.to_str().unwrap(),
        ]))
        .unwrap();
    assert_eq!(
        calls.load(Ordering::Acquire),
        0,
        "binding does not prepare or admit an attempt"
    );
    assert!(!root.path().join("imports").exists());
    assert!(!root.path().join("chronicle").exists());
    let (preview, run) = std::thread::spawn(move || {
        let prepared = request.prepare().unwrap();
        (prepared.preview(), prepared.run().unwrap().0)
    })
    .join()
    .unwrap();
    assert_eq!(preview.exit_code, 0, "{}", preview.stderr);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(contains(
        &root.path().join("chronicle"),
        "fc3 pre-argv snapshot native output"
    ));
    host.shutdown().unwrap();
}

#[test]
fn pre_argv_binding_preserves_original_deadline_host_stop_and_root_authority() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for interrupt in ["deadline", "host", "root"] {
        let root = fixture_directory();
        let host = open_import_fixture(
            root.path(),
            Arc::new(|_| panic!("refused pre-argv work cannot prepare")),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_millis(100);
        let original = if interrupt == "deadline" {
            WorkStop::linked(Arc::new(AtomicBool::new(false)), Some(deadline))
        } else {
            WorkStop::new()
        };
        let preparation = host
            .import_preparation(original, Default::default())
            .unwrap();
        let retained = tempfile::tempdir().unwrap();
        match interrupt {
            "deadline" => std::thread::sleep(deadline.saturating_duration_since(Instant::now())),
            "host" => host.request_stop(),
            "root" => {
                fs::rename(root.path(), retained.path().join("original")).unwrap();
                fs::create_dir(root.path()).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(preparation.checkpoint().is_err(), "{interrupt}");
        assert!(
            preparation.bind(owned(&["--list-importers"])).is_err(),
            "{interrupt}"
        );
        host.shutdown().unwrap();
        if interrupt == "root" {
            assert_eq!(
                fs::read_dir(root.path()).unwrap().count(),
                0,
                "replacement root stays empty"
            );
        } else {
            assert!(!root.path().join("imports").exists());
            assert!(!root.path().join("chronicle").exists());
        }
    }
}

fn held_owner_request(preparation: bool, interrupt: Interrupt) {
    use solstone_core_convey_shell::recovery_drain::{
        register_recovery_hooks, relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    let root = fixture_directory();
    let pdf = root.path().join("owner-held.pdf");
    fs::write(&pdf, b"%PDF-1.1\n").unwrap();
    let (entered_send, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    let resources_released = Arc::new(AtomicBool::new(false));
    let resource_state = Arc::clone(&resources_released);
    let observed_stop = Arc::new(AtomicBool::new(false));
    let native_stop = Arc::clone(&observed_stop);
    let host = open_import_fixture(
        root.path(),
        Arc::new(move |context| {
            let guard = NativeGuard {
                owner: std::thread::current().id(),
                released: Arc::clone(&resource_state),
            };
            let worker: Box<dyn PdfWorker + Send> = if preparation {
                entered_send.send(()).unwrap();
                released.lock().unwrap().recv().unwrap();
                Box::new(solstone_core_import_sources::NullPdfWorker)
            } else {
                Box::new(SharedPdf {
                    entered: entered_send.clone(),
                    release: Arc::clone(&released),
                    observed_stop: Arc::clone(&native_stop),
                })
            };
            let seams = context.seams_with_pdf(worker);
            Ok(PreparedImport::new(context.arguments().to_vec(), seams).with_resources(guard))
        }),
    )
    .unwrap();
    let original = WorkStop::new();
    let deadline =
        matches!(interrupt, Interrupt::Deadline).then(|| Instant::now() + Duration::from_secs(2));
    let stop = if deadline.is_some() {
        WorkStop::linked(Arc::new(AtomicBool::new(false)), deadline)
    } else {
        original.clone()
    };
    let request = host
        .import_request(
            owned(&[
                "--source",
                "document",
                pdf.to_str().unwrap(),
                "--timestamp",
                "20260809_090000",
            ]),
            stop.clone(),
            Default::default(),
        )
        .unwrap();
    let (result_send, result) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let outcome = request
            .prepare()
            .and_then(|prepared| prepared.run().map_err(|error| error.to_string()));
        result_send.send(outcome).unwrap();
    });
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut recovery = None;
    match interrupt {
        Interrupt::OriginalRequest => original.request(),
        Interrupt::Deadline => {
            let deadline = deadline.unwrap();
            assert!(
                Instant::now() < deadline,
                "native entry must precede deadline"
            );
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            assert!(stop.is_requested());
        }
        Interrupt::Recovery => {
            let (pause_send, paused) = mpsc::channel();
            let hooks = register_recovery_hooks(
                "fc3-native-import-proof",
                Arc::clone(&host.root),
                move |_| {
                    pause_send.send(()).unwrap();
                    Ok(())
                },
                |_| Ok(()),
            );
            let target = root.path().to_path_buf();
            let guard_released = Arc::clone(&resources_released);
            let (drain_send, drained) = mpsc::channel();
            let drain = std::thread::spawn(move || {
                let outcome = relinquish_admitted_work(&target);
                assert!(guard_released.load(Ordering::Acquire));
                drain_send.send(outcome).unwrap();
            });
            paused.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(drained.recv_timeout(Duration::from_millis(50)).is_err());
            recovery = Some((hooks, drain, drained));
        }
    }
    assert!(result.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(!resources_released.load(Ordering::Acquire));
    release.send(()).unwrap();
    let outcome = result.recv_timeout(Duration::from_secs(10)).unwrap();
    if preparation {
        assert!(outcome.is_err());
    } else {
        assert_ne!(outcome.unwrap().0.exit_code, 0);
    }
    worker.join().unwrap();
    assert!(resources_released.load(Ordering::Acquire));
    if !preparation {
        assert!(observed_stop.load(Ordering::Acquire));
    }
    assert!(!root.path().join("chronicle").exists());
    assert_eq!(fs::read(pdf).unwrap(), b"%PDF-1.1\n");
    if let Some((hooks, drain, drained)) = recovery {
        drained
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        drain.join().unwrap();
        resume_after_whole_journal_restore(root.path());
        let prepared = host
            .import_request(
                owned(&["--list-importers"]),
                WorkStop::new(),
                Default::default(),
            )
            .unwrap()
            .prepare()
            .unwrap();
        assert_eq!(prepared.preview().exit_code, 0);
        assert_eq!(prepared.run().unwrap().0.exit_code, 0);
        drop(hooks);
    }
    host.shutdown().unwrap();
}

#[test]
fn original_request_cancellation_is_retained_through_blocked_preparation() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    held_owner_request(true, Interrupt::OriginalRequest);
}

#[test]
fn original_deadline_joins_blocked_native_frame_before_resource_release() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    held_owner_request(false, Interrupt::Deadline);
}

#[test]
fn recovery_drains_native_frame_and_original_resources_then_same_owner_resumes() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    held_owner_request(false, Interrupt::Recovery);
}

#[test]
fn late_stop_after_native_commit_preserves_actual_success() {
    struct StopOnDrop(WorkStop);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.request();
        }
    }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let root = fixture_directory();
    let source = tempfile::tempdir().unwrap();
    fs::write(
        source.path().join("late.md"),
        "fc3 success before late Stop\n",
    )
    .unwrap();
    let original = WorkStop::new();
    let late_stop = original.clone();
    let host = open_import_fixture(
        root.path(),
        Arc::new(move |context| {
            let seams =
                context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
            Ok(PreparedImport::new(context.arguments().to_vec(), seams)
                .with_resources(StopOnDrop(late_stop.clone())))
        }),
    )
    .unwrap();
    let (outcome, _) = host
        .import_request(
            owned(&[
                "--sync",
                "obsidian",
                "--save",
                "--path",
                source.path().to_str().unwrap(),
            ]),
            original.clone(),
            Default::default(),
        )
        .unwrap()
        .prepare()
        .unwrap()
        .run()
        .unwrap();
    assert!(original.is_requested());
    assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
    assert!(contains(
        &root.path().join("chronicle"),
        "fc3 success before late Stop"
    ));
    host.shutdown().unwrap();
}
