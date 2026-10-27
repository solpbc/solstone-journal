// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_journal_io::activity::{ActivityAdmissionError, ActivityLease, TeardownLease};
use std::{sync::mpsc, thread, time::Duration};

#[test]
fn retained_archive_root_keeps_original_custody_and_refuses_replacement() {
    let parent = tempfile::tempdir().unwrap();
    let journal = parent.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    let root = Arc::new(JournalRoot::open(&journal).unwrap());
    let deps = crate::BackupWebDeps::with_root(Arc::clone(&root)).unwrap();
    let retained = retained_archive_root(&deps).unwrap();
    assert!(Arc::ptr_eq(&root, &retained));
    let original = parent.path().join("original");
    std::fs::rename(&journal, &original).unwrap();
    std::fs::create_dir(&journal).unwrap();
    assert!(retained_archive_root(&deps).is_err());
    assert!(std::fs::read_dir(&journal).unwrap().next().is_none());
    assert!(std::fs::read_dir(&original).unwrap().next().is_none());
}

#[test]
fn deferred_backup_owner_does_not_rebind_before_its_first_worker() {
    let parent = tempfile::tempdir().unwrap();
    let journal = parent.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    let deps = crate::BackupWebDeps::new(journal.clone());
    std::fs::rename(&journal, parent.path().join("original")).unwrap();
    std::fs::create_dir(&journal).unwrap();
    assert!(retained_archive_root(&deps).is_err());
    assert!(std::fs::read_dir(&journal).unwrap().next().is_none());
}

#[test]
fn backup_path_alias_is_checked_without_changing_the_root_owner() {
    let parent = tempfile::tempdir().unwrap();
    let journal = parent.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    let alias = parent.path().join("alias");
    std::os::unix::fs::symlink(&journal, &alias).unwrap();
    let root = Arc::new(JournalRoot::open(&journal).unwrap());
    let mut deps = crate::BackupWebDeps::with_root(Arc::clone(&root)).unwrap();
    deps.journal_root = alias;
    assert!(Arc::ptr_eq(&root, &retained_archive_root(&deps).unwrap()));
    let other = parent.path().join("other");
    std::fs::create_dir(&other).unwrap();
    deps.journal_root = other;
    assert!(retained_archive_root(&deps).is_err());
}

struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        100
    }
    fn iso_week(&self) -> u8 {
        1
    }
}

fn fixture() -> (
    tempfile::TempDir,
    Arc<JournalRoot>,
    OwnedClock,
    SharedOperationSlot,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/journal.json"), br#"{"backup":{"enabled":true,"mode":"byo","daily_key":"daily","recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ","destination":{"repository":"s3:repo","backend":"s3","credentials":{"access_key_id":"access","secret_access_key":"secret"}}}}"#).unwrap();
    let root = Arc::new(JournalRoot::open(dir.path()).unwrap());
    (dir, root, Arc::new(FixedClock), operation::new_slot())
}

fn closed(begin: OwnedArchiveBegin) -> (ArchiveTicket, ArchivePreparationCustody, ArchivePayload) {
    let (ticket, root, budget, clock) = begin.into_parts();
    let (input, custody) = OwnedArchivePreparationInput::capture(root, budget, clock)
        .unwrap_or_else(|_| panic!("actual original capture"));
    let owner = input
        .prepare()
        .unwrap_or_else(|_| panic!("actual original preparation"))
        .into_owned_hold();
    (ticket, custody, ArchivePayload::Prepared(Box::new(owner)))
}

