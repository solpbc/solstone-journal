// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
#[cfg(not(windows))]
use std::thread;
use std::{cell::RefCell, fs, path::PathBuf, time::Instant};

use serde::Deserialize;
use solstone_core_backup::archive_admission::Clock;
use solstone_core_journal_io::activity::ActivityLease;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use solstone_core_journal_io::activity::{ActivityAdmissionError, TeardownLease};
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};
use solstone_core_journal_io::paths::SegmentLayout;
use solstone_core_retention::archive_catalog::{RestoreDisposition, codec, read_complete_catalog};
use solstone_core_retention::archive_proof::{
    AcceptedProcessingObservation, ArchiveFileProof, OffloadProof,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::archive_operation::ArchiveBeginRefusal;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::archive_worker::begin_owned;
#[cfg(not(windows))]
use crate::archive_worker::prepare_read;

#[derive(Default)]
pub(super) struct DispatchControl {
    pause: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    resolver_panic: bool,
}
thread_local! {
    static DISPATCH: RefCell<DispatchControl> = RefCell::new(DispatchControl::default());
    static RESPONSE_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub(super) fn take_dispatch() -> DispatchControl {
    DISPATCH.with(|control| std::mem::take(&mut *control.borrow_mut()))
}
pub(super) fn install_dispatch(control: DispatchControl) {
    DISPATCH.with(|current| *current.borrow_mut() = control);
}
pub(super) fn before_pending() {
    let pause = DISPATCH.with(|control| control.borrow_mut().pause.take());
    if let Some((entered, resume)) = pause {
        entered.send(()).unwrap();
        resume.recv().unwrap();
    }
}
pub(super) fn before_resolver() {
    if DISPATCH.with(|control| control.borrow().resolver_panic) {
        panic!("restore resolver unwind with original input outside");
    }
}
pub(super) fn before_response() {
    if RESPONSE_PANIC.replace(false) {
        panic!("restore response unwind with unused input retained");
    }
}

fn join_pending(slot: &SharedOperationSlot) {
    let worker = slot
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .archive
        .restore
        .worker
        .take();
    if let Some(worker) = worker {
        worker.join().unwrap();
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Cut {
    Complete,
    Stale,
    LateDestination,
    ActorPanic,
    CancelBeforeActor,
    CancelBeforeFinish,
}

thread_local! {
    static CONTROL: RefCell<Option<(Cut, PathBuf)>> = const { RefCell::new(None) };
}

fn control() -> Option<(Cut, PathBuf)> {
    CONTROL.with(|control| control.borrow().clone())
}

pub(super) fn after_actor(native: &OwnedRestoreRunOutcome) {
    if matches!(control(), Some((Cut::ActorPanic, _))) {
        assert_eq!(native.report().files_restored, 1);
        let file = native.files().next().unwrap();
        assert_eq!(file.disposition, RestoreDisposition::Published);
        let owned::FileEvidence::Transaction(facts) = file.evidence else {
            panic!("actual native resources required at unwind cut");
        };
        assert!(facts.retained_media);
        assert!(facts.retirement.as_ref().unwrap().retired);
        assert!(facts.settlement_cleanup.is_none());
        panic!("actor unwind while native resources remain owned");
    }
}

pub(super) fn settle(
    native: OwnedRestoreRunOutcome,
) -> Result<(OwnedSettledDirectRead, SettledRestoreReport), Box<OwnedRestoreFinishHold>> {
    if let Some((Cut::LateDestination, segment)) = control() {
        let (settled, fired) = run_with_media_hook(
            MediaPrimitive::ParentSync,
            move || fs::write(segment.join("tombstone.json"), b"{}"),
            || native.settle_direct(),
        );
        assert!(fired);
        settled
    } else {
        native.settle_direct()
    }
}

fn cancel(slot: &SharedOperationSlot) {
    slot.lock()
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .archive_cancellation
        .as_ref()
        .unwrap()
        .cancel();
}

pub(super) fn before_finish(slot: &SharedOperationSlot) {
    if matches!(control(), Some((Cut::CancelBeforeFinish, _))) {
        cancel(slot);
    }
}

pub(super) fn before_handoff(slot: &SharedOperationSlot) {
    if matches!(control(), Some((Cut::Stale, _))) {
        let mut state = slot.lock().unwrap();
        let current = state.current.as_mut().unwrap();
        current.generation += 1;
        current.view.report = Some(json!({"replacement":true}));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Deserialize)]
struct Input {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    mode: Cut,
    journal: PathBuf,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    restic_path: PathBuf,
    repository_id: String,
    snapshot_id: String,
    archive_path: String,
    bytes: u64,
    sha256: String,
}

struct LiveClock(Instant);
impl Clock for LiveClock {
    fn now_unix(&self) -> i64 {
        10_000 + self.0.elapsed().as_secs() as i64
    }
    fn iso_week(&self) -> u8 {
        1
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn seed(input: &Input, root: &Arc<JournalRoot>) -> PathBuf {
    let proof = OffloadProof {
        journal_id: root.identity().observation_id(),
        manifest_id: "native-finish-manifest".into(),
        target: solstone_core_retention::Target {
            day: "20261009".into(),
            stream: "_default".into(),
            dir: "120000_60_fixture".into(),
        },
        layout: SegmentLayout::Direct,
        repository_id: input.repository_id.clone(),
        snapshot_id: input.snapshot_id.clone(),
        files: vec![ArchiveFileProof {
            witness_id: "native-finish-witness".into(),
            name: "audio.flac".into(),
            bytes: input.bytes,
            sha256: input.sha256.clone(),
            archive_path: input.archive_path.clone(),
            processing: AcceptedProcessingObservation {
                record: None,
                has_analysis_row: true,
            },
        }],
    };
    proof.validate().unwrap();
    let segment = input.journal.join("chronicle/20261009/120000_60_fixture");
    fs::create_dir_all(&segment).unwrap();
    let ledger = input.journal.join("health/offload");
    fs::create_dir_all(&ledger).unwrap();
    // Owning data codec only, for this synthetic snapshot-backed fixture.
    let event = codec::Event::ArchiveV2 {
        schema_version: 2,
        time: 1,
        day: proof.target.day.clone(),
        stream: proof.target.stream.clone(),
        segment: "120000_60".into(),
        segment_dir: proof.target.dir.clone(),
        layout: proof.layout,
        journal_id: proof.journal_id,
        manifest_id: proof.manifest_id,
        repository_id: proof.repository_id,
        snapshot_id: proof.snapshot_id,
        files: proof.files,
    };
    let mut bytes = serde_json::to_vec(&event).unwrap();
    bytes.push(b'\n');
    fs::write(ledger.join("20261009.jsonl"), bytes).unwrap();
    let catalog = read_complete_catalog(root).unwrap();
    assert_eq!(catalog.days.len(), 1);
    segment
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn blocked(slot: &SharedOperationSlot, root: &Arc<JournalRoot>, clock: &super::super::OwnedClock) {
    assert!(operation::is_busy(slot));
    assert!(operation::begin(slot, "restore", None, None, None).is_err());
    assert!(matches!(
        crate::archive_operation::begin(slot, "archive", clock.as_ref()),
        Err(ArchiveBeginRefusal::Busy)
    ));
    assert!(matches!(
        begin_owned(slot, "archive", Arc::clone(root), Arc::clone(clock)),
        Err(ArchiveBeginRefusal::Busy)
    ));
    assert!(matches!(
        ActivityLease::try_acquire(Arc::clone(root)),
        Err(ActivityAdmissionError::Busy)
    ));
    assert!(matches!(
        TeardownLease::try_acquire(Arc::clone(root)),
        Err(ActivityAdmissionError::Busy)
    ));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
#[ignore = "requires a copied current package and synthetic direct restore fixture"]
fn packaged_owned_restore_worker() {
    use sha2::{Digest, Sha256};
    let input: Input = serde_json::from_slice(
        &fs::read(std::env::var_os("OWNED_RESTORE_WORKER_JSON").unwrap()).unwrap(),
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(&input.journal).unwrap());
    let segment = seed(&input, &root);
    let slot = operation::new_slot();
    let clock: super::super::OwnedClock = Arc::new(LiveClock(Instant::now()));
    let begin = begin_owned(
        &slot,
        "offload_restore",
        Arc::clone(&root),
        Arc::clone(&clock),
    )
    .unwrap();
    let generation = begin.generation();
    let worker_slot = Arc::clone(&slot);
    let worker_segment = segment.clone();
    let restic = input.restic_path.clone();
    let repository = input.repository_id.clone();
    let mode = input.mode;
    let worker = thread::spawn(move || {
        let worker = prepare_read(&worker_slot, begin, repository, &restic)
            .unwrap_or_else(|| panic!("actual original worker entrance"));
        let carrier = Arc::downgrade(worker.outside.carrier());
        let lease = Arc::downgrade(worker.outside.carrier().activity());
        CONTROL.with(|control| *control.borrow_mut() = Some((mode, worker_segment)));
        if mode == Cut::CancelBeforeActor {
            cancel(&worker_slot);
        }
        day(&worker_slot, worker, "20261009", 2);
        CONTROL.with(|control| *control.borrow_mut() = None);
        (carrier, lease)
    });
    let (carrier, lease) = worker.join().unwrap();
    let guard = slot.lock().unwrap();
    let current = guard.current.as_ref().unwrap();
    let view = serde_json::to_value(&current.view).unwrap();
    let mut result = json!({"mode":format!("{mode:?}"), "view":view});
    if mode == Cut::Complete {
        assert!(guard.archive.retained.is_empty());
        assert!(guard.archive.reservation.is_none());
        assert_eq!(current.view.phase, "done");
        let report = current.view.report.as_ref().unwrap();
        assert_eq!(report["bytes_restored"], input.bytes);
        assert_eq!(report["runtime"]["state"], "completed");
        assert_eq!(report["runtime"]["retirement"]["effect"], "removed");
        assert_eq!(
            report["files"][0]["evidence"]["settlement_cleanup"]["cleanup"],
            "complete"
        );
        assert!(carrier.upgrade().is_none() && lease.upgrade().is_none());
        drop(guard);
        assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
        assert!(TeardownLease::try_acquire(Arc::clone(&root)).is_ok());
        let next = begin_owned(
            &slot,
            "offload_restore",
            Arc::clone(&root),
            Arc::clone(&clock),
        )
        .unwrap();
        let next = prepare_read(&slot, next, input.repository_id, &input.restic_path)
            .unwrap_or_else(|| panic!("successor original installed entrance"));
        return_owned(
            &slot,
            next.ticket,
            Some(next.custody),
            ArchivePayload::ReadHeld {
                read: Box::new(next.read.into_owned_hold()),
                outside: next.outside,
            },
            Terminal::error("cancelled"),
        );
        result["next_installed_admission"] = json!(true);
    } else {
        assert_eq!(guard.archive.retained.len(), 1);
        let retained = &guard.archive.retained[0];
        assert_eq!(retained.generation(), generation);
        if mode == Cut::Stale {
            assert_eq!(
                retained.reason(),
                Some(super::super::ArchiveHandoffRefusal::Stale)
            );
            assert_eq!(current.view.report, Some(json!({"replacement":true})));
        } else {
            assert!(retained.reason().is_none());
        }
        let report = retained.terminal().report.as_ref();
        if mode != Cut::CancelBeforeActor {
            assert_eq!(report.unwrap()["bytes_restored"], input.bytes);
            assert_eq!(report.unwrap()["files"][0]["disposition"], "published");
        }
        match retained.payload() {
            ArchivePayload::ReadHeld { read, outside } => {
                assert_eq!(mode, Cut::CancelBeforeActor);
                assert!(Arc::ptr_eq(read.carrier(), outside.carrier()));
                assert!(Arc::ptr_eq(outside.journal_root(), &root));
                assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
                assert_eq!(
                    retained.terminal().reason_code.as_deref(),
                    Some("cancelled")
                );
                assert!(!segment.join("audio.flac").exists());
                assert!(report.is_none());
            }
            ArchivePayload::NativeHeld { native, outside } => {
                assert_eq!(mode, Cut::LateDestination);
                assert!(matches!(
                    native.refusal(),
                    RestoreFinishRefusal::Native {
                        reason: "settlement_destination_changed",
                        ..
                    }
                ));
                assert!(Arc::ptr_eq(native.carrier(), outside.carrier()));
                assert_eq!(native.report().bytes_restored, input.bytes);
                assert_eq!(report.unwrap()["settlement"]["state"], "held");
                assert_eq!(
                    report.unwrap()["files"][0]["evidence"]["settlement_cleanup"]["cleanup"],
                    "complete"
                );
            }
            ArchivePayload::NativeFinished {
                report: native,
                runtime,
            } => {
                assert_eq!(native.report().bytes_restored, input.bytes);
                assert!(Arc::ptr_eq(runtime.journal_root(), &root));
                assert!(Arc::ptr_eq(runtime.owned_clock(), &clock));
                assert_eq!(
                    runtime.state(),
                    if mode == Cut::Stale {
                        DirectReadFinishState::Completed
                    } else {
                        DirectReadFinishState::Held
                    }
                );
                assert_eq!(
                    report.unwrap()["files"][0]["evidence"]["settlement_cleanup"]["cleanup"],
                    "complete"
                );
                if mode == Cut::Stale {
                    let retirement = runtime.retirement().unwrap();
                    assert!(std::ptr::eq(
                        Arc::as_ptr(retirement.owned.activity()),
                        lease.as_ptr()
                    ));
                } else {
                    assert!(runtime.reason().is_some());
                }
            }
            ArchivePayload::Unknown {
                runtime,
                report: native,
            } => {
                assert_eq!(mode, Cut::ActorPanic);
                assert_eq!(runtime.state(), DirectReadFinishState::Unknown);
                assert!(runtime.retirement().is_none() && native.is_none());
                assert!(Arc::ptr_eq(runtime.journal_root(), &root));
                assert!(Arc::ptr_eq(runtime.owned_clock(), &clock));
                assert_eq!(report.unwrap()["settlement"]["state"], "unknown");
                assert!(report.unwrap()["files"][0]["evidence"]["settlement_cleanup"].is_null());
            }
            _ => panic!("actual whole worker payload required"),
        }
        let terminal = retained.terminal();
        result["terminal"] = json!({
            "phase":terminal.phase, "reason_code":terminal.reason_code, "detail":terminal.detail,
            "guidance":terminal.guidance, "recording_failure":terminal.recording_failure,
            "report":terminal.report,
        });
        result["held"] = json!(true);
        drop(guard);
        blocked(&slot, &root, &clock);
        assert!(lease.upgrade().is_some());
        // Genuine retirement moves the carrier out of its Arc into the owning
        // receipt. Its activity lease remains retained by rejected completion.
        if mode != Cut::Stale {
            assert!(carrier.upgrade().is_some());
        }
    }
    if mode != Cut::CancelBeforeActor {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(segment.join("audio.flac")).unwrap())
            ),
            input.sha256
        );
    }
    println!("OWNED_RESTORE_WORKER_RESULT {}", result);
}

struct AdjustableClock(std::sync::atomic::AtomicI64);
impl Clock for AdjustableClock {
    fn now_unix(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn iso_week(&self) -> u8 {
        1
    }
}

fn local_restore() -> (
    tempfile::TempDir,
    crate::BackupWebDeps,
    Arc<AdjustableClock>,
    PathBuf,
) {
    let journal = crate::test_support::root("healthy");
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let input = Input {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        mode: Cut::Complete,
        journal: journal.path().to_path_buf(),
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        restic_path: PathBuf::new(),
        repository_id: "a".repeat(64),
        snapshot_id: "b".repeat(64),
        archive_path: "/synthetic/audio.flac".into(),
        bytes: 7,
        sha256: "c".repeat(64),
    };
    let segment = seed(&input, &root);
    let clock = Arc::new(AdjustableClock(std::sync::atomic::AtomicI64::new(10_000)));
    let mut deps = crate::BackupWebDeps::new(journal.path().to_path_buf());
    deps.clock = clock.clone();
    deps.executable = journal.path().join("missing-package/bin/solstone");
    (journal, deps, clock, segment)
}

#[test]
fn stopped_lifetime_releases_actual_unused_restore_without_status_response() {
    let (journal, deps, _, _) = local_restore();
    let before = fs::read(journal.path().join("config/journal.json")).unwrap();
    start(&deps, None).unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if deps
            .operations
            .lock()
            .unwrap()
            .archive
            .restore
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual restore worker must return"
        );
        std::thread::park_timeout(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        deps.operations.lock().unwrap().archive.restore.unused.len(),
        1
    );
    let lifetime = deps.lifetime();
    lifetime.request_stop();
    assert_eq!(
        lifetime.drain(
            crate::test_support::join_executor(),
            Instant::now() + std::time::Duration::from_secs(5)
        ),
        crate::BackupRouteDrain::Drained
    );
    let state = deps.operations.lock().unwrap();
    assert!(state.archive.restore.worker.is_none());
    assert!(state.join_batches.is_empty());
    assert!(!deps.operations.joining());
    assert!(!state.archive.is_busy());
    assert_eq!(
        state.current.as_ref().unwrap().view.reason_code.as_deref(),
        Some("restic_unavailable")
    );
    assert_eq!(
        fs::read(journal.path().join("config/journal.json")).unwrap(),
        before
    );
}

#[test]
fn public_restore_preselection_uses_complete_unique_bound_catalog() {
    let (journal, deps, _, _) = local_restore();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    assert_eq!(
        repository(&root, Some("20261009")),
        Ok(Some("a".repeat(64)))
    );
    assert_eq!(repository(&root, None), Ok(Some("a".repeat(64))));
    assert_eq!(
        repository(&root, Some("20261008")),
        Err("nothing_to_restore")
    );
    let ledger = journal.path().join("health/offload/20261009.jsonl");
    let original = fs::read(&ledger).unwrap();
    let mut extra: Value = serde_json::from_slice(&original).unwrap();
    extra["repository_id"] = json!("d".repeat(64));
    extra["manifest_id"] = json!("other-manifest");
    extra["files"][0]["witness_id"] = json!("other-witness");
    extra["segment_dir"] = json!("130000_60_fixture");
    extra["segment"] = json!("130000_60");
    let mut mixed = original.clone();
    mixed.extend(serde_json::to_vec(&extra).unwrap());
    mixed.push(b'\n');
    fs::write(&ledger, mixed).unwrap();
    assert_eq!(repository(&root, None), Err("repository_mismatch"));
    assert!(start(&deps, None).is_err());
    assert!(!operation::is_busy(&deps.operations));
    assert!(!journal.path().join("backup-activity.lock").exists());
    fs::write(&ledger, b"not-json\n").unwrap();
    assert_eq!(repository(&root, None), Err("ledger_degraded"));
    assert!(start(&deps, None).is_err());
    assert!(!operation::is_busy(&deps.operations));
}

fn historical(journal: &std::path::Path, dir: &str) {
    fs::create_dir_all(journal.join("chronicle/20261009").join(dir)).unwrap();
    solstone_core_offload::append_offload_event(
        journal,
        "20261009",
        "_default",
        dir,
        "legacy-snapshot",
        &[solstone_core_retention::archive_catalog::OffloadFile {
            name: "raw.webm".into(),
            bytes: 8,
            sha256: "e".repeat(64),
        }],
        1,
    )
    .unwrap();
}

#[test]
fn historical_files_without_a_repository_never_refuse_the_whole_request() {
    // Mixed: bound files name the repository; historical files ride the same read.
    let (journal, deps, _, _) = local_restore();
    historical(journal.path(), "130000_60_fixture");
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    assert_eq!(repository(&root, None), Ok(Some("a".repeat(64))));
    assert_eq!(
        repository(&root, Some("20261009")),
        Ok(Some("a".repeat(64)))
    );
    // Historical only: the configured repository is observed at admission.
    let only = crate::test_support::root("healthy");
    historical(only.path(), "120000_60_fixture");
    let only_root = Arc::new(JournalRoot::open(only.path()).unwrap());
    assert_eq!(repository(&only_root, None), Ok(None));
    assert_eq!(repository(&only_root, Some("20261009")), Ok(None));
    assert_eq!(
        repository(&only_root, Some("20261008")),
        Err("nothing_to_restore")
    );
    let mut only_deps = crate::BackupWebDeps::new(only.path().to_path_buf());
    only_deps.clock = deps.clock.clone();
    only_deps.executable = only.path().join("missing-package/bin/solstone");
    for (deps, journal) in [(&deps, journal.path()), (&only_deps, only.path())] {
        let ledger = fs::read(journal.join("health/offload/20261009.jsonl")).unwrap();
        let generation = start(deps, None).unwrap();
        join_pending(&deps.operations);
        assert!(
            response(deps, Some(generation))
                .unwrap()
                .status()
                .is_success()
        );
        let state = deps.operations.lock().unwrap();
        let view = &state.current.as_ref().unwrap().view;
        assert_eq!(view.phase, "refused");
        assert_eq!(view.reason_code.as_deref(), Some("restic_unavailable"));
        assert_eq!(view.report.as_ref().unwrap()["scope"], "all");
        drop(state);
        assert!(!operation::is_busy(&deps.operations));
        assert_eq!(
            fs::read(journal.join("health/offload/20261009.jsonl")).unwrap(),
            ledger
        );
        assert!(
            !journal
                .join("chronicle/20261009/120000_60_fixture/raw.webm")
                .exists()
        );
        assert!(
            !journal
                .join("chronicle/20261009/130000_60_fixture/raw.webm")
                .exists()
        );
    }
}

#[test]
fn pending_restore_preserves_original_input_and_matching_response_authority() {
    for cut in [
        "spawn", "tools", "cancel", "deadline", "resolver", "start", "stale", "absent", "poison",
        "response",
    ] {
        let (journal, deps, clock, _) = local_restore();
        let config = fs::read(journal.path().join("config/journal.json")).unwrap();
        let ledger = fs::read(journal.path().join("health/offload/20261009.jsonl")).unwrap();
        let (entered, waiting) = std::sync::mpsc::channel();
        let (resume, continued) = std::sync::mpsc::channel();
        DISPATCH.with(|control| {
            *control.borrow_mut() = DispatchControl {
                pause: (cut != "spawn").then_some((entered, continued)),
                resolver_panic: cut == "resolver",
            }
        });
        if cut == "spawn" {
            operation::fail_next_worker_start();
        }
        let generation = start(&deps, None).unwrap();
        if cut != "spawn" {
            waiting.recv().unwrap();
            {
                let mut state = deps.operations.lock().unwrap();
                let input = state.archive.restore.input.as_ref().unwrap();
                assert_eq!(input.repository, Some("a".repeat(64)));
                assert!(input.day.is_none());
                assert!(Arc::ptr_eq(&input.begin.ticket.0.clock, &deps.clock));
                assert_eq!(input.begin.generation(), generation);
                assert_eq!(
                    input.begin.ticket.0.root.identity().observation_id(),
                    JournalRoot::open(journal.path())
                        .unwrap()
                        .identity()
                        .observation_id()
                );
                match cut {
                    "cancel" => state
                        .current
                        .as_ref()
                        .unwrap()
                        .archive_cancellation
                        .as_ref()
                        .unwrap()
                        .cancel(),
                    "deadline" => clock.0.store(40_000, std::sync::atomic::Ordering::SeqCst),
                    "start" => state.current.as_mut().unwrap().worker_active = true,
                    "stale" => {
                        let current = state.current.as_mut().unwrap();
                        current.generation += 1;
                        current.view.report = Some(json!({"successor":true}));
                    }
                    "absent" => state.current = None,
                    _ => {}
                }
            }
            if cut == "poison" {
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    let _guard = deps.operations.lock().unwrap();
                    panic!("poison admission");
                }));
            }
            resume.send(()).unwrap();
        }
        join_pending(&deps.operations);
        assert!(operation::is_busy(&deps.operations));
        if cut == "response" {
            RESPONSE_PANIC.set(true);
        }
        let response = response(&deps, Some(generation)).unwrap();
        let refused_return = matches!(cut, "start" | "stale" | "absent" | "poison" | "response");
        assert_eq!(response.status().is_success(), !refused_return, "{cut}");
        assert_eq!(
            operation::is_busy(&deps.operations),
            refused_return,
            "{cut}"
        );
        let state = deps
            .operations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            state.archive.restore.unused.len(),
            usize::from(refused_return)
        );
        if cut == "stale" {
            assert_eq!(
                state
                    .current
                    .as_ref()
                    .unwrap()
                    .view
                    .report
                    .as_ref()
                    .unwrap()["successor"],
                true
            );
        }
        if !refused_return {
            assert_eq!(
                state.current.as_ref().unwrap().view.reason_code.as_deref(),
                Some(match cut {
                    "spawn" => "worker_start_failed",
                    "cancel" => "cancelled",
                    "deadline" => "deadline_exceeded",
                    "resolver" => "resolver_unknown",
                    _ => "restic_unavailable",
                })
            );
        }
        drop(state);
        assert_eq!(
            fs::read(journal.path().join("config/journal.json")).unwrap(),
            config
        );
        assert_eq!(
            fs::read(journal.path().join("health/offload/20261009.jsonl")).unwrap(),
            ledger
        );
        assert!(!journal.path().join("backup-activity.lock").exists());
    }
}

#[tokio::test]
#[ignore = "requires the existing copied package and snapshot fixture; public caller only"]
async fn packaged_public_bound_restore() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let input: Input = serde_json::from_slice(
        &fs::read(std::env::var_os("OWNED_RESTORE_WORKER_JSON").unwrap()).unwrap(),
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(&input.journal).unwrap());
    let segment = seed(&input, &root);
    let mut deps = crate::BackupWebDeps::new(input.journal.clone());
    deps.executable = std::env::current_exe().unwrap();
    deps.clock = Arc::new(LiveClock(Instant::now()));
    let router = crate::routes_with_deps(deps.clone());
    let started = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/backup/offload/restore")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"day":"20261009"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(started.status().is_success());
    join_pending(&deps.operations);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/app/backup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success());
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1_048_576).await.unwrap()).unwrap();
    assert_eq!(value["operation"]["phase"], "done");
    assert_eq!(value["operation"]["report"]["bytes_restored"], input.bytes);
    assert_eq!(
        value["operation"]["report"]["runtime"]["state"],
        "completed"
    );
    assert_eq!(
        fs::read(segment.join("audio.flac")).unwrap(),
        fs::read(input.archive_path).unwrap()
    );
    assert!(!operation::is_busy(&deps.operations));
    assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
    assert!(operation::begin(&deps.operations, "backup", None, None, None).is_ok());
    println!("PUBLIC_BOUND_RESTORE {}", value);
}
