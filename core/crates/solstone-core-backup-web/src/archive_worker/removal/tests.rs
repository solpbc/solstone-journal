// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::{cell::Cell, fs, path::PathBuf, sync::Arc, thread, time::Instant};

use chrono::TimeZone;
use serde::Deserialize;
use solstone_core_backup::archive_admission::Clock;
use solstone_core_backup_runtime::DirectReadFinishState;
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity::{ActivityAdmissionError, ActivityLease, TeardownLease};
use solstone_core_journal_io::media::{MediaDirectory, RemovalEffect};
use solstone_core_journal_io::paths::SegmentLayout;
use solstone_core_retention::archive_catalog::{codec, read_complete_catalog};
use solstone_core_retention::archive_proof::{
    AcceptedProcessingObservation, ArchiveFileProof, OffloadOffer, OffloadProof,
};
use solstone_core_retention::content::{ClosedHandlerSet, JournalMedia};
use solstone_core_retention::marks::{Mark, MarkId, MarkState, Proposal, Register, RemovalClass};
use solstone_core_retention::remove_marked::{
    OwnedReviewedArchiveRun, ReviewedArchiveFinishRefusal,
};
use solstone_core_retention::review_context::{
    check_review_context, load_at_root, read_review_context,
};

use crate::archive_operation::ArchiveBeginRefusal;
use crate::archive_worker::{begin_owned, prepare_read};
use crate::operation;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Cut {
    Complete,
    Stale,
    Unproven,
    ActorPanic,
    CancelBeforeActor,
    CancelBeforeFinish,
    PreparationRefusal,
    AdmissionRefusal,
    PreparationPanic,
    AdmissionPanic,
    FinishPanic,
    LateLockLoss,
}
thread_local! { static CONTROL: Cell<Option<Cut>> = const { Cell::new(None) }; }

#[derive(Default)]
pub(super) struct DispatchControl {
    pause: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    resolver_panic: bool,
    cut: Option<Cut>,
}
thread_local! {
    static DISPATCH: std::cell::RefCell<DispatchControl> = std::cell::RefCell::new(DispatchControl::default());
    static RESPONSE_PANIC: Cell<bool> = const { Cell::new(false) };
}
pub(super) fn take_dispatch() -> DispatchControl {
    DISPATCH.with(|control| std::mem::take(&mut *control.borrow_mut()))
}
pub(super) fn install_dispatch(control: DispatchControl) {
    CONTROL.set(control.cut);
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
        panic!("resolver unwind after original pending input was taken");
    }
}
pub(super) fn before_response() {
    if RESPONSE_PANIC.replace(false) {
        panic!("response construction while original unused owner remains held");
    }
}

fn join_pending(slot: &SharedOperationSlot) {
    let worker = slot
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .archive
        .approval
        .worker
        .take();
    if let Some(worker) = worker {
        worker.join().unwrap();
    }
}

struct MutableClock(std::sync::atomic::AtomicI64);
impl Clock for MutableClock {
    fn now_unix(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn iso_week(&self) -> u8 {
        1
    }
}

fn pending_fixture() -> (
    tempfile::TempDir,
    crate::BackupWebDeps,
    Arc<MutableClock>,
    ReviewContext,
    Mark,
) {
    use sha2::{Digest, Sha256};
    let journal = crate::test_support::hosted_bound_root();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let source = journal.path().join("audio.flac");
    fs::write(&source, b"synthetic original").unwrap();
    let bytes = fs::read(&source).unwrap();
    let input = Input {
        mode: Cut::Complete,
        journal: journal.path().to_path_buf(),
        restic_path: journal.path().join("missing-restic"),
        repository_id: "a".repeat(64),
        snapshot_id: "b".repeat(64),
        archive_path: source.to_string_lossy().into_owned(),
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    let (mark, _) = seed(&input, &root);
    let context = read_review_context(&root, std::slice::from_ref(&mark.id)).unwrap();
    let mut deps = crate::BackupWebDeps::production(
        journal.path().to_path_buf(),
        crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
            free_bytes: None,
            total_bytes: None,
        }),
    );
    let clock = Arc::new(MutableClock(std::sync::atomic::AtomicI64::new(10_000)));
    deps.clock = clock.clone();
    deps.executable = journal.path().join("missing/bin/solstone");
    (journal, deps, clock, context, mark)
}