#[cfg(unix)]
#[test]
fn unused_native_retirement_fault_retains_actual_custody_across_closed_drains() {
    use crate::{BackupRouteDrain, BackupWebDeps};
    use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};
    use std::time::Instant;
    for kind in ["prepared", "preparation_failure", "admission_failure"] {
        for fault in [
            None,
            Some(MediaPrimitive::CarrierRetirementRootSync),
            Some(MediaPrimitive::CarrierRetirementConfirmation),
        ] {
            let (dir, root, clock, slot) = fixture();
            let mut deps = BackupWebDeps::new(dir.path().to_path_buf());
            deps.operations = Arc::clone(&slot);
            let lifetime = deps.lifetime();
            let before = std::fs::read(dir.path().join("config/journal.json")).unwrap();
            let begin = begin_owned(
                &slot,
                "offload_restore",
                Arc::clone(&root),
                Arc::clone(&clock),
            )
            .unwrap();
            let worker_slot = Arc::clone(&slot);
            let worker_before = before.clone();
            let (ready, entered) = mpsc::channel();
            let (release, released) = mpsc::channel();
            // Register the actual admitted worker while holding its slot. The
            // drain, rather than this test caller, owns its eventual join.
            let mut state = slot.lock().unwrap();
            state.threads.push(thread::spawn(move || {
                let work = || {
                    if kind == "admission_failure" {
                        assert!(
                            prepare_read_in(
                                &worker_slot,
                                begin,
                                None,
                                Path::new("/must-not-open/restic")
                            )
                            .is_none()
                        );
                        return;
                    }
                    assert!(start_owned_worker(&worker_slot, &begin.ticket));
                    let (ticket, root, budget, clock) = begin.into_parts();
                    let (input, custody) =
                        OwnedArchivePreparationInput::capture(Arc::clone(&root), budget, clock)
                            .unwrap_or_else(|_| panic!("actual capture"));
                    let payload = if kind == "preparation_failure" {
                        std::fs::write(
                            root.canonical_path().join("config/journal.json"),
                            b"{broken",
                        )
                        .unwrap();
                        let failure = input
                            .prepare()
                            .err()
                            .expect("actual config preparation refusal");
                        std::fs::write(
                            root.canonical_path().join("config/journal.json"),
                            &worker_before,
                        )
                        .unwrap();
                        ArchivePayload::PreparationFailure(Box::new(failure))
                    } else {
                        ArchivePayload::Prepared(Box::new(
                            input
                                .prepare()
                                .unwrap_or_else(|_| panic!("actual preparation"))
                                .into_owned_hold(),
                        ))
                    };
                    return_owned(
                        &worker_slot,
                        ticket,
                        Some(custody),
                        payload,
                        Terminal::error("failed"),
                    );
                };
                let fired = if let Some(fault) = fault {
                    let (_, fired) = run_with_media_hook(
                        fault,
                        || Err(io::Error::other("unused retirement fault")),
                        work,
                    );
                    fired
                } else {
                    work();
                    false
                };
                ready.send(fired).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            }));
            drop(state);
            assert_eq!(
                entered.recv_timeout(Duration::from_secs(5)).unwrap(),
                fault.is_some(),
                "{kind} {fault:?}"
            );
            lifetime.request_stop();
            assert_eq!(
                lifetime.drain(crate::test_support::join_executor(), Instant::now()),
                BackupRouteDrain::Pending
            );
            assert_eq!(
                lifetime
                    .clone()
                    .drain(crate::test_support::join_executor(), Instant::now()),
                BackupRouteDrain::Pending
            );
            release.send(()).unwrap();
            let outcome = lifetime.drain(
                crate::test_support::join_executor(),
                Instant::now() + Duration::from_secs(5),
            );
            if fault.is_some() {
                assert_eq!(outcome, BackupRouteDrain::Pending, "{kind} {fault:?}");
                assert_eq!(
                    lifetime
                        .clone()
                        .drain(crate::test_support::join_executor(), Instant::now()),
                    BackupRouteDrain::Pending
                );
                let state = slot.lock().unwrap();
                assert!(
                    state.threads.is_empty(),
                    "actual worker must already have joined"
                );
                let retained = state
                    .archive
                    .retained
                    .first()
                    .expect("actual closed native owner retained");
                assert!(
                    retained
                        .payload
                        .matches(&retained.ticket.0, retained.custody.as_ref())
                );
                assert!(matches!(
                    retained.payload,
                    ArchivePayload::UnusedRetirement(_) | ArchivePayload::AdmissionFailure(_)
                ));
                assert!(matches!(
                    ActivityLease::try_acquire(Arc::clone(&root)),
                    Err(ActivityAdmissionError::Busy)
                ));
                assert!(matches!(
                    TeardownLease::try_acquire(Arc::clone(&root)),
                    Err(ActivityAdmissionError::Busy)
                ));
            } else {
                assert_eq!(outcome, BackupRouteDrain::Drained, "{kind}");
                assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_ok());
            }
            assert_eq!(
                std::fs::read(dir.path().join("config/journal.json")).unwrap(),
                before
            );
        }
    }
}

