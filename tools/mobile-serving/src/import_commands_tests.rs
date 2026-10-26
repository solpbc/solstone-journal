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

fn fixture() -> (tempfile::TempDir, ForegroundProcessingHost) {
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
    host.enable_imports(Arc::new(|request| {
        let seams = request.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
        Ok(PreparedImport::new(request.arguments().to_vec(), seams))
    }))
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
        host.enable_imports(Arc::new(|_| panic!("stopped factory must not run")))
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
    let (root, host) = fixture();
    let pdf = root.path().join("held.pdf");
    fs::write(&pdf, b"%PDF-1.1\n").unwrap();
    let (entered_send, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    let resources_released = Arc::new(AtomicBool::new(false));
    let resource_state = Arc::clone(&resources_released);
    let observed_stop = Arc::new(AtomicBool::new(false));
    let native_stop = Arc::clone(&observed_stop);
    host.enable_imports(Arc::new(move |request| {
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
    }))
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
    let (root, host) = fixture();
    let vault = tempfile::tempdir().unwrap();
    fs::write(
        vault.path().join("resumed.md"),
        "fc3 resumed owner import sentinel\n",
    )
    .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let factory_calls = Arc::clone(&calls);
    host.enable_imports(Arc::new(move |request| {
        factory_calls.fetch_add(1, Ordering::AcqRel);
        let seams = request.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
        Ok(PreparedImport::new(request.arguments().to_vec(), seams))
    }))
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

enum Interrupt {
    OriginalRequest,
    Deadline,
    Recovery,
}

fn held_owner_request(preparation: bool, interrupt: Interrupt) {
    use solstone_core_convey_shell::recovery_drain::{
        register_recovery_hooks, relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    let (root, host) = fixture();
    let pdf = root.path().join("owner-held.pdf");
    fs::write(&pdf, b"%PDF-1.1\n").unwrap();
    let (entered_send, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    let resources_released = Arc::new(AtomicBool::new(false));
    let resource_state = Arc::clone(&resources_released);
    let observed_stop = Arc::new(AtomicBool::new(false));
    let native_stop = Arc::clone(&observed_stop);
    host.enable_imports(Arc::new(move |context| {
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
    }))
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
    let (root, host) = fixture();
    let source = tempfile::tempdir().unwrap();
    fs::write(
        source.path().join("late.md"),
        "fc3 success before late Stop\n",
    )
    .unwrap();
    let original = WorkStop::new();
    let late_stop = original.clone();
    host.enable_imports(Arc::new(move |context| {
        let seams = context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
        Ok(PreparedImport::new(context.arguments().to_vec(), seams)
            .with_resources(StopOnDrop(late_stop.clone())))
    }))
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