#[test]
fn stopped_lifetime_releases_actual_unused_approval_without_status_response() {
    let (journal, deps, _, context, _) = pending_fixture();
    let before = fs::read(journal.path().join("config/journal.json")).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
    start(&deps, context, now.date_naive(), now).unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if deps
            .operations
            .lock()
            .unwrap()
            .archive
            .approval
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual approval worker must return"
        );
        std::thread::park_timeout(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        deps.operations
            .lock()
            .unwrap()
            .archive
            .approval
            .unused
            .len(),
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
    assert!(state.archive.approval.worker.is_none());
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
fn approval_pending_inputs_preserve_original_budget_custody_and_exact_response() {
    for cut in [
        "spawn", "start", "tools", "resolver", "deadline", "cancel", "stale", "poison",
    ] {
        let (journal, deps, clock, context, mark) = pending_fixture();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        if cut == "spawn" {
            operation::fail_next_worker_start();
        } else {
            DISPATCH.with(|control| {
                *control.borrow_mut() = DispatchControl {
                    pause: Some((entered_tx, resume_rx)),
                    resolver_panic: cut == "resolver",
                    cut: None,
                }
            });
        }
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
        let started = start(&deps, context.clone(), now.date_naive(), now).unwrap();
        if cut != "spawn" {
            entered_rx.recv().unwrap();
            assert!(
                !status(&deps.operations, started.generation())
                    .unwrap()
                    .worker_settled
            );
            match cut {
                "deadline" => clock.0.store(
                    10_000 + 6 * 60 * 60 + 1,
                    std::sync::atomic::Ordering::SeqCst,
                ),
                "cancel" => {
                    assert!(
                        !status(&deps.operations, started.generation())
                            .unwrap()
                            .cancellation_requested
                    );
                    assert!(
                        cancel_response(&deps.operations, started.generation())
                            .unwrap()
                            .status()
                            .is_success()
                    );
                    assert!(
                        status(&deps.operations, started.generation())
                            .unwrap()
                            .cancellation_requested
                    );
                    assert!(
                        cancel_response(&deps.operations, started.generation())
                            .unwrap()
                            .status()
                            .is_success()
                    );
                    assert!(
                        status(&deps.operations, started.generation())
                            .unwrap()
                            .cancellation_requested
                    );
                    assert!(operation::is_busy(&deps.operations));
                }
                "start" => {
                    deps.operations
                        .lock()
                        .unwrap()
                        .current
                        .as_mut()
                        .unwrap()
                        .worker_active = true
                }
                "stale" => {
                    operation::install_slot(
                        &mut deps.operations.lock().unwrap().current,
                        "enable",
                        None,
                        None,
                        None,
                        None,
                    );
                }
                "poison" => {
                    assert!(
                        catch_unwind(AssertUnwindSafe(|| {
                            let _guard = deps.operations.lock().unwrap();
                            panic!("poison admission mutex");
                        }))
                        .is_err()
                    );
                }
                _ => {}
            }
            resume_tx.send(()).unwrap();
        }
        join_pending(&deps.operations);
        let state = deps
            .operations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let original = &state.archive.approval.unused[0];
        assert_eq!(original.input.checked.marks(), std::slice::from_ref(&mark));
        assert!(Arc::ptr_eq(
            &original.input.begin.ticket.0.clock,
            &deps.clock
        ));
        assert!(state.archive.matches(&original.input.begin.ticket));
        match cut {
            "cancel" => assert!(matches!(
                &original.refusal,
                UnusedRefusal::Budget(AdmissionError::Cancelled)
            )),
            "deadline" => assert!(matches!(
                &original.refusal,
                UnusedRefusal::Budget(AdmissionError::DeadlineExceeded)
            )),
            "resolver" => assert!(matches!(&original.refusal, UnusedRefusal::ResolverUnknown)),
            "tools" => assert!(matches!(&original.refusal, UnusedRefusal::Tools(_))),
            _ => assert!(matches!(&original.refusal, UnusedRefusal::Start)),
        }
        drop(state);
        assert!(operation::is_busy(&deps.operations));
        assert!(operation::begin(&deps.operations, "archive", None, None, None).is_err());
        assert_eq!(
            load_at_root(&JournalRoot::open(journal.path()).unwrap())
                .unwrap()
                .marks
                .get(&mark.id),
            Some(&mark)
        );
        assert!(
            journal
                .path()
                .join("chronicle/20261009/120000_60_fixture/audio.flac")
                .exists()
        );
        assert!(!journal.path().join("backup-activity.lock").exists());
        assert!(response(&deps.operations, "999").is_err());
        if matches!(cut, "stale" | "poison" | "start") {
            assert!(response(&deps.operations, started.generation()).is_err());
            assert!(operation::is_busy(&deps.operations));
        } else {
            RESPONSE_PANIC.set(true);
            assert!(response(&deps.operations, started.generation()).is_err());
            assert!(operation::is_busy(&deps.operations));
            assert!(
                response(&deps.operations, started.generation())
                    .unwrap()
                    .status()
                    .is_success()
            );
            assert_eq!(
                status(&deps.operations, started.generation())
                    .unwrap()
                    .state,
                ApprovalState::Refused
            );
            assert!(!operation::is_busy(&deps.operations));
            assert!(operation::begin(&deps.operations, "archive", None, None, None).is_ok());
            assert!(status(&deps.operations, started.generation()).is_err());
        }
    }
}

#[test]
fn approval_cancellation_refuses_other_generations_kinds_tickets_and_poison() {
    for cut in ["ordinary", "ticket", "poison"] {
        let (_journal, deps, _clock, context, _mark) = pending_fixture();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        DISPATCH.with(|control| {
            control.borrow_mut().pause = Some((entered_tx, resume_rx));
        });
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
        let started = start(&deps, context, now.date_naive(), now).unwrap();
        entered_rx.recv().unwrap();
        let flag = deps
            .operations
            .lock()
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .archive_cancellation
            .as_ref()
            .unwrap()
            .clone();
        for id in ["", "01", "+1", "1.0", "18446744073709551616", "2"] {
            assert_eq!(
                cancel_response(&deps.operations, id).unwrap_err(),
                ApprovalStatusRefusal::Stale
            );
            assert!(!flag.is_cancelled());
        }
        match cut {
            "ordinary" => {
                deps.operations
                    .lock()
                    .unwrap()
                    .current
                    .as_mut()
                    .unwrap()
                    .view
                    .kind = "backup".into()
            }
            "ticket" => deps.operations.lock().unwrap().archive.approval.ticket = None,
            "poison" => {
                assert!(
                    catch_unwind(AssertUnwindSafe(|| {
                        let _guard = deps.operations.lock().unwrap();
                        panic!("poison original cancellation mutex");
                    }))
                    .is_err()
                );
            }
            _ => unreachable!(),
        }
        let expected = if cut == "poison" {
            ApprovalStatusRefusal::Unavailable
        } else {
            ApprovalStatusRefusal::Stale
        };
        assert_eq!(
            cancel_response(&deps.operations, started.generation()).unwrap_err(),
            expected
        );
        assert!(!flag.is_cancelled());
        assert!(operation::is_busy(&deps.operations));
        resume_tx.send(()).unwrap();
        join_pending(&deps.operations);
        assert!(!flag.is_cancelled());
        assert!(operation::is_busy(&deps.operations));
    }
}

pub(super) fn after_actor(proof: &OwnedReviewedArchiveRun) {
    if CONTROL.get() == Some(Cut::ActorPanic) {
        assert_eq!(proof.outcome().removed_paths().count(), 1);
        assert_eq!(proof.attempts().len(), 1);
        assert_eq!(
            proof.attempts()[0].native().receipt().effect,
            RemovalEffect::Deleted
        );
        assert_eq!(proof.archive_reads().count(), 1);
        assert!(proof.mutation().unwrap().is_ok());
        panic!("actor unwind while actual removal and archive owners remain held");
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
    if CONTROL.get() == Some(Cut::CancelBeforeFinish) {
        cancel(slot);
    }
}
pub(super) fn before_handoff(slot: &SharedOperationSlot) {
    if CONTROL.get() == Some(Cut::LateLockLoss) {
        let state = slot.lock().unwrap();
        let root = &state.archive.reservation.as_ref().unwrap().root;
        let path = root.canonical_path().join("backup-activity.lock");
        fs::rename(&path, root.canonical_path().join("retired-test-lock")).unwrap();
        fs::write(&path, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    if CONTROL.get() == Some(Cut::Stale) {
        let mut state = slot.lock().unwrap();
        let current = state.current.as_mut().unwrap();
        current.generation += 1;
        current.view.report = Some(json!({"replacement":true}));
    }
}

#[derive(Deserialize)]
struct Input {
    mode: Cut,
    journal: PathBuf,
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

fn seed(input: &Input, root: &Arc<JournalRoot>) -> (Mark, PathBuf) {
    let target = solstone_core_retention::Target {
        day: "20261009".into(),
        stream: "_default".into(),
        dir: "120000_60_fixture".into(),
    };
    let relative =
        solstone_core_retention::layout::segment_rel(&target.day, &target.stream, &target.dir);
    let segment = input.journal.join(&relative);
    fs::create_dir_all(&segment).unwrap();
    fs::create_dir_all(input.journal.join("chronicle/20261009/health")).unwrap();
    fs::write(
        segment.join("audio.flac"),
        fs::read(&input.archive_path).unwrap(),
    )
    .unwrap();
    fs::write(segment.join("audio.jsonl"), format!(
        "{{\"_solstone_processing\":{{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"reason_code\":\"ok\",\"handler\":\"transcribe\",\"input_size\":{},\"attempted_at\":\"2026-10-09T12:00:00Z\"}}}}\n{{\"start\":0,\"text\":\"synthetic transcript\"}}\n",input.bytes
    )).unwrap();
    let directory =
        MediaDirectory::open(Arc::clone(root), std::path::Path::new(&relative)).unwrap();
    let scan = solstone_core_retention::scan::scan_segment_bound(
        &directory,
        &ClosedHandlerSet,
        &JournalMedia,
        65_536,
    )
    .unwrap();
    let file = &scan.files()[0];
    let processing = AcceptedProcessingObservation::capture(
        &ClosedHandlerSet,
        &target,
        &file.name,
        file.size,
        &file.sidecar,
    )
    .unwrap();
    let observed = directory
        .observe_file(std::ffi::OsStr::new("audio.flac"))
        .unwrap()
        .unwrap();
    assert_eq!(observed.digest.size, input.bytes);
    assert_eq!(observed.digest.sha256, input.sha256);
    let proof = OffloadProof {
        journal_id: root.identity().observation_id(),
        manifest_id: "synthetic-worker-manifest".into(),
        target: target.clone(),
        layout: SegmentLayout::Direct,
        repository_id: input.repository_id.clone(),
        snapshot_id: input.snapshot_id.clone(),
        files: vec![ArchiveFileProof {
            witness_id: "synthetic-worker-witness".into(),
            name: "audio.flac".into(),
            bytes: input.bytes,
            sha256: input.sha256.clone(),
            // Pending-admission fixtures read a native local source above;
            // their synthetic archive locator uses the backup's path syntax.
            archive_path: if cfg!(windows) {
                "/synthetic/audio.flac".into()
            } else {
                input.archive_path.clone()
            },
            processing,
        }],
    };
    let event = codec::Event::ArchiveV2 {
        schema_version: 2,
        time: 1,
        day: target.day.clone(),
        stream: target.stream.clone(),
        segment: "120000_60".into(),
        segment_dir: target.dir.clone(),
        layout: proof.layout,
        journal_id: proof.journal_id.clone(),
        manifest_id: proof.manifest_id.clone(),
        repository_id: proof.repository_id.clone(),
        snapshot_id: proof.snapshot_id.clone(),
        files: proof.files.clone(),
    };
    let ledger = input.journal.join("health/offload");
    fs::create_dir_all(&ledger).unwrap();
    let mut bytes = serde_json::to_vec(&event).unwrap();
    bytes.push(b'\n');
    fs::write(ledger.join("20261009.jsonl"), bytes).unwrap();
    let names = vec!["audio.flac".into()];
    let mark = Mark {
        id: MarkId::derive(RemovalClass::OffloadRawRelease, &target, &names),
        target,
        class: RemovalClass::OffloadRawRelease,
        marked_at: "2026-10-09T12:00:00Z".into(),
        proposal: Proposal {
            names,
            bytes: input.bytes,
            reason: "synthetic control".into(),
        },
        state: MarkState::Marked,
        offload: Some(OffloadOffer::new(proof).unwrap()),
    };
    let mut register = Register {
        version: 2,
        marks: Default::default(),
    };
    register.marks.insert(mark.id.clone(), mark.clone());
    fs::write(
        input.journal.join("health/retention-marks.json"),
        serde_json::to_vec(&register).unwrap(),
    )
    .unwrap();
    assert!(
        read_complete_catalog(root)
            .unwrap()
            .active_witness(
                &mark.offload.as_ref().unwrap().proof,
                &mark.offload.as_ref().unwrap().proof.files[0]
            )
            .is_some()
    );
    (mark, segment)
}

fn blocked(slot: &SharedOperationSlot, root: &Arc<JournalRoot>, clock: &super::super::OwnedClock) {
    assert!(operation::is_busy(slot));
    assert!(operation::begin(slot, "archive", None, None, None).is_err());
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
fn positive(report: &Value) {
    assert_eq!(report["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(report["attempts"][0]["native"]["effect"], "deleted");
    assert_eq!(report["attempts"][0]["append"]["effect"], "appended");
    assert_eq!(
        report["attempts"][0]["marker"]["publication"]["effect"],
        "published"
    );
    assert_eq!(report["mutation"]["publication"], "published");
    assert_eq!(report["archive_reads"][0]["cleanup"], "complete");
    assert_eq!(
        report["outcome"]["targets"][0]["removed"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
#[ignore = "requires copied package and synthetic approval entrance fixture"]
fn packaged_owned_approval_entrance() {
    let input: Input = serde_json::from_slice(
        &fs::read(std::env::var_os("OWNED_APPROVAL_JSON").unwrap()).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        input.mode,
        Cut::Complete | Cut::Stale | Cut::CancelBeforeFinish
    ));
    let root = Arc::new(JournalRoot::open(&input.journal).unwrap());
    let (mark, segment) = seed(&input, &root);
    let context = read_review_context(&root, std::slice::from_ref(&mark.id)).unwrap();
    let mut deps = crate::BackupWebDeps::production(
        input.journal.clone(),
        crate::measurement::with_geometry(crate::measurement::DeviceGeometry {
            free_bytes: None,
            total_bytes: None,
        }),
    );
    deps.executable = std::env::current_exe().unwrap();
    let clock: super::super::OwnedClock = Arc::new(LiveClock(Instant::now()));
    deps.clock = Arc::clone(&clock);
    DISPATCH.with(|control| control.borrow_mut().cut = Some(input.mode));
    let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
    let started = start(&deps, context, now.date_naive(), now).unwrap();
    join_pending(&deps.operations);
    assert!(!segment.join("audio.flac").exists());
    assert!(load_at_root(&root).unwrap().marks.is_empty());
    let observed = status(&deps.operations, started.generation());
    let mut result = json!({"mode":format!("{:?}",input.mode),"typed_removed":true});
    if input.mode == Cut::Complete {
        let observed = observed.unwrap();
        assert!(observed.worker_settled);
        assert_eq!(observed.state, ApprovalState::Completed);
        assert_eq!(observed.actor_phase.as_deref(), Some("done"));
        positive(observed.report.as_ref().unwrap());
        assert_eq!(
            observed.report.as_ref().unwrap()["runtime"]["state"],
            "completed"
        );
        assert!(!operation::is_busy(&deps.operations));
        assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
        assert!(TeardownLease::try_acquire(Arc::clone(&root)).is_ok());
        result["status"] = serde_json::to_value(observed).unwrap();
        assert!(operation::begin(&deps.operations, "enable", None, None, None).is_ok());
        assert!(status(&deps.operations, started.generation()).is_err());
        result["successor_admitted"] = json!(true);
    } else {
        let state = deps.operations.lock().unwrap();
        let returned = &state.archive.retained[0];
        let ArchivePayload::ProofFinished { report, runtime } = returned.payload() else {
            panic!("whole actual finished actor plus Runtime owner");
        };
        assert_eq!(report.outcome().removed_paths().count(), 1);
        assert!(Arc::ptr_eq(runtime.owned_clock(), &clock));
        assert_eq!(returned.terminal().phase, "done");
        positive(returned.terminal().report.as_ref().unwrap());
        result["original_report"] = returned.terminal().report.clone().unwrap();
        if input.mode == Cut::Stale {
            assert_eq!(runtime.state(), DirectReadFinishState::Completed);
            assert_eq!(
                returned.reason(),
                Some(super::super::ArchiveHandoffRefusal::Stale)
            );
            assert!(matches!(observed, Err(ApprovalStatusRefusal::Stale)));
            assert_eq!(
                state.current.as_ref().unwrap().view.report,
                Some(json!({"replacement":true}))
            );
            result["stale_refused"] = json!(true);
        } else {
            let observed = observed.unwrap();
            assert_eq!(runtime.state(), DirectReadFinishState::Held);
            assert_eq!(observed.state, ApprovalState::Held);
            assert_eq!(observed.actor_phase.as_deref(), Some("done"));
            assert!(observed.worker_settled);
            assert_eq!(observed.report, returned.terminal().report);
            result["status"] = serde_json::to_value(observed).unwrap();
        }
        drop(state);
        assert!(operation::is_busy(&deps.operations));
        assert!(operation::begin(&deps.operations, "enable", None, None, None).is_err());
        assert!(
            response(&deps.operations, started.generation()).is_err()
                || operation::is_busy(&deps.operations)
        );
        result["held_busy"] = json!(true);
    }
    println!("OWNED_APPROVAL_RESULT {}", result);
}

#[test]
fn approval_unknown_snapshot_keeps_actual_preparation_custody() {
    use solstone_core_backup::archive_activity::OwnedArchivePreparationInput;
    use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};
    let (_journal, deps, _clock, context, _mark) = pending_fixture();
    let root = Arc::new(JournalRoot::open(&deps.journal_root).unwrap());
    let checked = check_review_context(Arc::clone(&root), &context).unwrap();
    let begin = begin_owned(
        &deps.operations,
        KIND,
        Arc::clone(&root),
        Arc::clone(&deps.clock),
    )
    .unwrap();
    let generation = begin.generation();
    {
        let mut state = deps.operations.lock().unwrap();
        state.archive.approval.generation = Some(generation);
        state.archive.approval.ticket = Some(ArchiveTicket(Arc::clone(&begin.ticket.0)));
    }
    let slot = Arc::clone(&deps.operations);
    std::thread::spawn(move || {
        assert!(super::super::start_owned_worker(&slot, &begin.ticket));
        let (ticket, root, budget, clock) = begin.into_parts();
        let (input, custody) = OwnedArchivePreparationInput::capture(root, budget, clock)
            .unwrap_or_else(|_| panic!("actual preparation capture"));
        let (result, consumed) = run_with_media_hook(
            MediaPrimitive::CarrierConfirmation,
            || panic!("actual preparation publication unwind"),
            || catch_unwind(AssertUnwindSafe(|| input.prepare())),
        );
        assert!(consumed && result.is_err());
        return_owned(
            &slot,
            ticket,
            Some(custody),
            ArchivePayload::EarlyUnknown,
            Terminal::error("unknown"),
        );
        slot.lock().unwrap().archive.approval.refused_selection = Some(checked);
    })
    .join()
    .unwrap();
    let observed = status(&deps.operations, &generation.to_string()).unwrap();
    assert_eq!(observed.state, ApprovalState::Unknown);
    assert!(observed.worker_settled);
    assert!(!observed.cancellation_requested);
    assert!(
        cancel_response(&deps.operations, &generation.to_string())
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        status(&deps.operations, &generation.to_string())
            .unwrap()
            .cancellation_requested
    );
    assert!(operation::is_busy(&deps.operations));
    assert!(
        response(&deps.operations, &generation.to_string())
            .unwrap()
            .status()
            .is_success()
    );
    assert!(operation::is_busy(&deps.operations));
    assert!(matches!(
        ActivityLease::try_acquire(root),
        Err(ActivityAdmissionError::Busy)
    ));
}

#[test]
#[ignore = "requires copied package and synthetic installed reviewed-removal fixture"]
fn packaged_owned_removal_worker() {
    let input: Input = serde_json::from_slice(
        &fs::read(std::env::var_os("OWNED_REMOVAL_WORKER_JSON").unwrap()).unwrap(),
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(&input.journal).unwrap());
    let (mark, segment) = seed(&input, &root);
    let context = read_review_context(&root, std::slice::from_ref(&mark.id)).unwrap();
    let checked = check_review_context(Arc::clone(&root), &context).unwrap();
    if input.mode == Cut::Unproven {
        fs::copy(segment.join("audio.flac"), segment.join("extra.flac")).unwrap();
    }
    let slot = operation::new_slot();
    let clock: super::super::OwnedClock = Arc::new(LiveClock(Instant::now()));
    let begin = begin_owned(
        &slot,
        "offload_approve",
        Arc::clone(&root),
        Arc::clone(&clock),
    )
    .unwrap();
    let generation = begin.generation();
    let worker_slot = Arc::clone(&slot);
    let restic = input.restic_path.clone();
    let repository = input.repository_id.clone();
    let mode = input.mode;
    if mode == Cut::PreparationRefusal {
        fs::write(input.journal.join("config/journal.json"), b"{").unwrap();
    }
    let worker = thread::spawn(move || {
        use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};
        CONTROL.set(Some(mode));
        let prepare = || prepare_read(&worker_slot, begin, repository, &restic);
        let (ready, preparation_hook, admission_hook) = if mode == Cut::PreparationPanic {
            let (ready, consumed) = run_with_media_hook(
                MediaPrimitive::CarrierRootSync,
                || panic!("actual stage creation"),
                prepare,
            );
            (ready, consumed, false)
        } else if mode == Cut::AdmissionPanic {
            let (ready, consumed) =
                solstone_core_backup_runtime::test_support::run_with_owned_read_admission_hook(
                    || panic!("actual admitted read before installed pair"),
                    prepare,
                );
            (ready, false, consumed)
        } else {
            (prepare(), false, false)
        };
        let Some(ready) = ready else {
            CONTROL.set(None);
            return (None, None, preparation_hook, admission_hook, false);
        };
        let carrier = Arc::downgrade(ready.outside.carrier());
        let lease = Arc::downgrade(ready.outside.carrier().activity());
        assert!(std::ptr::eq(
            ready.custody.activity(),
            ready.outside.carrier().activity().as_ref()
        ));
        if mode == Cut::CancelBeforeActor {
            cancel(&worker_slot);
        }
        let run = || {
            remove(
                &worker_slot,
                ready,
                checked,
                NaiveDate::from_ymd_opt(2026, 10, 11).unwrap(),
                Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
                    .single()
                    .unwrap(),
            )
        };
        let finish_hook = if mode == Cut::FinishPanic {
            run_with_media_hook(
                MediaPrimitive::CarrierRetirementRootSync,
                || panic!("actual consuming carrier retirement after removal"),
                run,
            )
            .1
        } else {
            run();
            false
        };
        CONTROL.set(None);
        (
            Some(carrier),
            Some(lease),
            preparation_hook,
            admission_hook,
            finish_hook,
        )
    });
    let (carrier, lease, preparation_hook, admission_hook, finish_hook) = worker.join().unwrap();
    let guard = slot.lock().unwrap();
    let current = guard.current.as_ref().unwrap();
    let mut result = json!({"mode":format!("{mode:?}"),"view":current.view,"preparation_hook":preparation_hook,"admission_hook":admission_hook,"finish_hook":finish_hook});
    if mode == Cut::Complete {
        assert!(guard.archive.retained.is_empty() && guard.archive.reservation.is_none());
        assert_eq!(current.view.phase, "done");
        let report = current.view.report.as_ref().unwrap();
        positive(report);
        assert_eq!(report["archive_reads"][0]["state"], "confirmed");
        assert_eq!(report["runtime"]["state"], "completed");
        assert_eq!(report["runtime"]["retirement"]["effect"], "removed");
        assert!(
            carrier.as_ref().unwrap().upgrade().is_none()
                && lease.as_ref().unwrap().upgrade().is_none()
        );
        drop(guard);
        assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
        assert!(TeardownLease::try_acquire(Arc::clone(&root)).is_ok());
        let next = begin_owned(&slot, "archive", Arc::clone(&root), Arc::clone(&clock)).unwrap();
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
        } else if mode == Cut::LateLockLoss {
            assert_eq!(
                retained.reason(),
                Some(super::super::ArchiveHandoffRefusal::BindingChanged)
            );
            assert!(retained.binding_error().is_some());
        } else {
            assert!(retained.reason().is_none());
        }
        let custody = retained.custody().expect("actual original custody");
        assert!(Arc::ptr_eq(custody.journal_root(), &root));
        assert!(Arc::ptr_eq(custody.owned_clock(), &clock));
        assert_eq!(custody.revalidate().is_err(), mode == Cut::LateLockLoss);
        let facts = retained.terminal().report.as_ref();
        match retained.payload() {
            ArchivePayload::ReadHeld { read, outside } => {
                assert_eq!(mode, Cut::CancelBeforeActor);
                assert!(Arc::ptr_eq(read.carrier(), outside.carrier()));
                assert!(Arc::ptr_eq(outside.journal_root(), &root));
                assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
                assert!(facts.is_none());
                assert!(segment.join("audio.flac").exists());
                assert_eq!(
                    load_at_root(&root).unwrap().marks.get(&mark.id),
                    Some(&mark)
                );
            }
            ArchivePayload::ProofHeld { proof, outside } => {
                assert_eq!(mode, Cut::Unproven);
                assert!(Arc::ptr_eq(proof.archive_carrier(), outside.carrier()));
                assert!(Arc::ptr_eq(outside.journal_root(), &root));
                assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
                assert!(matches!(
                    proof.finish_refusal(),
                    Some(ReviewedArchiveFinishRefusal::IncompleteSelection)
                ));
                assert!(proof.refusal().is_none());
                assert!(proof.attempts().is_empty());
                assert_eq!(proof.selected_marks(), std::slice::from_ref(&mark));
                assert!(segment.join("audio.flac").exists());
                assert_eq!(
                    load_at_root(&root).unwrap().marks.get(&mark.id),
                    Some(&mark)
                );
                let report = facts.unwrap();
                assert_eq!(report["finish_refusal"]["kind"], "incomplete_selection");
                assert_eq!(report["attempts"], json!([]));
                assert_eq!(report["mutation"]["publication"], "unchanged");
                assert_eq!(report["settlement"]["state"], "held");
                assert_eq!(proof.archive_reads().count(), 1);
            }
            ArchivePayload::ProofFinished { report, runtime } => {
                assert!(matches!(
                    mode,
                    Cut::Stale | Cut::CancelBeforeFinish | Cut::LateLockLoss
                ));
                positive(facts.unwrap());
                assert_eq!(facts.unwrap()["archive_reads"][0]["state"], "confirmed");
                assert_eq!(report.outcome().removed_paths().count(), 1);
                assert!(Arc::ptr_eq(runtime.journal_root(), &root));
                assert!(Arc::ptr_eq(runtime.owned_clock(), &clock));
                assert_eq!(
                    runtime.state(),
                    if matches!(mode, Cut::Stale | Cut::LateLockLoss) {
                        DirectReadFinishState::Completed
                    } else {
                        DirectReadFinishState::Held
                    }
                );
                if matches!(mode, Cut::Stale | Cut::LateLockLoss) {
                    assert!(std::ptr::eq(
                        custody.activity(),
                        runtime.retirement().unwrap().owned.activity().as_ref()
                    ));
                    assert!(std::ptr::eq(
                        Arc::as_ptr(runtime.retirement().unwrap().owned.activity()),
                        lease.as_ref().unwrap().as_ptr()
                    ));
                } else {
                    assert!(runtime.reason().is_some());
                }
            }
            ArchivePayload::ProofUnknown { runtime } => {
                assert_eq!(mode, Cut::ActorPanic);
                positive(facts.unwrap());
                assert_eq!(facts.unwrap()["archive_reads"][0]["state"], "prepared");
                assert_eq!(runtime.state(), DirectReadFinishState::Unknown);
                assert!(runtime.retirement().is_none());
                assert!(Arc::ptr_eq(runtime.journal_root(), &root));
                assert!(Arc::ptr_eq(runtime.owned_clock(), &clock));
                assert_eq!(facts.unwrap()["settlement"]["state"], "unknown");
            }
            ArchivePayload::PreparationFailure(failure) => {
                assert_eq!(mode, Cut::PreparationRefusal);
                assert!(failure.carrier().is_some());
                assert!(Arc::ptr_eq(failure.journal_root(), &root));
                assert!(Arc::ptr_eq(failure.owned_clock(), &clock));
                assert_eq!(facts.unwrap()["preparation"]["state"], "refused");
            }
            ArchivePayload::AdmissionFailure(failure) => {
                assert_eq!(mode, Cut::AdmissionRefusal);
                assert!(failure.effects().broker_requested);
                assert!(
                    !failure.effects().write_repository_requested
                        && !failure.effects().read_repository_requested
                );
                assert!(failure.carrier().is_some());
                assert!(Arc::ptr_eq(failure.owned_clock().unwrap(), &clock));
                assert_eq!(facts.unwrap()["admission"]["broker_requested"], true);
            }
            ArchivePayload::EarlyUnknown => {
                assert!(matches!(mode, Cut::PreparationPanic | Cut::AdmissionPanic));
                assert_eq!(preparation_hook, mode == Cut::PreparationPanic);
                assert_eq!(admission_hook, mode == Cut::AdmissionPanic);
                assert!(
                    !solstone_core_journal_io::activity_obligations::observe_obligations(&root)
                        .unwrap()
                        .entries()
                        .is_empty()
                );
                result["early_unknown"] = json!(true);
            }
            ArchivePayload::ProofFinishUnknown { report } => {
                assert_eq!(mode, Cut::FinishPanic);
                assert!(finish_hook);
                positive(facts.unwrap());
                assert_eq!(report.outcome().removed_paths().count(), 1);
                assert_eq!(facts.unwrap()["settlement"]["state"], "settled");
                assert_eq!(facts.unwrap()["runtime"]["state"], "unknown");
                assert!(
                    solstone_core_journal_io::activity_obligations::observe_obligations(&root)
                        .unwrap()
                        .entries()
                        .is_empty()
                );
                result["observed_stage_removed"] = json!(true);
            }
            _ => panic!("actual concrete Proof custody required"),
        }
        result["terminal"] = json!({"phase":retained.terminal().phase,"reason_code":retained.terminal().reason_code,"report":retained.terminal().report});
        result["held"] = json!(true);
        if matches!(
            mode,
            Cut::PreparationRefusal
                | Cut::AdmissionRefusal
                | Cut::PreparationPanic
                | Cut::AdmissionPanic
        ) {
            assert!(segment.join("audio.flac").exists());
            assert_eq!(
                load_at_root(&root).unwrap().marks.get(&mark.id),
                Some(&mark)
            );
        }
        result["binding_error"] = retained
            .binding_error()
            .map(|error| error.kind().to_string())
            .into();
        if mode == Cut::LateLockLoss {
            let current = guard.current.as_ref().unwrap();
            assert_eq!(current.view.phase, "error");
            assert_eq!(current.view.reason_code.as_deref(), Some("failed"));
            assert!(!current.worker_active);
            assert_eq!(current.view.report, retained.terminal().report);
            assert_eq!(current.view.detail, retained.terminal().detail);
            assert_eq!(current.view.guidance, retained.terminal().guidance);
            assert_eq!(
                current.view.recording_failure,
                retained.terminal().recording_failure
            );
            assert_eq!(retained.terminal().phase, "done");
        }
        drop(guard);
        if mode == Cut::LateLockLoss {
            assert!(operation::is_busy(&slot));
            assert!(begin_owned(&slot, "archive", Arc::clone(&root), Arc::clone(&clock)).is_err());
            // The replacement native lock can be acquired; keeper refusal is
            // independent of exclusion on the displaced original file.
            assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
        } else {
            blocked(&slot, &root, &clock);
        }
        if let Some(lease) = lease {
            assert!(lease.upgrade().is_some());
        }
        if !matches!(mode, Cut::Stale | Cut::LateLockLoss | Cut::FinishPanic)
            && let Some(carrier) = carrier
        {
            assert!(carrier.upgrade().is_some());
        }
    }
    if matches!(
        mode,
        Cut::Complete
            | Cut::Stale
            | Cut::ActorPanic
            | Cut::CancelBeforeFinish
            | Cut::FinishPanic
            | Cut::LateLockLoss
    ) {
        assert!(!segment.join("audio.flac").exists());
        assert!(load_at_root(&root).unwrap().marks.is_empty());
    }
    println!("OWNED_REMOVAL_WORKER_RESULT {}", result);
}

/// A journal holding policy marks only, with a day rule old rows satisfy.
fn policy_fixture(
    names: &[&str],
    legacy: bool,
) -> (tempfile::TempDir, Arc<JournalRoot>, Vec<Mark>) {
    let journal = tempfile::tempdir().unwrap();
    fs::create_dir(journal.path().join("config")).unwrap();
    fs::create_dir(journal.path().join("health")).unwrap();
    fs::write(
        journal.path().join("config/journal.json"),
        br#"{"retention":{"raw_media":"days","raw_media_days":1}}"#,
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let target = solstone_core_retention::Target {
        day: "20261009".into(),
        stream: "_default".into(),
        dir: "120000_60_policy".into(),
    };
    let segment = journal
        .path()
        .join(solstone_core_retention::layout::segment_rel(
            &target.day,
            &target.stream,
            &target.dir,
        ));
    fs::create_dir_all(&segment).unwrap();
    fs::create_dir_all(journal.path().join("chronicle/20261009/health")).unwrap();
    fs::write(segment.join("audio.flac"), b"raw").unwrap();
    fs::write(
        segment.join("audio.jsonl"),
        "{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"reason_code\":\"ok\",\"handler\":\"transcribe\",\"input_size\":3,\"attempted_at\":\"2026-10-09T12:00:00Z\"}}\n{\"start\":0,\"text\":\"x\"}\n",
    )
    .unwrap();
    let mut marks = Vec::new();
    let mut classes = vec![RemovalClass::PolicyRawRelease];
    if legacy {
        classes.push(RemovalClass::OffloadRawRelease);
    }
    for class in classes {
        let names = names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        marks.push(Mark {
            id: MarkId::derive(class, &target, &names),
            class,
            target: target.clone(),
            marked_at: "2026-10-10T00:00:00Z".into(),
            proposal: Proposal {
                names,
                bytes: 3,
                reason: "synthetic policy".into(),
            },
            state: MarkState::Marked,
            offload: None,
        });
    }
    let mut register = Register::empty();
    for mark in &marks {
        register.marks.insert(mark.id.clone(), mark.clone());
    }
    fs::write(
        journal.path().join("health/retention-marks.json"),
        serde_json::to_vec(&register).unwrap(),
    )
    .unwrap();
    (journal, root, marks)
}

fn policy_start(
    journal: &tempfile::TempDir,
    root: &Arc<JournalRoot>,
    marks: &[Mark],
) -> (
    crate::BackupWebDeps,
    Result<ApprovalStart, ApprovalStartRefusal>,
) {
    let ids = marks.iter().map(|mark| mark.id.clone()).collect::<Vec<_>>();
    let context = read_review_context(root, &ids).unwrap();
    let mut deps = crate::BackupWebDeps::new(journal.path().to_path_buf());
    // A local policy release never resolves or runs the backup tools.
    deps.executable = journal.path().join("missing/bin/solstone");
    let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
    let started = start(&deps, context, now.date_naive(), now);
    join_pending(&deps.operations);
    (deps, started)
}

#[test]
fn policy_only_approval_settles_locally_and_frees_the_slot() {
    let (journal, root, marks) = policy_fixture(&["audio.flac"], false);
    let raw = journal
        .path()
        .join("chronicle/20261009/120000_60_policy/audio.flac");
    let (deps, started) = policy_start(&journal, &root, &marks);
    let started = started.unwrap();
    let observed = status(&deps.operations, started.generation()).unwrap();
    assert!(observed.worker_settled);
    assert_eq!(observed.state, ApprovalState::Completed);
    assert_eq!(
        observed.actor_phase.as_deref(),
        Some("done"),
        "{observed:?}"
    );
    let report = observed.report.as_ref().unwrap();
    assert_eq!(report["settlement"]["state"], "settled");
    assert_eq!(report["attempts"][0]["native"]["effect"], "deleted");
    assert_eq!(report["mutation"]["publication"], "published");
    assert!(!raw.exists());
    // It waits in the trash, settled once the journal was handed back, and
    // the history says trashed, never removed.
    let items = solstone_core_trash::list(journal.path()).unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(matches!(
        &items[0].unit,
        solstone_core_removals::Unit::Originals { files, .. } if files == &["audio.flac".to_owned()]
    ));
    let held = journal
        .path()
        .join(solstone_core_trash::TRASH_DIR)
        .join(&items[0].id)
        .join("files/chronicle/20261009/120000_60_policy/audio.flac");
    assert_eq!(fs::read(held).unwrap(), b"raw");
    let history: Vec<_> = solstone_core_removals::history(journal.path(), 50)
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| (entry.event, entry.action, entry.surface))
        .collect();
    assert_eq!(
        history,
        [(
            solstone_core_removals::Event::Trashed,
            solstone_core_removals::Action::OriginalsRelease,
            solstone_core_removals::Surface::Web {
                page: solstone_core_removals::Page::Home
            }
        )]
    );
    assert!(load_at_root(&root).unwrap().marks.is_empty());
    assert!(!operation::is_busy(&deps.operations));
    assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
    assert!(
        response(&deps.operations, started.generation())
            .unwrap()
            .status()
            .is_success()
    );
    assert!(operation::begin(&deps.operations, "enable", None, None, None).is_ok());
}

#[test]
fn policy_only_known_unremoved_files_settle_and_free_the_slot() {
    // One original is released, one is kept by the current policy and one is
    // already gone: every outcome is known, so nothing stays held.
    let (journal, root, marks) = policy_fixture(&["a.flac", "audio.flac", "gone.flac"], false);
    let segment = journal.path().join("chronicle/20261009/120000_60_policy");
    fs::write(segment.join("a.flac"), b"raw").unwrap();
    fs::write(
        segment.join("a.jsonl"),
        "{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"empty\",\"reason_code\":\"no_decodable_audio\",\"handler\":\"transcribe\",\"input_size\":3,\"attempted_at\":\"2026-10-09T12:00:00Z\"}}\n",
    )
    .unwrap();
    fs::write(
        journal.path().join("config/journal.json"),
        br#"{"retention":{"raw_media":"keep"}}"#,
    )
    .unwrap();
    let (deps, started) = policy_start(&journal, &root, &marks);
    let started = started.unwrap();
    let observed = status(&deps.operations, started.generation()).unwrap();
    assert!(observed.worker_settled);
    assert_eq!(observed.state, ApprovalState::Completed);
    assert_eq!(
        observed.actor_phase.as_deref(),
        Some("degraded"),
        "{observed:?}"
    );
    let report = observed.report.as_ref().unwrap();
    assert_eq!(report["settlement"]["state"], "settled");
    let row = &report["outcome"]["targets"][0];
    assert_eq!(row["removed"].as_array().unwrap().len(), 1);
    let reasons = row["not_removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["entry"].as_str().unwrap().rsplit('/').next().unwrap(),
                item["reason"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        [
            ("audio.flac", "this one is kept indefinitely."),
            (
                "gone.flac",
                "this file was on the removal list but is no longer present"
            ),
        ]
    );
    assert!(!segment.join("a.flac").exists());
    assert!(segment.join("audio.flac").exists());
    assert!(!operation::is_busy(&deps.operations));
    assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
    assert!(
        crate::archive_operation::begin(&deps.operations, "backup", deps.clock.as_ref()).is_ok()
    );
}

#[test]
fn policy_only_uncertain_remnant_stays_in_the_slot_and_status_reports_it() {
    let (journal, root, marks) = policy_fixture(&["audio.flac"], false);
    let segment = journal.path().join("chronicle/20261009/120000_60_policy");
    // The deletion succeeds but its event record cannot be written.
    fs::create_dir(segment.join("events.jsonl")).unwrap();
    let (deps, started) = policy_start(&journal, &root, &marks);
    let started = started.unwrap();
    let observed = status(&deps.operations, started.generation()).unwrap();
    assert!(observed.worker_settled);
    assert_eq!(observed.state, ApprovalState::Held, "{observed:?}");
    assert_eq!(observed.actor_phase.as_deref(), Some("degraded"));
    assert_eq!(observed.reason_code.as_deref(), Some("failed"));
    assert_eq!(
        observed.report.as_ref().unwrap()["settlement"]["state"],
        "held"
    );
    assert!(!segment.join("audio.flac").exists());
    // The held result keeps the journal: the trash's item waits, still
    // holding the original, for the sweep once the hold is let go.
    let waiting = solstone_core_trash::stopped(journal.path()).unwrap();
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert_eq!(waiting[0].state, solstone_core_trash::State::Detaching);
    assert!(
        journal
            .path()
            .join(solstone_core_trash::TRASH_DIR)
            .join(&waiting[0].id)
            .join("files/chronicle/20261009/120000_60_policy/audio.flac")
            .is_file()
    );
    {
        let state = deps.operations.lock().unwrap();
        let returned = state.archive_returns().next().unwrap();
        let ArchivePayload::OrdinaryHeld(held) = returned.payload() else {
            panic!("the whole local owner stays in the slot");
        };
        assert_eq!(held.removal().selected_marks(), marks.as_slice());
        assert!(held.removal().carrier_preparation().is_some());
    }
    // Reading the status never releases the retained hold.
    assert!(
        response(&deps.operations, started.generation())
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        status(&deps.operations, started.generation())
            .unwrap()
            .state,
        ApprovalState::Held
    );
    let clock = Arc::clone(&deps.clock);
    blocked(&deps.operations, &root, &clock);
}

/// A segment something else holds can't go to the trash now, so the approval
/// doesn't start: nothing is released, and no item is left.
#[test]
fn a_busy_segment_refuses_the_approval_and_releases_nothing() {
    let (journal, root, marks) = policy_fixture(&["audio.flac"], false);
    let segment = journal.path().join("chronicle/20261009/120000_60_policy");
    let _held = solstone_core_journal_io::locking::hold_lock(
        &segment,
        solstone_core_journal_io::LockOptions::default(),
    )
    .unwrap();
    let (deps, started) = policy_start(&journal, &root, &marks);
    let started = started.unwrap();
    let observed = status(&deps.operations, started.generation()).unwrap();
    assert_eq!(observed.state, ApprovalState::Refused, "{observed:?}");
    assert_eq!(observed.reason_code.as_deref(), Some("trash_unavailable"));
    assert!(segment.join("audio.flac").exists());
    assert!(
        solstone_core_trash::list(journal.path())
            .unwrap()
            .is_empty()
    );
    assert!(
        solstone_core_trash::stopped(journal.path())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unbound_offload_row_refuses_before_start() {
    let (journal, root, marks) = policy_fixture(&["audio.flac"], true);
    let raw = journal
        .path()
        .join("chronicle/20261009/120000_60_policy/audio.flac");
    let before = fs::read(journal.path().join("health/retention-marks.json")).unwrap();
    for selected in [&marks[1..], &marks[..]] {
        let (deps, started) = policy_start(&journal, &root, selected);
        assert!(matches!(
            started,
            Err(ApprovalStartRefusal::UnsupportedSelection)
        ));
        assert!(!operation::is_busy(&deps.operations));
    }
    assert_eq!(fs::read(&raw).unwrap(), b"raw");
    assert_eq!(
        fs::read(journal.path().join("health/retention-marks.json")).unwrap(),
        before
    );
}
