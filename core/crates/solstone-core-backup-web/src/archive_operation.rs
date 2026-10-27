// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Original archive lifetime and negative cancellation transport.
//! Admission here starts no executor or HTTP request.

use solstone_core_backup::archive_admission::{AdmissionError, ArchiveBudget, Clock};

use crate::operation::{self, SharedOperationSlot};

/// An operation generation paired with its one owning archive budget.
pub struct ArchiveBegin {
    generation: u64,
    budget: ArchiveBudget,
}

impl ArchiveBegin {
    pub(crate) fn new(generation: u64, budget: ArchiveBudget) -> Self {
        Self { generation, budget }
    }

    /// Move this original budget into the consuming runtime admission.
    pub fn into_parts(self) -> (u64, ArchiveBudget) {
        (self.generation, self.budget)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveBeginRefusal {
    Busy,
    Admission(AdmissionError),
}

/// A stop request describes admission or signaling, never completed effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveCancellationRequest {
    Refused,
    BeforeStart,
    Requested,
}

/// Reserve the shared slot and create its original lower-owned archive budget.
pub fn begin(
    slot: &SharedOperationSlot,
    kind: &str,
    clock: &dyn Clock,
) -> Result<ArchiveBegin, ArchiveBeginRefusal> {
    operation::begin_archive(slot, kind, clock)
}

/// Signal only the selected archive generation. Active work settles itself.
pub fn request_cancellation(
    slot: &SharedOperationSlot,
    generation: u64,
) -> ArchiveCancellationRequest {
    operation::request_archive_cancellation(slot, generation)
}

#[cfg(test)]
#[cfg(not(windows))]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct TestClock {
        now: Arc<AtomicI64>,
        calls: Arc<AtomicUsize>,
    }

    impl TestClock {
        fn new(now: i64) -> Self {
            Self {
                now: Arc::new(AtomicI64::new(now)),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn set(&self, now: i64) {
            self.now.store(now, Ordering::SeqCst);
        }
    }

    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.now.load(Ordering::SeqCst)
        }

        fn iso_week(&self) -> u8 {
            1
        }
    }

    fn admitted(slot: &SharedOperationSlot, clock: &TestClock) -> (u64, ArchiveBudget) {
        begin(slot, "offload_approve", clock)
            .unwrap_or_else(|error| panic!("archive admission: {error:?}"))
            .into_parts()
    }

    #[test]
    fn admission_creates_one_original_budget_and_busy_does_not_observe_clock() {
        let slot = operation::new_slot();
        let clock = TestClock::new(10);
        let (generation, budget) = admitted(&slot, &clock);
        assert_eq!(clock.calls.load(Ordering::SeqCst), 1);
        assert_eq!(generation, 1);
        assert!(matches!(
            begin(&slot, "another", &clock),
            Err(ArchiveBeginRefusal::Busy)
        ));
        assert_eq!(clock.calls.load(Ordering::SeqCst), 1);
        clock.set(20);
        assert_eq!(
            budget.remaining(&clock).unwrap(),
            Duration::from_secs(21_590)
        );
        assert!(!budget.cancellation().unwrap().is_cancelled());
        assert_eq!(
            request_cancellation(&slot, generation),
            ArchiveCancellationRequest::BeforeStart
        );
        assert_eq!(budget.remaining(&clock), Err(AdmissionError::Cancelled));
        assert!(!operation::start_worker(&slot, generation));
    }

    #[test]
    fn lower_budget_refusal_preserves_observations_without_consuming_generation() {
        let slot = operation::new_slot();
        let clock = TestClock::new(i64::MAX);
        assert!(matches!(
            begin(&slot, "archive", &clock),
            Err(ArchiveBeginRefusal::Admission(
                AdmissionError::DeadlineExceeded
            ))
        ));
        assert!(slot.lock().unwrap().current.is_none());
        let generation = operation::begin(
            &slot,
            "restore_hosted",
            Some("portal".into()),
            Some("nonce".into()),
            Some("key".into()),
        )
        .unwrap()
        .generation;
        slot.lock().unwrap().current.as_mut().unwrap().started =
            Instant::now() - operation::HANDOFF_TTL - Duration::from_secs(1);
        assert!(matches!(
            begin(&slot, "archive", &clock),
            Err(ArchiveBeginRefusal::Admission(
                AdmissionError::DeadlineExceeded
            ))
        ));
        assert_eq!(operation::generation_of(&slot), Some(generation));
        let view = operation::current(&slot).unwrap();
        assert_eq!(view.reason_code.as_deref(), Some("expired"));
        assert!(view.portal_url.is_none());
        clock.set(10);
        let (next, _) = admitted(&slot, &clock);
        assert_eq!(next, generation + 1);
    }

    #[test]
    fn transferred_budget_preserves_expiry_credential_cap_and_clock_poisoning() {
        let clock = TestClock::new(100);
        let (_, mut budget) = admitted(&operation::new_slot(), &clock);
        clock.set(125);
        assert_eq!(
            budget.remaining(&clock).unwrap(),
            Duration::from_secs(21_575)
        );
        budget.cap_credential_expiry(140);
        budget.cap_credential_expiry(1_000);
        assert_eq!(budget.remaining(&clock).unwrap(), Duration::from_secs(15));
        clock.set(140);
        assert_eq!(
            budget.remaining(&clock),
            Err(AdmissionError::CredentialsExpired)
        );
        let (_, budget) = admitted(&operation::new_slot(), &clock);
        clock.set(139);
        assert_eq!(budget.remaining(&clock), Err(AdmissionError::ClockChanged));
        clock.set(141);
        assert_eq!(budget.remaining(&clock), Err(AdmissionError::ClockChanged));
        let (_, budget) = admitted(&operation::new_slot(), &clock);
        clock.set(141 + 21_600);
        assert_eq!(
            budget.remaining(&clock),
            Err(AdmissionError::DeadlineExceeded)
        );
    }

    #[test]
    fn cancellation_refuses_missing_stale_ordinary_and_terminal_generations() {
        let slot = operation::new_slot();
        assert_eq!(
            request_cancellation(&slot, 1),
            ArchiveCancellationRequest::Refused
        );
        let ordinary = operation::begin(&slot, "enable", None, None, None)
            .unwrap()
            .generation;
        let original = operation::current(&slot);
        assert_eq!(
            request_cancellation(&slot, ordinary),
            ArchiveCancellationRequest::Refused
        );
        assert_eq!(operation::current(&slot), original);
        assert!(operation::mark_cancelled(&slot, ordinary));
        let clock = TestClock::new(10);
        let (generation, budget) = admitted(&slot, &clock);
        let original = operation::current(&slot);
        assert_eq!(
            request_cancellation(&slot, ordinary),
            ArchiveCancellationRequest::Refused
        );
        assert_eq!(operation::current(&slot), original);
        assert!(!budget.cancellation().unwrap().is_cancelled());
        assert_eq!(
            request_cancellation(&slot, generation),
            ArchiveCancellationRequest::BeforeStart
        );
        assert_eq!(
            request_cancellation(&slot, generation),
            ArchiveCancellationRequest::Refused
        );
        let (next, next_budget) = admitted(&slot, &clock);
        assert_eq!(
            request_cancellation(&slot, generation),
            ArchiveCancellationRequest::Refused
        );
        assert!(!next_budget.cancellation().unwrap().is_cancelled());
        assert!(budget.cancellation().unwrap().is_cancelled());
        assert_eq!(
            request_cancellation(&slot, next),
            ArchiveCancellationRequest::BeforeStart
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod worker_tests {
        use super::*;
        use std::sync::mpsc;
        use std::thread::JoinHandle;

        struct OwnedWorker {
            release: Option<mpsc::Sender<()>>,
            handle: Option<JoinHandle<()>>,
        }

        impl OwnedWorker {
            fn settle(mut self) {
                self.release.take().unwrap().send(()).unwrap();
                self.handle.take().unwrap().join().unwrap();
            }
        }

        impl Drop for OwnedWorker {
            fn drop(&mut self) {
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
            }
        }

        fn cancellation_preserves_terminal(terminal: operation::Terminal) {
            let slot = operation::new_slot();
            let clock = TestClock::new(100);
            let (generation, budget) = admitted(&slot, &clock);
            let old_flag = budget.cancellation().unwrap().clone();
            clock.set(110);
            let worker_clock = clock.clone();
            let (entered, observed) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let (stopped, stop_observed) = mpsc::channel();
            let expected = terminal.clone();
            assert!(operation::spawn_worker(
                slot.clone(),
                generation,
                move || {
                    entered.send(budget.remaining(&worker_clock)).unwrap();
                    released.recv_timeout(Duration::from_secs(5)).unwrap();
                    stopped.send(budget.remaining(&worker_clock)).unwrap();
                    terminal
                }
            ));
            let handle = slot.lock().unwrap().threads.pop().unwrap();
            let worker = OwnedWorker {
                release: Some(release),
                handle: Some(handle),
            };
            assert_eq!(
                observed
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap(),
                Duration::from_secs(21_590)
            );
            for _ in 0..2 {
                assert_eq!(
                    request_cancellation(&slot, generation),
                    ArchiveCancellationRequest::Requested
                );
            }
            assert!(old_flag.is_cancelled());
            assert_eq!(operation::current(&slot).unwrap().phase, "cancelling");
            assert!(operation::is_busy(&slot));
            assert!(matches!(
                begin(&slot, "second", &clock),
                Err(ArchiveBeginRefusal::Busy)
            ));
            assert!(!operation::start_worker(&slot, generation));
            assert!(!operation::mark_cancelled(&slot, generation));
            operation::mark_expired(&slot, generation);
            operation::finish(&slot, generation, "done", None, None);
            assert_eq!(operation::current(&slot).unwrap().phase, "cancelling");
            worker.settle();
            assert_eq!(
                stop_observed.recv_timeout(Duration::from_secs(5)).unwrap(),
                Err(AdmissionError::Cancelled)
            );
            let actual = operation::current(&slot).unwrap();
            assert_eq!(actual.phase, expected.phase);
            assert_eq!(actual.reason_code, expected.reason_code);
            assert_eq!(actual.detail, expected.detail);
            assert_eq!(actual.guidance, expected.guidance);
            assert_eq!(actual.recording_failure, expected.recording_failure);
            assert_eq!(actual.report, expected.report);
            assert!(!operation::is_busy(&slot));
            let (next, next_budget) = admitted(&slot, &clock);
            assert_eq!(
                request_cancellation(&slot, generation),
                ArchiveCancellationRequest::Refused
            );
            operation::finish(&slot, generation, "error", Some("stale".into()), None);
            assert_eq!(operation::generation_of(&slot), Some(next));
            assert!(!next_budget.cancellation().unwrap().is_cancelled());
            assert!(old_flag.is_cancelled());
            assert_eq!(
                request_cancellation(&slot, next),
                ArchiveCancellationRequest::BeforeStart
            );
        }

        #[test]
        fn active_cancellation_preserves_complete_partial_terminal() {
            cancellation_preserves_terminal(operation::Terminal {
                phase: "degraded".into(),
                reason_code: Some("later_target_refused".into()),
                detail: Some("earlier effects retained".into()),
                guidance: Some("review remaining targets".into()),
                recording_failure: Some("recording failed".into()),
                report: Some(serde_json::json!({"files_restored":1, "bytes_restored":7})),
            });
        }

        #[test]
        fn cancellation_after_completed_effects_preserves_success() {
            cancellation_preserves_terminal(operation::Terminal::done());
        }

        #[test]
        fn queued_cancellation_prevents_actual_worker_start() {
            let slot = operation::new_slot();
            let clock = TestClock::new(100);
            let (generation, budget) = admitted(&slot, &clock);
            assert_eq!(
                request_cancellation(&slot, generation),
                ArchiveCancellationRequest::BeforeStart
            );
            let (entered, observed) = mpsc::channel();
            assert!(!operation::spawn_worker(
                slot.clone(),
                generation,
                move || {
                    entered.send(budget.remaining(&clock)).unwrap();
                    operation::Terminal::done()
                }
            ));
            assert_eq!(
                observed.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            );
            assert!(!operation::is_busy(&slot));
        }

        #[test]
        fn panicked_archive_worker_settles_before_successor_admission() {
            let slot = operation::new_slot();
            let clock = TestClock::new(100);
            let (generation, budget) = admitted(&slot, &clock);
            let old_flag = budget.cancellation().unwrap().clone();
            assert!(operation::spawn_worker(
                slot.clone(),
                generation,
                move || {
                    let _budget = budget;
                    panic!("archive worker control")
                }
            ));
            let handle = slot.lock().unwrap().threads.pop().unwrap();
            handle.join().unwrap();
            assert_eq!(
                operation::current(&slot).unwrap().reason_code.as_deref(),
                Some("failed")
            );
            let (next, next_budget) = admitted(&slot, &clock);
            old_flag.cancel();
            assert!(!next_budget.cancellation().unwrap().is_cancelled());
            assert_eq!(
                request_cancellation(&slot, next),
                ArchiveCancellationRequest::BeforeStart
            );
        }
    }
}
