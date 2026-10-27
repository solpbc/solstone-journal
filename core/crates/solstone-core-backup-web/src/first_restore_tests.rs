// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use super::*;
use solstone_core_convey_shell::recovery_drain::{
    PausedRecovery, RecoveryHooks, pause_admitted_work, register_recovery_hooks,
};
use solstone_core_journal_io::{
    JournalRoot,
    whole_journal_restore::{
        WholeJournalRestoreReadiness, begin_target_use_bound, read_whole_journal_restore,
    },
};
use std::sync::mpsc;

// libtest and Convey's dependency use separate instances of this crate.
// Bridge those test traits to the same concrete production phase owner.
struct Lifecycle;
struct Phase(PausedRecovery);

impl crate::EmbeddedRestoreLifecycle for Lifecycle {
    fn drain(&self, root: &Arc<JournalRoot>) -> Result<Box<dyn crate::PausedRestore>, String> {
        pause_admitted_work(root.clone())
            .map(|phase| Box::new(Phase(phase)) as Box<dyn crate::PausedRestore>)
    }
}

impl crate::PausedRestore for Phase {
    fn resume(self: Box<Self>, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        self.0.resume(cancelled)
    }
}

struct Worker {
    release: mpsc::Sender<()>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(join) = self.join.take() {
            join.join().unwrap();
        }
    }
}

struct CopyRunner {
    root: PathBuf,
    copies: AtomicUsize,
    saw_record: AtomicBool,
    partial_once: AtomicBool,
    wrong_key: bool,
    check_exit: i32,
    after_copy: Mutex<Option<CopyHold>>,
    tls_hold: Mutex<Option<TlsHold>>,
}

struct TlsHold {
    root: Arc<JournalRoot>,
    entered: tokio::sync::oneshot::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl Drop for TlsHold {
    fn drop(&mut self) {
        let (replacement, _) = tokio::sync::oneshot::channel();
        let entered = std::mem::replace(&mut self.entered, replacement);
        let _ = entered.send(());
        if self.release.recv_timeout(FIXTURE_WAIT).is_ok()
            && self.root.revalidate_canonical_binding().is_ok()
        {
            fs::write(
                self.root.canonical_path().join("native-tls-finished"),
                b"joined cleanup",
            )
            .unwrap();
        }
    }
}

thread_local! {
    static NATIVE_TLS: std::cell::RefCell<Option<TlsHold>> = const { std::cell::RefCell::new(None) };
}

struct CopyHold {
    entered: tokio::sync::oneshot::Sender<()>,
    cancelled: tokio::sync::oneshot::Sender<()>,
    release: mpsc::Receiver<()>,
}

struct ResumeHold {
    entered: tokio::sync::oneshot::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl ToolRunner for CopyRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        match request.argv.first().and_then(|s| s.to_str()) {
            Some("snapshots") => Ok(if self.wrong_key {
                output(12, "")
            } else {
                output(0, journal_catalog())
            }),
            Some("restore") => {
                self.copies.fetch_add(1, Ordering::SeqCst);
                self.saw_record.store(
                    matches!(read_whole_journal_restore(&self.root),
                        WholeJournalRestoreReadiness::Pending(record)
                        if record.snapshot_id == JOURNAL_SNAPSHOT_ID && record.subtree == "/original"),
                    Ordering::SeqCst,
                );
                let target = request
                    .argv
                    .windows(2)
                    .find(|a| a[0] == "--target")
                    .unwrap();
                let target = Path::new(&target[1]);
                fs::write(target.join("restored-original"), b"original bytes")?;
                if self.partial_once.swap(false, Ordering::SeqCst) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "partial fixture copy",
                    ));
                }
                fs::write(target.join("restored-derived"), b"derived bytes")?;
                Ok(output(0, restore_summary()))
            }
            Some("check") => Ok(output(self.check_exit, "")),
            other => Err(io::Error::other(format!(
                "unexpected fixture command {other:?}"
            ))),
        }
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        cancellation: &solstone_core_backup::archive_admission::ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        if request.argv.first().is_some_and(|arg| arg == "restore")
            && let Some(hold) = self.tls_hold.lock().unwrap().take()
        {
            NATIVE_TLS.with(|slot| *slot.borrow_mut() = Some(hold));
        }
        if cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "fixture stopped",
            ));
        }
        let output = self.run(request);
        if request.argv.first().is_some_and(|arg| arg == "restore") {
            let hold = self.after_copy.lock().unwrap().take();
            if let Some(hold) = hold {
                let _ = hold.entered.send(());
                let mut observed = Some(hold.cancelled);
                let deadline = Instant::now() + FIXTURE_WAIT;
                loop {
                    if cancellation.is_cancelled()
                        && let Some(sender) = observed.take()
                    {
                        let _ = sender.send(());
                    }
                    match hold.release.recv_timeout(Duration::from_millis(5)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if Instant::now() >= deadline {
                                return Err(io::Error::new(
                                    io::ErrorKind::TimedOut,
                                    "copy hold expired",
                                ));
                            }
                        }
                    }
                }
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "held copy returned",
                ));
            }
        }
        output
    }
}