fn terminal() -> Terminal {
    Terminal {
        phase: "degraded".into(),
        reason_code: Some("held".into()),
        detail: Some("earlier effects".into()),
        guidance: Some("remaining work".into()),
        recording_failure: Some("recording".into()),
        report: Some(serde_json::json!({"files_restored":1,"bytes_restored":7})),
    }
}

fn blocked(slot: &SharedOperationSlot, root: &Arc<JournalRoot>, clock: &OwnedClock) {
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
}

#[test]
fn binding_rejected_projection_preserves_facts_and_other_handoff_views() {
    for case in [
        "current",
        "reservation",
        "stale",
        "missing",
        "inactive",
        "terminal",
        "poll",
        "pending",
        "poison",
        "other_refusal",
    ] {
        let (_dir, root, clock, slot) = fixture();
        let begin = begin_owned(
            &slot,
            "offload_restore",
            Arc::clone(&root),
            Arc::clone(&clock),
        )
        .unwrap();
        assert!(start_owned_worker(&slot, &begin.ticket));
        let (ticket, custody, payload) = closed(begin);
        let ArchivePayload::Prepared(owner) = &payload else {
            unreachable!()
        };
        let carrier = Arc::downgrade(owner.carrier());
        let poll = ArchiveCancellation::new();
        {
            let mut state = slot.lock().unwrap();
            match case {
                "reservation" => {
                    state.archive.reservation = Some(Arc::new(Reservation {
                        generation: ticket.generation(),
                        root: Arc::clone(&root),
                        clock: Arc::clone(&clock),
                    }));
                }
                "missing" => state.current = None,
                "stale" => state.current.as_mut().unwrap().generation += 1,
                "inactive" => state.current.as_mut().unwrap().worker_active = false,
                "terminal" => state.current.as_mut().unwrap().view.phase = "done".into(),
                "poll" => {
                    state.current.as_mut().unwrap().handoff_poll = Some(poll.clone());
                }
                "pending" => {
                    state.current.as_mut().unwrap().pending_handoff_terminal = Some(terminal());
                }
                _ => {}
            }
        }
        if case == "poison" {
            let poison = Arc::clone(&slot);
            assert!(
                thread::spawn(move || {
                    let _guard = poison.lock().unwrap();
                    panic!("projection poison control");
                })
                .join()
                .is_err()
            );
        }
        let before = {
            let state = slot.lock().unwrap_or_else(|poison| poison.into_inner());
            state.current.as_ref().map(|current| {
                (
                    current.view.clone(),
                    current.worker_active,
                    current.generation,
                    current.nonce.clone(),
                    current.restore_key.clone(),
                    current.started,
                    current.handoff_poll.is_some(),
                    current.pending_handoff_terminal.clone(),
                )
            })
        };
        let reason = if case == "other_refusal" {
            ArchiveHandoffRefusal::OwnerChanged
        } else {
            ArchiveHandoffRefusal::BindingChanged
        };
        // Only the private display envelope is synthetic. The whole Prepared
        // owner is real; genuine Completed/binding-loss credit is installed.
        let rejected = Box::new(RejectedArchiveReturn {
            ticket,
            payload,
            custody: Some(custody),
            binding_error: Some(io::Error::other("display envelope control")),
            terminal: terminal(),
            reason: Some(reason),
        });
        let worker_slot = Arc::clone(&slot);
        thread::spawn(move || park_rejected(&worker_slot, rejected))
            .join()
            .unwrap();
        blocked(&slot, &root, &clock);
        assert!(carrier.upgrade().is_some());
        assert!(matches!(
            ActivityLease::try_acquire(Arc::clone(&root)),
            Err(ActivityAdmissionError::Busy)
        ));
        let state = slot.lock().unwrap_or_else(|poison| poison.into_inner());
        assert_eq!(state.archive.retained.len(), 1, "{case}");
        let retained = &state.archive.retained[0];
        assert_eq!(retained.reason(), Some(reason));
        assert_eq!(retained.terminal(), &terminal());
        assert!(retained.custody().is_some());
        assert_eq!(
            retained.binding_error().unwrap().kind(),
            io::ErrorKind::Other
        );
        let ArchivePayload::Prepared(owner) = retained.payload() else {
            panic!("whole actual Prepared owner");
        };
        assert!(Arc::ptr_eq(owner.carrier(), &carrier.upgrade().unwrap()));
        let after = state.current.as_ref().map(|current| {
            (
                current.view.clone(),
                current.worker_active,
                current.generation,
                current.nonce.clone(),
                current.restore_key.clone(),
                current.started,
                current.handoff_poll.is_some(),
                current.pending_handoff_terminal.clone(),
            )
        });
        if case == "current" {
            let mut expected = before.unwrap();
            expected.0.phase = "error".into();
            expected.0.reason_code = Some("failed".into());
            expected.0.detail = terminal().detail;
            expected.0.guidance = terminal().guidance;
            expected.0.recording_failure = terminal().recording_failure;
            expected.0.report = terminal().report;
            expected.1 = false;
            assert_eq!(after, Some(expected));
        } else {
            assert_eq!(after, before, "{case}");
        }
        if case == "poll" {
            state
                .current
                .as_ref()
                .unwrap()
                .handoff_poll
                .as_ref()
                .unwrap()
                .cancel();
            assert!(poll.is_cancelled());
        }
        assert_eq!(slot.is_poisoned(), case == "poison");
    }
}

