// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A recovery frame belongs to the retained backup route lifetime.

use solstone_core_backup::archive_admission::ArchiveCancellation;

use crate::operation::{self, SharedOperationSlot, Terminal, WriteGuard};

pub(crate) struct RecoveryFrame {
    slot: SharedOperationSlot,
    generation: u64,
    cancellation: ArchiveCancellation,
    _write: WriteGuard,
    finished: bool,
}

impl RecoveryFrame {
    pub(crate) fn begin(slot: &SharedOperationSlot) -> Result<Self, String> {
        let (generation, cancellation) = {
            let mut state = operation::lock_for_begin(slot)
                .map_err(|_| "backup operation is busy".to_owned())?;
            let cancellation = ArchiveCancellation::new_child(&state.cancellation);
            let begun = operation::install_slot(
                &mut state.current,
                "restore_recovery",
                None,
                None,
                None,
                Some(cancellation.clone()),
            );
            (begun.generation, cancellation)
        };
        let write = operation::admit_write(slot, operation::AdmitWrite::DuringOperation)
            .map_err(|_| "backup owner has stopped".to_owned())?;
        if !operation::start_worker(slot, generation) {
            return Err("backup owner has stopped".into());
        }
        Ok(Self {
            slot: slot.clone(),
            generation,
            cancellation,
            _write: write,
            finished: false,
        })
    }

    pub(crate) fn cancellation(&self) -> ArchiveCancellation {
        self.cancellation.clone()
    }

    pub(crate) fn finish(mut self, terminal: Terminal) {
        self.finish_inner(terminal);
    }

    fn finish_inner(&mut self, terminal: Terminal) {
        if self.finished {
            return;
        }
        self.finished = true;
        #[cfg(windows)]
        operation::finish_worker(&self.slot, self.generation, terminal, Vec::new());
        #[cfg(not(windows))]
        operation::finish_worker(&self.slot, self.generation, terminal);
    }
}

impl Drop for RecoveryFrame {
    fn drop(&mut self) {
        self.finish_inner(Terminal::error("restore_failed"));
    }
}