struct Fixture {
    worker: Arc<Mutex<Option<Worker>>>,
    _hooks: RecoveryHooks,
    root: tempfile::TempDir,
    retained: Arc<JournalRoot>,
    package: tempfile::TempDir,
    deps: crate::BackupWebDeps,
    runner: Arc<CopyRunner>,
    pauses: Arc<AtomicUsize>,
    resumes: Arc<AtomicUsize>,
    resume_hold: Arc<Mutex<Option<ResumeHold>>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let worker = self.worker.lock().unwrap().take();
        drop(worker);
    }
}

impl Fixture {
    fn new(partial: bool, wrong_key: bool, check_exit: i32) -> Self {
        let root = crate::test_support::root("healthy");
        let retained = Arc::new(JournalRoot::open(root.path()).unwrap());
        let admitted = begin_target_use_bound(retained.clone()).unwrap();
        let (release, released) = mpsc::channel();
        let path = root.path().to_owned();
        let join = std::thread::spawn(move || {
            let _admitted = admitted;
            released.recv_timeout(FIXTURE_WAIT).unwrap();
            fs::write(path.join("producer-joined"), b"finished before copy").unwrap();
        });
        let worker = Arc::new(Mutex::new(Some(Worker {
            release,
            join: Some(join),
        })));
        let pauses = Arc::new(AtomicUsize::new(0));
        let resumes = Arc::new(AtomicUsize::new(0));
        let stop = worker.clone();
        let paused = pauses.clone();
        let resumed = resumes.clone();
        let resume_hold = Arc::new(Mutex::new(None::<ResumeHold>));
        let resume_gate = resume_hold.clone();
        let resume_root = retained.clone();
        let hooks = register_recovery_hooks(
            "first-restore-consumer",
            retained.clone(),
            move |journal| {
                if !matches!(
                    read_whole_journal_restore(journal),
                    WholeJournalRestoreReadiness::Pending(_)
                ) {
                    return Err("drain preceded durable selection".into());
                }
                let worker = stop.lock().unwrap().take();
                drop(worker);
                paused.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |journal| {
                let hold = resume_gate.lock().unwrap().take();
                if let Some(hold) = hold {
                    let _ = hold.entered.send(());
                    hold.release
                        .recv_timeout(FIXTURE_WAIT)
                        .map_err(|e| e.to_string())?;
                }
                let _admitted =
                    begin_target_use_bound(resume_root.clone()).map_err(|e| e.to_string())?;
                let bytes =
                    fs::read(journal.join("restored-derived")).map_err(|e| e.to_string())?;
                fs::write(journal.join("resumed-read"), bytes).map_err(|e| e.to_string())?;
                resumed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        let runner = Arc::new(CopyRunner {
            root: root.path().to_owned(),
            copies: AtomicUsize::new(0),
            saw_record: AtomicBool::new(false),
            partial_once: AtomicBool::new(partial),
            wrong_key,
            check_exit,
            after_copy: Mutex::new(None),
            tls_hold: Mutex::new(None),
        });
        let (prepared, package) = prepared(root.path().to_owned(), runner.clone());
        let mut deps = crate::BackupWebDeps::with_root(retained.clone()).unwrap();
        deps.runner = runner.clone();
        deps.executable = prepared.executable;
        deps.clock = prepared.clock;
        deps.journal_maintenance = Arc::new(OkMaintenance);
        deps.install_restore_lifecycle(Arc::new(Lifecycle));
        Self {
            worker,
            _hooks: hooks,
            root,
            retained,
            package,
            deps,
            runner,
            pauses,
            resumes,
            resume_hold,
        }
    }

    async fn stop(&self) {
        let lifetime = self.deps.lifetime();
        lifetime.request_stop();
        let runtime = tokio::runtime::Handle::current();
        assert_eq!(
            tokio::task::spawn_blocking(
                move || lifetime.drain(&runtime, Instant::now() + FIXTURE_WAIT)
            )
            .await
            .unwrap(),
            crate::BackupRouteDrain::Drained
        );
    }
}

#[tokio::test]
async fn initial_route_records_then_drains_actual_consumer_before_copy_and_resume() {
    let fixture = Fixture::new(false, false, 0);
    assert_eq!(
        post_json(&fixture.deps, "/app/backup/restore", Some(restore_body()))
            .await
            .0,
        200
    );
    let done = wait_terminal(&fixture.deps).await;
    fixture.stop().await;
    assert_eq!(done["operation"]["phase"], "done", "{done}");
    assert!(fixture.runner.saw_record.load(Ordering::SeqCst));
    assert_eq!(fixture.runner.copies.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.pauses.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.resumes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fs::read(fixture.root.path().join("resumed-read")).unwrap(),
        b"derived bytes"
    );
    assert!(matches!(
        read_whole_journal_restore(fixture.root.path()),
        WholeJournalRestoreReadiness::Absent
    ));
}

#[tokio::test]
async fn wrong_key_cannot_publish_selection_pause_consumer_or_copy() {
    let fixture = Fixture::new(false, true, 0);
    let ca = fs::read(fixture.root.path().join("link/ca/cert.pem")).unwrap();
    assert_eq!(
        post_json(&fixture.deps, "/app/backup/restore", Some(restore_body()))
            .await
            .0,
        200
    );
    let done = wait_terminal(&fixture.deps).await;
    fixture.stop().await;
    assert_eq!(done["operation"]["reason_code"], "auth_failed", "{done}");
    assert_eq!(fixture.runner.copies.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.pauses.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.resumes.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::read(fixture.root.path().join("link/ca/cert.pem")).unwrap(),
        ca
    );
    assert!(matches!(
        read_whole_journal_restore(fixture.root.path()),
        WholeJournalRestoreReadiness::Absent
    ));
}

#[tokio::test]
async fn partial_initial_copy_is_recorded_and_reconstructed_retry_resumes_after_completion() {
    let fixture = Fixture::new(true, false, 0);
    assert_eq!(
        post_json(&fixture.deps, "/app/backup/restore", Some(restore_body()))
            .await
            .0,
        200
    );
    let failed = wait_terminal(&fixture.deps).await;
    fixture.stop().await;
    assert_eq!(failed["operation"]["phase"], "error");
    assert!(matches!(
        read_whole_journal_restore(fixture.root.path()),
        WholeJournalRestoreReadiness::Pending(_)
    ));
    assert_eq!(fixture.resumes.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::read(fixture.root.path().join("restored-original")).unwrap(),
        b"original bytes"
    );
    assert!(!fixture.root.path().join("restored-derived").exists());
    let new_root = Arc::new(JournalRoot::open(fixture.root.path()).unwrap());
    let mut retry = crate::BackupWebDeps::with_root(new_root).unwrap();
    retry.runner = fixture.runner.clone();
    retry.executable = fixture.package.path().join("bin/solstone");
    retry.journal_maintenance = Arc::new(OkMaintenance);
    let lifecycle = Lifecycle;
    let retained = retry.retained_root().unwrap();
    let body = serde_json::to_vec(&restore_body()).unwrap();
    let result = tokio::task::spawn_blocking(move || {
        crate::recover_recorded_journal_with(&retry, &body, &mut || {
            crate::EmbeddedRestoreLifecycle::drain(&lifecycle, &retained)
        })
    })
    .await
    .unwrap();
    assert_eq!(result, Ok(()));
    assert_eq!(fixture.runner.copies.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.pauses.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.resumes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fs::read(fixture.root.path().join("resumed-read")).unwrap(),
        b"derived bytes"
    );
    assert!(fixture.retained.revalidate_canonical_binding().is_ok());
    assert!(matches!(
        read_whole_journal_restore(fixture.root.path()),
        WholeJournalRestoreReadiness::Absent
    ));
}

#[tokio::test]
async fn failed_integrity_does_not_publish_mobile_readiness_or_resume() {
    let fixture = Fixture::new(false, false, 1);
    assert_eq!(
        post_json(&fixture.deps, "/app/backup/restore", Some(restore_body()))
            .await
            .0,
        200
    );
    let done = wait_terminal(&fixture.deps).await;
    fixture.stop().await;
    assert_eq!(done["operation"]["phase"], "error", "{done}");
    assert!(matches!(
        read_whole_journal_restore(fixture.root.path()),
        WholeJournalRestoreReadiness::Pending(_)
    ));
    assert_eq!(fixture.resumes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn first_copy_stop_and_active_own_the_actual_generation_until_native_return() {
    let fixture = Fixture::new(false, false, 0);
    let (entered, observing) = tokio::sync::oneshot::channel();
    let (cancelled, observed_cancel) = tokio::sync::oneshot::channel();
    let (release, released) = mpsc::channel();
    *fixture.runner.after_copy.lock().unwrap() = Some(CopyHold {
        entered,
        cancelled,
        release: released,
    });
    assert_eq!(
        post_json(&fixture.deps, "/app/backup/restore", Some(restore_body()))
            .await
            .0,
        200
    );
    tokio::time::timeout(FIXTURE_WAIT, observing)
        .await
        .unwrap()
        .unwrap();
    let active_before =
        crate::recorded_journal_recovery_active(&fixture.deps, &tokio::runtime::Handle::current())
            .unwrap();
    let deps = fixture.deps.clone();
    let mut stop = tokio::task::spawn_blocking(move || {
        crate::stop_recorded_journal(&deps, &tokio::runtime::Handle::current())
    });
    let (returned_early, stop_result) = tokio::time::timeout(FIXTURE_WAIT, async {
        tokio::select! {
            result = &mut stop => (true, Some(result.unwrap())),
            result = observed_cancel => {
                result.unwrap();
                (stop.is_finished(), None)
            }
        }
    })
    .await
    .unwrap();
    release.send(()).unwrap();
    let stop_result = match stop_result {
        Some(result) => result,
        None => tokio::time::timeout(FIXTURE_WAIT, stop)
            .await
            .unwrap()
            .unwrap(),
    };
    let done = wait_terminal(&fixture.deps).await;
    fixture.stop().await;
    eprintln!(
        "first_stop active_before={active_before} returned_early={returned_early} terminal={done}"
    );
    assert!(
        active_before && !returned_early,
        "first-copy generation escaped recovery Stop"
    );
    assert_eq!(stop_result, Ok(()));
    assert_eq!(done["operation"]["phase"], "error");
    assert_eq!(fixture.resumes.load(Ordering::SeqCst), 0);
    assert!(matches!(
        read_whole_journal_restore(fixture.root.path()),
        WholeJournalRestoreReadiness::Pending(_)
    ));
}

#[tokio::test]
async fn recovery_stop_waits_for_initial_worker_native_tls_cleanup() {
    let fixture = Fixture::new(false, false, 0);
    let (entered, observing) = tokio::sync::oneshot::channel();
    let (release, released) = mpsc::channel();
    *fixture.runner.tls_hold.lock().unwrap() = Some(TlsHold {
        root: fixture.retained.clone(),
        entered,
        release: released,
    });
    assert_eq!(
        post_json(&fixture.deps, "/app/backup/restore", Some(restore_body()))
            .await
            .0,
        200
    );
    tokio::time::timeout(FIXTURE_WAIT, observing)
        .await
        .unwrap()
        .unwrap();
    let deps = fixture.deps.clone();
    let mut stop = tokio::task::spawn_blocking(move || {
        crate::stop_recorded_journal(&deps, &tokio::runtime::Handle::current())
    });
    let duplicate_deps = fixture.deps.clone();
    let duplicate = tokio::task::spawn_blocking(move || {
        crate::stop_recorded_journal(&duplicate_deps, &tokio::runtime::Handle::current())
    });
    let deadline = tokio::time::Instant::now() + FIXTURE_WAIT;
    while fixture
        .deps
        .operations
        .lock()
        .unwrap()
        .recovery_controls_in_flight
        < 2
    {
        fixture_tick(deadline).await.unwrap();
    }
    let early = tokio::time::timeout(Duration::from_millis(30), &mut stop).await;
    let returned_early = early.is_ok();
    let duplicate_returned_early = duplicate.is_finished();
    release.send(()).unwrap();
    let result = match early {
        Ok(result) => result.unwrap(),
        Err(_) => tokio::time::timeout(FIXTURE_WAIT, stop)
            .await
            .unwrap()
            .unwrap(),
    };
    let duplicate_result = tokio::time::timeout(FIXTURE_WAIT, duplicate)
        .await
        .unwrap()
        .unwrap();
    let reusable = !fixture
        .deps
        .operations
        .lock()
        .unwrap()
        .cancellation
        .is_cancelled();
    fixture.stop().await;
    eprintln!("initial_tls stop_returned_before_cleanup={returned_early}");
    assert!(
        !returned_early && !duplicate_returned_early,
        "Stop returned before actual native thread join"
    );
    assert_eq!(result, Ok(()));
    assert_eq!(duplicate_result, Ok(()));
    assert!(reusable);
    assert_eq!(
        fs::read(fixture.root.path().join("native-tls-finished")).unwrap(),
        b"joined cleanup"
    );
}

#[tokio::test]
async fn recorded_retry_frame_stays_active_until_actual_resume_returns() {
    let fixture = Fixture::new(true, false, 0);
    assert_eq!(
        post_json(&fixture.deps, "/app/backup/restore", Some(restore_body()))
            .await
            .0,
        200
    );
    assert_eq!(
        wait_terminal(&fixture.deps).await["operation"]["phase"],
        "error"
    );
    fixture.stop().await;
    let (entered, observing) = tokio::sync::oneshot::channel();
    let (release, released) = mpsc::channel();
    *fixture.resume_hold.lock().unwrap() = Some(ResumeHold {
        entered,
        release: released,
    });
    let mut retry = crate::BackupWebDeps::with_root(fixture.retained.clone()).unwrap();
    retry.runner = fixture.runner.clone();
    retry.executable = fixture.package.path().join("bin/solstone");
    retry.journal_maintenance = Arc::new(OkMaintenance);
    let work = retry.clone();
    let root = retry.retained_root().unwrap();
    let body = serde_json::to_vec(&restore_body()).unwrap();
    let retry_task = tokio::task::spawn_blocking(move || {
        crate::recover_recorded_journal_with(&work, &body, &mut || {
            crate::EmbeddedRestoreLifecycle::drain(&Lifecycle, &root)
        })
    });
    tokio::time::timeout(FIXTURE_WAIT, observing)
        .await
        .unwrap()
        .unwrap();
    let active =
        crate::recorded_journal_recovery_active(&retry, &tokio::runtime::Handle::current())
            .unwrap();
    let stopping = retry.clone();
    let mut stop = tokio::task::spawn_blocking(move || {
        crate::stop_recorded_journal(&stopping, &tokio::runtime::Handle::current())
    });
    let early = tokio::time::timeout(Duration::from_millis(30), &mut stop).await;
    let returned_early = early.is_ok();
    release.send(()).unwrap();
    let stop_result = match early {
        Ok(result) => result.unwrap(),
        Err(_) => tokio::time::timeout(FIXTURE_WAIT, stop)
            .await
            .unwrap()
            .unwrap(),
    };
    let result = tokio::time::timeout(FIXTURE_WAIT, retry_task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        active && !returned_early,
        "retry frame finished before its resume owner"
    );
    assert_eq!(stop_result, Ok(()));
    assert!(
        result.is_err(),
        "Stop must remain visible after the joined resume"
    );
    assert_eq!(
        fs::read(fixture.root.path().join("resumed-read")).unwrap(),
        b"derived bytes"
    );
}