#[test]
fn real_worker_retains_closed_owner_after_matching_and_rejected_handoffs() {
    fn send_static<T: Send + 'static>() {}
    send_static::<offload::OffloadReturn>();
    for reason in [
        None,
        Some(ArchiveHandoffRefusal::Missing),
        Some(ArchiveHandoffRefusal::Stale),
        Some(ArchiveHandoffRefusal::Inactive),
        Some(ArchiveHandoffRefusal::Poisoned),
    ] {
        let (_dir, root, clock, slot) = fixture();
        let begin = begin_owned(
            &slot,
            "offload_restore",
            Arc::clone(&root),
            Arc::clone(&clock),
        )
        .unwrap();
        let generation = begin.generation();
        assert!(start_owned_worker(&slot, &begin.ticket));
        assert!(!operation::start_worker(&slot, generation));
        let (entered, observed) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker_slot = Arc::clone(&slot);
        let worker = thread::spawn(move || {
            let (ticket, custody, payload) = closed(begin);
            let ArchivePayload::Prepared(owner) = &payload else {
                unreachable!()
            };
            entered.send(Arc::downgrade(owner.carrier())).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            if let Err(rejected) = handoff(&worker_slot, ticket, Some(custody), payload, terminal())
            {
                // This is the actual worker caller, before thread return.
                park_rejected(&worker_slot, rejected);
            }
        });
        let carrier = observed.recv_timeout(Duration::from_secs(10)).unwrap();
        match reason {
            Some(ArchiveHandoffRefusal::Missing) => {
                slot.lock().unwrap().current = None;
            }
            Some(ArchiveHandoffRefusal::Stale) => {
                slot.lock().unwrap().current.as_mut().unwrap().generation += 1;
            }
            Some(ArchiveHandoffRefusal::Inactive) => {
                slot.lock().unwrap().current.as_mut().unwrap().worker_active = false;
            }
            Some(ArchiveHandoffRefusal::Poisoned) => {
                let poison = Arc::clone(&slot);
                assert!(
                    thread::spawn(move || {
                        let _guard = poison.lock().unwrap();
                        panic!("poison control");
                    })
                    .join()
                    .is_err()
                );
            }
            None => {}
            other => panic!("unexpected control {other:?}"),
        }
        blocked(&slot, &root, &clock);
        release.send(()).unwrap();
        worker.join().unwrap();
        blocked(&slot, &root, &clock);
        assert!(carrier.upgrade().is_some());
        assert!(matches!(
            ActivityLease::try_acquire(Arc::clone(&root)),
            Err(ActivityAdmissionError::Busy)
        ));
        assert!(matches!(
            TeardownLease::try_acquire(Arc::clone(&root)),
            Err(ActivityAdmissionError::Busy)
        ));
        let guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
        assert_eq!(guard.archive.retained.len(), 1);
        let retained = &guard.archive.retained[0];
        assert_eq!(retained.reason(), reason);
        assert_eq!(retained.generation(), generation);
        assert_eq!(retained.terminal(), &terminal());
        let ArchivePayload::Prepared(owner) = retained.payload() else {
            panic!("whole actual owner");
        };
        assert!(Arc::ptr_eq(owner.journal_root(), &root));
        assert!(Arc::ptr_eq(owner.owned_clock(), &clock));
        assert!(Arc::ptr_eq(owner.carrier(), &carrier.upgrade().unwrap()));
        if reason.is_none() {
            let current = guard.current.as_ref().unwrap();
            assert!(!current.worker_active);
            assert_eq!(current.view.phase, "restoring");
            assert_eq!(current.view.report, terminal().report);
        }
        drop(guard);
        assert_eq!(
            slot.is_poisoned(),
            reason == Some(ArchiveHandoffRefusal::Poisoned)
        );
    }
}