pub(crate) fn stop(slot: &SharedOperationSlot) -> Result<(), String> {
    let generation = {
        let state = slot
            .lock()
            .map_err(|_| "backup operation is unavailable".to_owned())?;
        let Some(current) = state.current.as_ref() else {
            return Ok(());
        };
        if !owns_recovery(&current.view.kind) {
            return Ok(());
        }
        current.generation
    };
    if operation::request_archive_cancellation(slot, generation)
        == crate::archive_operation::ArchiveCancellationRequest::Refused
    {
        let state = slot.lock().map_err(|_| "backup operation is unavailable")?;
        if state.current.as_ref().is_some_and(|current| {
            current.generation == generation
                && !operation::is_terminal(&current.view.phase)
                && !current
                    .archive_cancellation
                    .as_ref()
                    .is_some_and(|stop| stop.is_cancelled())
        }) {
            return Err("restore controls are unavailable".into());
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let active = {
            let state = slot
                .lock()
                .map_err(|_| "backup operation is unavailable".to_owned())?;
            state
                .current
                .as_ref()
                .is_some_and(|current| current.generation == generation && current.worker_active)
        };
        if !active {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("restore is still stopping".into());
        }
        slot.wait_timeout(remaining.min(std::time::Duration::from_millis(50)));
    }
}

pub(crate) fn active(slot: &SharedOperationSlot) -> Result<bool, String> {
    let state = slot
        .lock()
        .map_err(|_| "backup operation is unavailable".to_owned())?;
    Ok(state.current.as_ref().is_some_and(|current| {
        owns_recovery(&current.view.kind)
            && (current.worker_active || !operation::is_terminal(&current.view.phase))
    }))
}

pub(crate) fn owns_recovery(kind: &str) -> bool {
    matches!(kind, "restore" | "restore_hosted" | "restore_recovery")
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[cfg(not(windows))]
    #[test]
    fn accepted_recovery_cannot_start_its_native_writer_after_stop_returns() {
        let mut observed = Vec::new();
        for kind in ["restore", "restore_hosted", "restore_recovery"] {
            let journal = tempfile::tempdir().unwrap();
            let deps = crate::BackupWebDeps::new(journal.path().to_owned());
            let generation = operation::begin(&deps.operations, kind, None, None, None)
                .unwrap()
                .generation;
            let active = crate::recorded_journal_recovery_active(
                &deps,
                crate::test_support::join_executor(),
            )
            .unwrap();
            let stopped = crate::stop_recorded_journal(&deps, crate::test_support::join_executor());
            let path = journal.path().join("late-accepted-restore");
            let work_path = path.clone();
            let (done, received) = mpsc::channel();
            let accepted =
                operation::spawn_worker(deps.operations.clone(), generation, move || {
                    std::fs::write(work_path, b"native work after Stop").unwrap();
                    done.send(()).unwrap();
                    Terminal::done()
                });
            if accepted {
                received.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            let lifetime = deps.lifetime();
            lifetime.request_stop();
            assert_eq!(
                lifetime.drain(
                    crate::test_support::join_executor(),
                    Instant::now() + Duration::from_secs(5)
                ),
                crate::BackupRouteDrain::Drained
            );
            observed.push((kind, active, stopped, accepted, path.exists()));
        }
        eprintln!("accepted-before-start controls={observed:?}");
        assert!(
            observed
                .iter()
                .all(|(_, active, stopped, accepted, copied)| *active
                    && stopped.is_ok()
                    && !accepted
                    && !copied),
            "accepted recovery generation escaped Stop: {observed:?}"
        );
    }

    #[test]
    fn cancel_waits_for_the_real_writer_and_preserves_the_retained_owner_for_retry() {
        let journal = tempfile::tempdir().unwrap();
        let deps = crate::BackupWebDeps::new(journal.path().to_owned());
        let frame = RecoveryFrame::begin(&deps.operations).unwrap();
        let cancellation = frame.cancellation();
        let (cancelled, observed) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let path = journal.path().to_owned();
        let writer = std::thread::spawn(move || {
            std::fs::write(path.join("accepted"), b"native frame admitted").unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !cancellation.is_cancelled() {
                assert!(Instant::now() < deadline);
                std::thread::park_timeout(Duration::from_millis(1));
            }
            cancelled.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            std::fs::write(path.join("drained"), b"actual writer returned").unwrap();
            frame.finish(Terminal::error("cancelled"));
        });
        let slot = deps.operations.clone();
        let stopping = std::thread::spawn(move || stop(&slot));
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!stopping.is_finished());
        assert!(active(&deps.operations).unwrap());
        assert!(RecoveryFrame::begin(&deps.operations).is_err());
        assert!(!journal.path().join("drained").exists());
        assert!(!deps.operations.lock().unwrap().cancellation.is_cancelled());
        release.send(()).unwrap();
        writer.join().unwrap();
        stopping.join().unwrap().unwrap();
        assert_eq!(
            std::fs::read(journal.path().join("drained")).unwrap(),
            b"actual writer returned"
        );
        let retry = RecoveryFrame::begin(&deps.operations).unwrap();
        assert!(!retry.cancellation().is_cancelled());
        retry.finish(Terminal::done());
    }

    #[test]
    fn app_stop_cancels_and_drains_an_admitted_recovery_frame() {
        let journal = tempfile::tempdir().unwrap();
        let deps = crate::BackupWebDeps::new(journal.path().to_owned());
        let frame = RecoveryFrame::begin(&deps.operations).unwrap();
        let cancellation = frame.cancellation();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let lifetime = deps.lifetime();
        lifetime.request_stop();
        assert!(cancellation.is_cancelled());
        assert_eq!(
            lifetime.drain(runtime.handle(), Instant::now() + Duration::from_millis(30)),
            crate::BackupRouteDrain::Pending
        );
        assert!(RecoveryFrame::begin(&deps.operations).is_err());
        frame.finish(Terminal::error("cancelled"));
        assert_eq!(
            lifetime.drain(runtime.handle(), Instant::now() + Duration::from_secs(1)),
            crate::BackupRouteDrain::Drained
        );
    }

    #[test]
    fn changed_retry_selection_is_refused_before_tools_or_drain() {
        use solstone_core_journal_io::whole_journal_restore::{
            WholeJournalRestoreSource, publish_whole_journal_restore,
        };
        let journal = tempfile::tempdir().unwrap();
        let snapshot = "a".repeat(64);
        publish_whole_journal_restore(
            journal.path(),
            &WholeJournalRestoreSource::Byo {
                repository: "s3:synthetic/original".into(),
                backend: "s3".into(),
            },
            &snapshot,
            "/journal",
        )
        .unwrap();
        let record = journal.path().join("health/whole-journal-restore.json");
        let original = std::fs::read(&record).unwrap();
        let deps = crate::BackupWebDeps::new(journal.path().to_owned());
        for changed in [
            serde_json::json!({"recovery_key":"synthetic", "snapshot_id":"b".repeat(64)}),
            serde_json::json!({"recovery_key":"synthetic", "subtree":"/other"}),
            serde_json::json!({"recovery_key":"synthetic", "repository":"s3:synthetic/other", "backend":"s3"}),
        ] {
            let mut drained = false;
            let result = crate::recover_recorded_journal_with(
                &deps,
                &serde_json::to_vec(&changed).unwrap(),
                &mut || {
                    drained = true;
                    Err("unexpected drain".into())
                },
            );
            assert_eq!(result, Err("restore_source_mismatch".into()));
            assert!(!drained);
            assert!(
                operation::current(&deps.operations).is_none(),
                "refusal precedes tool/worker admission"
            );
            assert_eq!(std::fs::read(&record).unwrap(), original);
            assert!(!journal.path().join("config").exists());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recorded_retry_retains_the_actual_runner_frame_through_cancel_and_reuses_its_owner() {
        use solstone_core_backup_runtime::{ToolOutput, ToolRequest, ToolRunner};
        use solstone_core_journal_io::whole_journal_restore::{
            TargetRestoreHold, WholeJournalRestoreReadiness, WholeJournalRestoreSource,
            publish_whole_journal_restore, read_whole_journal_restore,
        };
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        struct Runner {
            calls: AtomicUsize,
            entered: Mutex<Option<mpsc::Sender<()>>>,
            cancelled: Mutex<Option<mpsc::Sender<()>>>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl Runner {
            fn execute(
                &self,
                request: &ToolRequest<'_>,
                cancellation: Option<&ArchiveCancellation>,
            ) -> std::io::Result<ToolOutput> {
                let args: Vec<_> = request
                    .argv
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                let output = |code, text: String| ToolOutput {
                    returncode: code,
                    stdout: text.into_bytes(),
                    stderr: Vec::new(),
                };
                match args.first().map(String::as_str) {
                    Some("version") => Ok(output(0, "restic 0.19.0\n".into())),
                    Some("snapshots") => Ok(output(
                        0,
                        serde_json::json!([{
                            "id":"a".repeat(64), "time":"2026-10-26T00:00:00Z", "paths":["/journal"]
                        }])
                        .to_string(),
                    )),
                    Some("restore") => {
                        let index = args.iter().position(|arg| arg == "--target").unwrap();
                        let target = std::path::Path::new(&args[index + 1]);
                        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
                        std::fs::write(
                            target.join("restored-through-retained-frame"),
                            if first {
                                b"partial".as_slice()
                            } else {
                                b"restored".as_slice()
                            },
                        )?;
                        if first {
                            let cancellation = cancellation.ok_or_else(|| {
                                std::io::Error::other("lost original cancellation")
                            })?;
                            self.entered
                                .lock()
                                .unwrap()
                                .take()
                                .unwrap()
                                .send(())
                                .unwrap();
                            let deadline = Instant::now() + Duration::from_secs(5);
                            while !cancellation.is_cancelled() {
                                if Instant::now() >= deadline {
                                    return Err(std::io::Error::other(
                                        "cancel did not reach runner",
                                    ));
                                }
                                std::thread::park_timeout(Duration::from_millis(1));
                            }
                            self.cancelled
                                .lock()
                                .unwrap()
                                .take()
                                .unwrap()
                                .send(())
                                .unwrap();
                            self.release
                                .lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(5))
                                .map_err(std::io::Error::other)?;
                            return Ok(output(130, String::new()));
                        }
                        Ok(output(0, r#"[{"message_type":"summary","total_files":1,"files_restored":1,"total_bytes":8,"bytes_restored":8}]"#.into()))
                    }
                    Some("check") => Ok(output(0, "{}".into())),
                    _ => Err(std::io::Error::other(format!(
                        "unexpected controlled request: {args:?}"
                    ))),
                }
            }
        }
        impl ToolRunner for Runner {
            fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
                self.execute(request, None)
            }
            fn run_cancellable(
                &self,
                request: &ToolRequest<'_>,
                cancellation: &ArchiveCancellation,
            ) -> std::io::Result<ToolOutput> {
                self.execute(request, Some(cancellation))
            }
        }
        struct Maintenance;
        impl solstone_core_backup_runtime::JournalMaintenance for Maintenance {
            fn rebuild_body_history(
                &self,
                root: &std::path::Path,
            ) -> Result<(), solstone_core_backup_runtime::JournalMaintenanceError> {
                std::fs::write(root.join("rebuild-finished"), b"rebuild")
                    .map_err(|_| solstone_core_backup_runtime::JournalMaintenanceError)
            }
            fn full_scan(
                &self,
                root: &std::path::Path,
            ) -> Result<(), solstone_core_backup_runtime::JournalMaintenanceError> {
                std::fs::write(root.join("scan-enqueued"), b"scan")
                    .map_err(|_| solstone_core_backup_runtime::JournalMaintenanceError)
            }
        }
        let journal = tempfile::tempdir().unwrap();
        let package = tempfile::tempdir().unwrap();
        crate::test_support::write_ready_restic(package.path());
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        let repository = "s3:https://synthetic.invalid/bucket/prefix";
        publish_whole_journal_restore(
            journal.path(),
            &WholeJournalRestoreSource::Byo {
                repository: repository.into(),
                backend: "s3".into(),
            },
            &"a".repeat(64),
            "/journal",
        )
        .unwrap();
        let (entered, observing) = mpsc::channel();
        let (cancelled, observed_cancel) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let runner = Arc::new(Runner {
            calls: AtomicUsize::new(0),
            entered: Mutex::new(Some(entered)),
            cancelled: Mutex::new(Some(cancelled)),
            release: Mutex::new(released),
        });
        let mut deps = crate::BackupWebDeps::new(journal.path().to_owned());
        deps.executable = package.path().join("bin/solstone");
        deps.runner = runner.clone();
        deps.journal_maintenance = Arc::new(Maintenance);
        let body = serde_json::to_vec(&serde_json::json!({
            "recovery_key":stored.recovery_key, "repository":repository, "backend":"s3",
            "credentials":{"access_key_id":"synthetic","secret_access_key":"synthetic"}
        }))
        .unwrap();
        struct Paused(TargetRestoreHold);
        impl crate::PausedRestore for Paused {
            fn resume(self: Box<Self>, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
                self.0.revalidate().map_err(|e| e.to_string())?;
                if cancelled() {
                    return Err("stopped".into());
                }
                Ok(())
            }
        }
        let drain = || -> Result<Box<dyn crate::PausedRestore>, String> {
            let hold = TargetRestoreHold::acquire(deps.retained_root().unwrap())
                .map_err(|e| e.to_string())?;
            hold.wait_for_admitted_use().map_err(|e| e.to_string())?;
            Ok(Box::new(Paused(hold)))
        };
        std::thread::scope(|scope| {
            let retry =
                scope.spawn(|| crate::recover_recorded_journal_with(&deps, &body, &mut || drain()));
            observing.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(
                std::fs::read(journal.path().join("restored-through-retained-frame")).unwrap(),
                b"partial"
            );
            let stop = scope.spawn(|| {
                crate::stop_recorded_journal(&deps, crate::test_support::join_executor())
            });
            observed_cancel
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert!(
                !stop.is_finished(),
                "stop awaits the actual foreign writer frame"
            );
            assert!(RecoveryFrame::begin(&deps.operations).is_err());
            assert!(active(&deps.operations).unwrap());
            release.send(()).unwrap();
            assert!(retry.join().unwrap().is_err());
            stop.join().unwrap().unwrap();
        });
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));
        assert!(!deps.operations.lock().unwrap().cancellation.is_cancelled());
        crate::recover_recorded_journal_with(&deps, &body, &mut || drain()).unwrap();
        assert_eq!(
            std::fs::read(journal.path().join("restored-through-retained-frame")).unwrap(),
            b"restored"
        );
        assert!(journal.path().join("rebuild-finished").exists());
        assert!(journal.path().join("scan-enqueued").exists());
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
        assert!(
            solstone_core_journal_io::whole_journal_restore::begin_target_use(journal.path())
                .is_ok()
        );
    }
}