#[test]
fn live_reservation_blocks_absent_terminal_and_replacement_view_before_handoff() {
    let (_dir, root, clock, slot) = fixture();
    let begin = begin_owned(
        &slot,
        "offload_restore",
        Arc::clone(&root),
        Arc::clone(&clock),
    )
    .unwrap();
    assert!(start_owned_worker(&slot, &begin.ticket));
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker_slot = Arc::clone(&slot);
    let worker = thread::spawn(move || {
        let (ticket, custody, payload) = closed(begin);
        entered.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
        if let Err(refusal) = handoff(&worker_slot, ticket, Some(custody), payload, terminal()) {
            park_rejected(&worker_slot, refusal);
        }
    });
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut original = slot.lock().unwrap().current.take().unwrap();
    blocked(&slot, &root, &clock);
    original.view.phase = "done".into();
    slot.lock().unwrap().current = Some(original);
    blocked(&slot, &root, &clock);
    slot.lock().unwrap().current.as_mut().unwrap().generation += 1;
    blocked(&slot, &root, &clock);
    release.send(()).unwrap();
    worker.join().unwrap();
    blocked(&slot, &root, &clock);
    assert_eq!(
        slot.lock().unwrap().archive.retained[0].reason(),
        Some(ArchiveHandoffRefusal::Stale)
    );
}

#[test]
fn intact_original_begin_cancels_only_before_preparation_and_releases_reservation() {
    let (_dir, root, clock, slot) = fixture();
    let begin = begin_owned(
        &slot,
        "offload_restore",
        Arc::clone(&root),
        Arc::clone(&clock),
    )
    .unwrap();
    let generation = begin.generation();
    assert!(matches!(
        crate::archive_operation::request_cancellation(&slot, generation),
        crate::archive_operation::ArchiveCancellationRequest::Requested
    ));
    blocked(&slot, &root, &clock);
    assert!(!start_owned_worker(&slot, &begin.ticket));
    begin.cancel_before_preparation(&slot);
    assert!(!operation::is_busy(&slot));
    assert_eq!(
        operation::current(&slot).unwrap().reason_code.as_deref(),
        Some("cancelled")
    );
    assert!(
        solstone_core_journal_io::activity_obligations::observe_obligations(&root)
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(operation::begin(&slot, "restore", None, None, None).is_ok());
}

#[test]
fn refused_prepare_start_releases_unused_begin_without_native_capture() {
    let (dir, root, clock, slot) = fixture();
    let begin = begin_owned(
        &slot,
        "offload_restore",
        Arc::clone(&root),
        Arc::clone(&clock),
    )
    .unwrap();
    let generation = begin.generation();
    assert!(matches!(
        crate::archive_operation::request_cancellation(&slot, generation),
        crate::archive_operation::ArchiveCancellationRequest::Requested
    ));
    let worker_slot = Arc::clone(&slot);
    thread::spawn(move || {
        assert!(prepare_read(&worker_slot, begin, "unused".into(), Path::new("unused")).is_none());
    })
    .join()
    .unwrap();
    // The unused begin owned nothing native, so cancellation ends it at once.
    assert!(!dir.path().join("backup-activity.lock").exists());
    admitted_after(&slot, &root, dir.path());
}

/// Nothing of an effect-free return remains: the slot, lease and census are
/// free, a following operation starts and an owner deletion proceeds.
fn admitted_after(slot: &SharedOperationSlot, root: &Arc<JournalRoot>, journal: &Path) {
    assert!(!operation::is_busy(slot));
    assert!(slot.lock().unwrap().archive.retained.is_empty());
    assert!(
        solstone_core_journal_io::activity_obligations::observe_obligations(root)
            .unwrap()
            .entries()
            .is_empty()
    );
    drop(ActivityLease::try_acquire(Arc::clone(root)).unwrap());
    let target = solstone_core_retention::Target {
        day: "20261011".into(),
        stream: "audio".into(),
        dir: "070000_17".into(),
    };
    let segment = journal.join("chronicle/20261011/audio/070000_17");
    std::fs::create_dir_all(&segment).unwrap();
    std::fs::write(segment.join("audio.flac"), b"sole raw bytes").unwrap();
    let removed = solstone_core_retention::door::remove_segments(
        journal,
        std::slice::from_ref(&target),
        "2026-10-11T00:00:00Z",
        solstone_core_retention::RemovalReason::OwnerSegmentDelete,
        "fixture",
    );
    assert!(removed.targets[0].not_removed.is_empty(), "{removed:?}");
    assert!(!segment.join("audio.flac").exists());
    assert!(operation::begin(slot, "enable", None, None, None).is_ok());
}

#[test]
fn admission_failure_releases_its_reservation_and_admits_followers() {
    let (dir, root, clock, slot) = fixture();
    let begin = begin_owned(
        &slot,
        "offload_restore",
        Arc::clone(&root),
        Arc::clone(&clock),
    )
    .unwrap();
    let worker_slot = Arc::clone(&slot);
    thread::spawn(move || {
        assert!(
            prepare_read(
                &worker_slot,
                begin,
                "a".repeat(64),
                Path::new("/absent/restic")
            )
            .is_none()
        );
    })
    .join()
    .unwrap();
    let view = operation::current(&slot).unwrap();
    assert_eq!(view.phase, "error");
    assert_eq!(view.report.unwrap()["admission"]["state"], "refused");
    admitted_after(&slot, &root, dir.path());
}

#[test]
fn cancellation_after_reservation_before_any_effect_releases_and_admits_followers() {
    use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};
    let (dir, root, clock, slot) = fixture();
    let begin = begin_owned(
        &slot,
        "offload_restore",
        Arc::clone(&root),
        Arc::clone(&clock),
    )
    .unwrap();
    let generation = begin.generation();
    let worker_slot = Arc::clone(&slot);
    thread::spawn(move || {
        let cancel_slot = Arc::clone(&worker_slot);
        let (prepared, fired) = run_with_media_hook(
            MediaPrimitive::CarrierConfirmation,
            move || {
                assert!(matches!(
                    crate::archive_operation::request_cancellation(&cancel_slot, generation),
                    crate::archive_operation::ArchiveCancellationRequest::Requested
                ));
                Ok(())
            },
            || {
                prepare_read(
                    &worker_slot,
                    begin,
                    "a".repeat(64),
                    Path::new("/absent/restic"),
                )
            },
        );
        assert!(fired);
        assert!(prepared.is_none());
    })
    .join()
    .unwrap();
    let view = operation::current(&slot).unwrap();
    assert_eq!(view.phase, "error");
    assert_eq!(
        view.report.unwrap()["preparation"]["reason"],
        solstone_core_backup::archive_admission::AdmissionError::Cancelled.to_string()
    );
    admitted_after(&slot, &root, dir.path());
}
