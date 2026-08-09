// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Single-provider reconciliation in the same ordered phases as the supervisor.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::events::{ProviderRuntimeEvent, ProviderRuntimeEventSink};
use super::gate::ProviderStartupGate;
use super::model::{
    InFlight, LaunchOutcomeStatus, ManagedProcess, PROVIDER_PROBE_INTERVAL_SECONDS,
    PROVIDER_START_CANCEL_PHASES, PROVIDER_STARTUP_TERMINAL_PHASES,
    PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS, PROVIDER_TRUTH_PRESERVED_PHASES, ProbeStatus,
    ProviderFence, ProviderRuntimeNow, ProviderRuntimeState, ProviderTruthObservation,
    RuntimePhase, StopCleanupStatus, phase_in,
};
use super::retry::{retry_token_phase, schedule_cleanup_retry, schedule_launch_retry};
use super::seams::{
    LifecycleSeam, ProbeSeam, RuntimeStore, RuntimeStoreError, TruthObservationSeam, reset_retry,
};
use super::stop::{
    duplicate_owned_process_request, make_stop_request, stop_before_replace_request,
};

static NEXT_INCARNATION: AtomicU64 = AtomicU64::new(1);

/// Owns the process-wide incarnation used by all provider fences in this process.
#[derive(Debug, Clone)]
pub struct ProviderRuntimeCoordinator {
    incarnation: String,
}

impl ProviderRuntimeCoordinator {
    pub fn new() -> Self {
        let sequence = NEXT_INCARNATION.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Self {
            incarnation: format!("{nanos:x}-{sequence:x}"),
        }
    }

    #[cfg(test)]
    fn with_incarnation(incarnation: &str) -> Self {
        Self {
            incarnation: incarnation.to_owned(),
        }
    }

    pub fn reconcile(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        processes: &mut Vec<ManagedProcess>,
        truth: &mut dyn TruthObservationSeam,
        lifecycle: &mut dyn LifecycleSeam,
        probe: &mut dyn ProbeSeam,
        store: &mut dyn RuntimeStore,
        sink: &mut dyn ProviderRuntimeEventSink,
        mut gate: Option<&mut ProviderStartupGate>,
    ) {
        sink.emit(ProviderRuntimeEvent::Step("handle-retry-token"));
        self.handle_retry_token(now, state, store, sink);

        sink.emit(ProviderRuntimeEvent::Step("handle-truth-result"));
        self.handle_truth_result(now, state, store, sink, gate.as_deref_mut());

        sink.emit(ProviderRuntimeEvent::Step("handle-start-result"));
        self.handle_start_result(now, state, processes, store, sink, gate.as_deref_mut());

        sink.emit(ProviderRuntimeEvent::Step("handle-stop-cleanup-result"));
        let stop_result_handled = self.handle_stop_cleanup_result(
            now,
            state,
            processes,
            store,
            sink,
            gate.as_deref_mut(),
        );

        sink.emit(ProviderRuntimeEvent::Step("handle-probe-result"));
        self.handle_probe_result(now, state, store, sink, gate.as_deref_mut());

        if !stop_result_handled {
            sink.emit(ProviderRuntimeEvent::Step("submit-stop-cleanup-if-needed"));
            self.submit_stop_cleanup_if_needed(now, state, processes, lifecycle, sink);

            sink.emit(ProviderRuntimeEvent::Step("submit-start-if-needed"));
            self.submit_start_if_needed(
                now,
                state,
                processes,
                lifecycle,
                sink,
                gate.as_deref_mut(),
            );
        }

        sink.emit(ProviderRuntimeEvent::Step("submit-probe-if-needed"));
        self.submit_probe_if_needed(now, state, probe, sink);

        sink.emit(ProviderRuntimeEvent::Step("submit-truth-if-needed"));
        self.submit_truth_if_needed(now, state, truth, sink);

        if let Some(gate) = gate.as_deref_mut() {
            gate.release_if_ready(now, sink);
        }
    }

    fn fence(&self, state: &ProviderRuntimeState, attempt: u32) -> ProviderFence {
        ProviderFence {
            incarnation: self.incarnation.clone(),
            generation: state.generation,
            fingerprint: state.desired_fingerprint.clone(),
            attempt,
        }
    }

    fn fence_matches(
        &self,
        state: &ProviderRuntimeState,
        fence: &ProviderFence,
        expected_attempt: u32,
    ) -> bool {
        fence.incarnation == self.incarnation
            && fence.generation == state.generation
            && fence.fingerprint == state.desired_fingerprint
            && fence.attempt == expected_attempt
    }

    fn handle_retry_token(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        store: &mut dyn RuntimeStore,
        sink: &mut dyn ProviderRuntimeEventSink,
    ) {
        let token = match store.read_retry_token(state.provider) {
            Ok(token) => token,
            Err(error) => {
                state.latest_phase = store_error_phase(error);
                return;
            }
        };
        let Some(token) = token else { return };
        if token.desired_fingerprint != state.desired_fingerprint {
            return;
        }
        state.latest_phase = retry_token_phase(state.latest_phase);
        state.next_truth_at = now.monotonic_seconds;
        reset_retry(state);
        if store
            .consume_retry_token(state.provider, &token.token_id)
            .is_err()
        {
            state.latest_phase = RuntimePhase::StateUnavailable;
        }
        self.persist(state, store);
        let _ = sink;
    }

    fn handle_truth_result(
        &self,
        _now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        store: &mut dyn RuntimeStore,
        sink: &mut dyn ProviderRuntimeEventSink,
        gate: Option<&mut ProviderStartupGate>,
    ) {
        let Some(in_flight) = state.truth.take() else {
            return;
        };
        let Some(result) = in_flight.result else {
            state.truth = Some(in_flight);
            return;
        };
        if !self.fence_matches(state, &in_flight.fence, state.retry.attempt_count) {
            sink.emit(ProviderRuntimeEvent::StaleResultDiscarded {
                operation: "truth",
                provider: state.provider,
            });
            return;
        }
        self.apply_truth(state, result, gate);
        self.persist(state, store);
    }

    fn apply_truth(
        &self,
        state: &mut ProviderRuntimeState,
        result: ProviderTruthObservation,
        gate: Option<&mut ProviderStartupGate>,
    ) {
        if state.desired_fingerprint != result.desired_fingerprint {
            state.generation += 1;
            state.desired_fingerprint = result.desired_fingerprint.clone();
            reset_retry(state);
        }
        state.has_plan = result.has_plan;
        state.boot_required = result.boot_required;
        state.latest_phase = result.phase;
        self.note_terminal(state, gate);
    }

    fn handle_start_result(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        processes: &mut Vec<ManagedProcess>,
        store: &mut dyn RuntimeStore,
        sink: &mut dyn ProviderRuntimeEventSink,
        mut gate: Option<&mut ProviderStartupGate>,
    ) {
        let Some(in_flight) = state.start.take() else {
            return;
        };
        let Some(result) = in_flight.result else {
            state.start = Some(in_flight);
            return;
        };
        if !self.fence_matches(state, &in_flight.fence, state.retry.attempt_count) {
            sink.emit(ProviderRuntimeEvent::StaleResultDiscarded {
                operation: "start",
                provider: state.provider,
            });
            return;
        }
        if state.start_cancelled || phase_in(&PROVIDER_START_CANCEL_PHASES, state.latest_phase) {
            state.start_cancelled = false;
            return;
        }
        if let Some(gate) = gate.as_deref_mut() {
            gate.on_start_result(state.provider, result.status);
        }
        match result.status {
            LaunchOutcomeStatus::Ready => {
                if let Some(managed) = result.managed {
                    processes.push(managed);
                }
                state.latest_phase = RuntimePhase::Ready;
                self.note_terminal(state, gate.as_deref_mut());
            }
            LaunchOutcomeStatus::HostBlocked => {
                state.latest_phase = RuntimePhase::HostBlocked;
                self.note_terminal(state, gate.as_deref_mut());
            }
            _ => {
                schedule_launch_retry(state, now, sink);
                self.note_terminal(state, gate.as_deref_mut());
            }
        }
        self.persist(state, store);
    }

    fn handle_stop_cleanup_result(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        processes: &mut Vec<ManagedProcess>,
        store: &mut dyn RuntimeStore,
        sink: &mut dyn ProviderRuntimeEventSink,
        gate: Option<&mut ProviderStartupGate>,
    ) -> bool {
        let Some(in_flight) = state.stop_cleanup.take() else {
            return false;
        };
        let Some(result) = in_flight.result else {
            state.stop_cleanup = Some(in_flight);
            return false;
        };
        if !self.fence_matches(state, &in_flight.fence, state.cleanup_attempt_count) {
            sink.emit(ProviderRuntimeEvent::StaleResultDiscarded {
                operation: "stop-cleanup",
                provider: state.provider,
            });
            return true;
        }
        if state.stop_cancelled || result.status == StopCleanupStatus::Cancelled {
            state.stop_cancelled = false;
            return true;
        }
        match result.status {
            StopCleanupStatus::Stopped => {
                if let Some(request) = state.pending_stop_request.take() {
                    processes.retain(|process| process.id != request.managed.id);
                    state.latest_phase = request.target_phase;
                    state.pending_stop_target_reason_code = request.target_reason_code;
                } else {
                    state.latest_phase = RuntimePhase::Stopped;
                }
                state.cleanup_attempt_count = 0;
                self.note_terminal(state, gate);
            }
            StopCleanupStatus::StopDeferred => {
                state.latest_phase = RuntimePhase::StopDeferred;
                sink.emit(ProviderRuntimeEvent::StopDeferred {
                    provider: state.provider,
                });
            }
            StopCleanupStatus::CleanupFailed => {
                state.pending_stop_target_reason_code = Some(result.reason_code);
                schedule_cleanup_retry(state, now, sink);
            }
            StopCleanupStatus::Cancelled => unreachable!("handled above"),
        }
        self.persist(state, store);
        true
    }

    fn handle_probe_result(
        &self,
        _now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        store: &mut dyn RuntimeStore,
        sink: &mut dyn ProviderRuntimeEventSink,
        gate: Option<&mut ProviderStartupGate>,
    ) {
        let Some(in_flight) = state.probe.take() else {
            return;
        };
        let Some(result) = in_flight.result else {
            state.probe = Some(in_flight);
            return;
        };
        if !self.fence_matches(state, &in_flight.fence, state.retry.attempt_count) {
            sink.emit(ProviderRuntimeEvent::StaleResultDiscarded {
                operation: "probe",
                provider: state.provider,
            });
            return;
        }
        state.latest_phase = match result.status {
            ProbeStatus::Ready => RuntimePhase::Ready,
            ProbeStatus::NotReady | ProbeStatus::Unavailable => RuntimePhase::ReadyProofUnavailable,
        };
        self.note_terminal(state, gate);
        self.persist(state, store);
    }

    fn submit_stop_cleanup_if_needed(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        processes: &[ManagedProcess],
        lifecycle: &mut dyn LifecycleSeam,
        sink: &mut dyn ProviderRuntimeEventSink,
    ) {
        if state.stop_cleanup.is_some() || now.monotonic_seconds < state.cleanup_next_at {
            return;
        }
        let request = if let Some(request) = state.pending_stop_request.clone() {
            Some(request)
        } else if let Some(request) = duplicate_owned_process_request(state, processes) {
            Some(request)
        } else if state.latest_phase == RuntimePhase::StopDeferred {
            processes
                .iter()
                .find(|process| process.running)
                .cloned()
                .map(|managed| {
                    make_stop_request(
                        state,
                        managed,
                        super::model::ReasonCode::known("intent-removed"),
                        state.pending_stop_target_phase,
                        state.pending_stop_target_reason_code.clone(),
                        state.pending_stop_admission_exclusive,
                    )
                })
        } else {
            stop_before_replace_request(state, processes)
        };
        let Some(request) = request else { return };
        state.pending_stop_request = Some(request);
        state.latest_phase = RuntimePhase::Stopping;
        let fence = self.fence(state, state.cleanup_attempt_count);
        state.stop_cleanup = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        lifecycle.dispatch_stop(state, &fence);
        sink.emit(ProviderRuntimeEvent::Dispatched {
            operation: "stop-cleanup",
            provider: state.provider,
        });
    }

    fn submit_start_if_needed(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        processes: &[ManagedProcess],
        lifecycle: &mut dyn LifecycleSeam,
        sink: &mut dyn ProviderRuntimeEventSink,
        gate: Option<&mut ProviderStartupGate>,
    ) {
        if state.start.is_some()
            || state.stop_cleanup.is_some()
            || !state.has_plan
            || processes.iter().any(|process| process.running)
            || !matches!(
                state.latest_phase,
                RuntimePhase::Starting
                    | RuntimePhase::Backoff
                    | RuntimePhase::RetryRequested
                    | RuntimePhase::Stopped
            )
            || now.monotonic_seconds < state.retry.next_at
        {
            return;
        }
        if state.retry.attempt_count as usize >= super::model::PROVIDER_RETRY_SCHEDULE_SECONDS.len()
        {
            state.latest_phase = RuntimePhase::Failed;
            self.note_terminal(state, gate);
            return;
        }
        state.retry.attempt_count += 1;
        state.latest_phase = RuntimePhase::Starting;
        let fence = self.fence(state, state.retry.attempt_count);
        state.start = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        if let Some(gate) = gate {
            gate.on_start_submitted(state.provider, now);
        }
        lifecycle.dispatch_start(state, &fence);
        sink.emit(ProviderRuntimeEvent::Dispatched {
            operation: "start",
            provider: state.provider,
        });
    }

    fn submit_probe_if_needed(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        probe: &mut dyn ProbeSeam,
        sink: &mut dyn ProviderRuntimeEventSink,
    ) {
        if state.probe.is_some()
            || now.monotonic_seconds < state.next_probe_at
            || !matches!(
                state.latest_phase,
                RuntimePhase::Ready | RuntimePhase::ReadyProofUnavailable
            )
        {
            return;
        }
        state.next_probe_at = now.monotonic_seconds + PROVIDER_PROBE_INTERVAL_SECONDS;
        let fence = self.fence(state, state.retry.attempt_count);
        state.probe = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        probe.dispatch_probe(state, &fence);
        sink.emit(ProviderRuntimeEvent::Dispatched {
            operation: "probe",
            provider: state.provider,
        });
    }

    fn submit_truth_if_needed(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        truth: &mut dyn TruthObservationSeam,
        sink: &mut dyn ProviderRuntimeEventSink,
    ) {
        if state.truth.is_some() || now.monotonic_seconds < state.next_truth_at {
            return;
        }
        state.next_truth_at = now.monotonic_seconds + PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS;
        if !phase_in(&PROVIDER_TRUTH_PRESERVED_PHASES, state.latest_phase) {
            state.latest_phase = RuntimePhase::Observing;
        }
        let fence = self.fence(state, state.retry.attempt_count);
        state.truth = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        truth.dispatch_truth(state, &fence);
        sink.emit(ProviderRuntimeEvent::Dispatched {
            operation: "truth",
            provider: state.provider,
        });
    }

    fn note_terminal(
        &self,
        state: &mut ProviderRuntimeState,
        gate: Option<&mut ProviderStartupGate>,
    ) {
        state.startup_terminal = phase_in(&PROVIDER_STARTUP_TERMINAL_PHASES, state.latest_phase);
        if let Some(gate) = gate {
            gate.on_phase(state.provider, state.latest_phase);
        }
    }

    fn persist(&self, state: &mut ProviderRuntimeState, store: &mut dyn RuntimeStore) {
        if let Err(error) = store.publish_state(state) {
            state.latest_phase = store_error_phase(error);
        }
    }
}

impl Default for ProviderRuntimeCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

fn store_error_phase(error: RuntimeStoreError) -> RuntimePhase {
    match error {
        RuntimeStoreError::Corrupt => RuntimePhase::StateCorrupt,
        RuntimeStoreError::Unavailable | RuntimeStoreError::Conflict => {
            RuntimePhase::StateUnavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::provider_runtime::{
        CortexEventKind, CortexOutcomeEvent, GATE_TICK_INTERVAL_SECONDS, InMemoryRuntimeStore,
        KNOWN_REASON_CODES, LOCAL_WEDGE_PROVIDER_MAP_CAP, LOCAL_WEDGE_RECYCLE_GRACE_SECONDS,
        LOCAL_WEDGE_THRESHOLD, PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS,
        PROVIDER_RETRY_SCHEDULE_SECONDS, ProviderLaunchOutcome, ProviderName, ProviderProbeOutcome,
        ProviderStopCleanupOutcome, ReasonCode, RetryToken, VecEventSink, WedgeState, cancel_start,
        cancel_stop, defer_target_stop, duplicate_owned_process_request, schedule_cleanup_retry,
        schedule_launch_retry,
    };

    #[derive(Clone, Default)]
    struct RecordingWorkers {
        calls: Rc<RefCell<Vec<&'static str>>>,
    }

    impl TruthObservationSeam for RecordingWorkers {
        fn dispatch_truth(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("truth-dispatch");
        }
    }

    impl LifecycleSeam for RecordingWorkers {
        fn dispatch_start(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("start-dispatch");
        }

        fn dispatch_stop(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("stop-dispatch");
        }
    }

    impl ProbeSeam for RecordingWorkers {
        fn dispatch_probe(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("probe-dispatch");
        }
    }

    fn now(seconds: f64) -> ProviderRuntimeNow {
        ProviderRuntimeNow {
            monotonic_seconds: seconds,
        }
    }

    fn managed(id: &str) -> ManagedProcess {
        ManagedProcess {
            id: id.to_owned(),
            name: "provider".to_owned(),
            running: true,
        }
    }

    fn state_for(provider: ProviderName) -> ProviderRuntimeState {
        let mut state = ProviderRuntimeState::new(provider);
        state.desired_fingerprint = Some("desired-a".to_owned());
        state.has_plan = true;
        state
    }

    fn launch(status: LaunchOutcomeStatus) -> ProviderLaunchOutcome {
        ProviderLaunchOutcome {
            status,
            reason_code: ReasonCode::known("launch-failed"),
            managed: (status == LaunchOutcomeStatus::Ready).then(|| managed("started")),
        }
    }

    fn stop(status: StopCleanupStatus) -> ProviderStopCleanupOutcome {
        ProviderStopCleanupOutcome {
            status,
            reason_code: ReasonCode::known("cleanup-attempt-failed"),
            managed: None,
        }
    }

    fn probe(status: ProbeStatus) -> ProviderProbeOutcome {
        ProviderProbeOutcome {
            status,
            reason_code: ReasonCode::known("probe-not-ready"),
        }
    }

    #[test]
    fn all_runtime_phases_are_representable() {
        assert_eq!(RuntimePhase::ALL.len(), 17);
        assert_eq!(
            RuntimePhase::ALL
                .iter()
                .map(|phase| phase.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            17
        );
    }

    #[test]
    fn startup_terminal_phases_exact_membership() {
        assert_eq!(
            PROVIDER_STARTUP_TERMINAL_PHASES
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                RuntimePhase::Ready,
                RuntimePhase::ReadyProofUnavailable,
                RuntimePhase::NotDesired,
                RuntimePhase::ArtifactNotReady,
                RuntimePhase::HostBlocked,
                RuntimePhase::Failed,
                RuntimePhase::StateCorrupt,
                RuntimePhase::StateUnavailable,
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn start_cancel_phases_exact_membership() {
        assert_eq!(
            PROVIDER_START_CANCEL_PHASES
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                RuntimePhase::NotDesired,
                RuntimePhase::StateCorrupt,
                RuntimePhase::StateUnavailable
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn truth_preserved_phases_exact_membership() {
        assert_eq!(
            PROVIDER_TRUTH_PRESERVED_PHASES
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                RuntimePhase::Ready,
                RuntimePhase::ReadyProofUnavailable,
                RuntimePhase::StopDeferred,
                RuntimePhase::Stopping,
                RuntimePhase::CleanupFailed,
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn unclassified_phases_are_exact_and_documented() {
        let unclassified = RuntimePhase::ALL
            .into_iter()
            .filter(|phase| {
                !phase_in(&PROVIDER_STARTUP_TERMINAL_PHASES, *phase)
                    && !phase_in(&PROVIDER_START_CANCEL_PHASES, *phase)
                    && !phase_in(&PROVIDER_TRUTH_PRESERVED_PHASES, *phase)
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            unclassified,
            [
                RuntimePhase::Observing,
                RuntimePhase::Starting,
                RuntimePhase::Warming,
                RuntimePhase::Backoff,
                RuntimePhase::RetryRequested,
                RuntimePhase::Stopped,
            ]
            .into_iter()
            .collect()
        );
        for phase in unclassified {
            assert!(super::super::model::unclassified_phase_reason(phase).is_some());
        }
    }

    #[test]
    fn all_42_reason_codes_recognized() {
        assert_eq!(KNOWN_REASON_CODES.len(), 42);
        assert!(
            KNOWN_REASON_CODES
                .into_iter()
                .all(|code| ReasonCode::from_wire(code).is_recognized())
        );
        assert!(ReasonCode::from_wire("openmp-runtime-unavailable").is_recognized());
    }

    #[test]
    fn unrecognized_reason_code_round_trips_not_rejected() {
        let code = ReasonCode::from_wire("future-runtime-reason");
        assert_eq!(code.as_str(), "future-runtime-reason");
        assert!(!code.is_recognized());
    }

    #[test]
    fn desired_not_running_starts() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        state.next_truth_at = 100.0;
        let workers = RecordingWorkers::default();
        let mut truth = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut truth,
            &mut lifecycle,
            &mut probe,
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(*workers.calls.borrow(), ["start-dispatch"]);
    }

    #[test]
    fn not_desired_running_stops() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        defer_target_stop(&mut state, RuntimePhase::NotDesired, None, false);
        state.next_truth_at = 100.0;
        let mut processes = vec![managed("old")];
        let workers = RecordingWorkers::default();
        let mut truth = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(0.0),
            &mut state,
            &mut processes,
            &mut truth,
            &mut lifecycle,
            &mut probe,
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(*workers.calls.borrow(), ["stop-dispatch"]);
    }

    #[test]
    fn desired_and_ready_is_noop() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        state.next_truth_at = 100.0;
        state.next_probe_at = 100.0;
        let workers = RecordingWorkers::default();
        let mut truth = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(0.0),
            &mut state,
            &mut vec![managed("ready")],
            &mut truth,
            &mut lifecycle,
            &mut probe,
            &mut store,
            &mut sink,
            None,
        );
        assert!(workers.calls.borrow().is_empty());
    }

    #[test]
    fn probe_unavailable_distinct_from_not_ready_and_ready() {
        assert_ne!(ProbeStatus::Unavailable, ProbeStatus::NotReady);
        assert_ne!(ProbeStatus::Unavailable, ProbeStatus::Ready);
        assert_ne!(ProbeStatus::NotReady, ProbeStatus::Ready);
    }

    #[test]
    fn stop_cleanup_deferred_and_failed_distinct_from_stopped() {
        assert_ne!(StopCleanupStatus::StopDeferred, StopCleanupStatus::Stopped);
        assert_ne!(StopCleanupStatus::CleanupFailed, StopCleanupStatus::Stopped);
        assert_ne!(
            StopCleanupStatus::StopDeferred,
            StopCleanupStatus::CleanupFailed
        );
    }

    #[test]
    fn launch_warmup_timeout_distinct_from_launch_failed_and_ready() {
        assert_ne!(
            LaunchOutcomeStatus::WarmupTimeout,
            LaunchOutcomeStatus::LaunchFailed
        );
        assert_ne!(
            LaunchOutcomeStatus::WarmupTimeout,
            LaunchOutcomeStatus::Ready
        );
        assert_ne!(
            LaunchOutcomeStatus::LaunchFailed,
            LaunchOutcomeStatus::Ready
        );
    }

    #[test]
    fn reconcile_pass_idempotent_and_follows_exact_order() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        let workers = RecordingWorkers::default();
        let mut truth = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut truth,
            &mut lifecycle,
            &mut probe,
            &mut store,
            &mut sink,
            None,
        );
        coordinator.reconcile(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut truth,
            &mut lifecycle,
            &mut probe,
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(
            workers
                .calls
                .borrow()
                .iter()
                .filter(|call| **call == "start-dispatch")
                .count(),
            1
        );
        assert_eq!(
            sink.events
                .iter()
                .filter_map(|event| match event {
                    ProviderRuntimeEvent::Step(step) => Some(*step),
                    _ => None,
                })
                .take(9)
                .collect::<Vec<_>>(),
            [
                "handle-retry-token",
                "handle-truth-result",
                "handle-start-result",
                "handle-stop-cleanup-result",
                "handle-probe-result",
                "submit-stop-cleanup-if-needed",
                "submit-start-if-needed",
                "submit-probe-if-needed",
                "submit-truth-if-needed",
            ]
        );
    }

    fn stale_truth_fixture() -> (
        ProviderRuntimeCoordinator,
        ProviderRuntimeState,
        InFlight<ProviderTruthObservation>,
    ) {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let state = state_for(ProviderName::Local);
        let fence = coordinator.fence(&state, 0);
        let result = ProviderTruthObservation {
            provider: ProviderName::Local,
            phase: RuntimePhase::Ready,
            reason_code: None,
            desired_fingerprint: state.desired_fingerprint.clone(),
            has_plan: true,
            boot_required: false,
        };
        (
            coordinator,
            state,
            InFlight {
                fence,
                result: Some(result),
            },
        )
    }

    #[test]
    fn stale_truth_fence_discards_result() {
        let (coordinator, mut state, in_flight) = stale_truth_fixture();
        state.generation += 1;
        state.truth = Some(in_flight);
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(0.0), &mut state, &mut store, &mut sink, None);
        assert_ne!(state.latest_phase, RuntimePhase::Ready);
    }

    #[test]
    fn current_truth_fence_applies_result() {
        let (coordinator, mut state, in_flight) = stale_truth_fixture();
        state.truth = Some(in_flight);
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(0.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::Ready);
    }

    #[test]
    fn stale_start_fence_discards_result() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        let fence = coordinator.fence(&state, 1);
        state.generation += 1;
        state.start = Some(InFlight {
            fence,
            result: Some(launch(LaunchOutcomeStatus::Ready)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        let mut processes = vec![];
        coordinator.handle_start_result(
            now(0.0),
            &mut state,
            &mut processes,
            &mut store,
            &mut sink,
            None,
        );
        assert!(processes.is_empty());
    }

    #[test]
    fn current_start_fence_applies_result() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.retry.attempt_count = 1;
        let fence = coordinator.fence(&state, 1);
        state.start = Some(InFlight {
            fence,
            result: Some(launch(LaunchOutcomeStatus::Ready)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        let mut processes = vec![];
        coordinator.handle_start_result(
            now(0.0),
            &mut state,
            &mut processes,
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(state.latest_phase, RuntimePhase::Ready);
        assert_eq!(processes.len(), 1);
    }

    #[test]
    fn stale_stop_fence_discards_result() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        let request = make_stop_request(
            &state,
            managed("old"),
            ReasonCode::known("intent-removed"),
            RuntimePhase::Stopped,
            None,
            false,
        );
        let fence = coordinator.fence(&state, 0);
        state.generation += 1;
        state.pending_stop_request = Some(request);
        state.stop_cleanup = Some(InFlight {
            fence,
            result: Some(stop(StopCleanupStatus::Stopped)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        let mut processes = vec![managed("old")];
        coordinator.handle_stop_cleanup_result(
            now(0.0),
            &mut state,
            &mut processes,
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(processes.len(), 1);
    }

    #[test]
    fn current_stop_fence_applies_result() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        let request = make_stop_request(
            &state,
            managed("old"),
            ReasonCode::known("intent-removed"),
            RuntimePhase::Stopped,
            None,
            false,
        );
        let fence = coordinator.fence(&state, 0);
        state.pending_stop_request = Some(request);
        state.stop_cleanup = Some(InFlight {
            fence,
            result: Some(stop(StopCleanupStatus::Stopped)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        let mut processes = vec![managed("old")];
        coordinator.handle_stop_cleanup_result(
            now(0.0),
            &mut state,
            &mut processes,
            &mut store,
            &mut sink,
            None,
        );
        assert!(processes.is_empty());
        assert_eq!(state.latest_phase, RuntimePhase::Stopped);
    }

    #[test]
    fn stale_probe_fence_discards_result() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.generation += 1;
        state.probe = Some(InFlight {
            fence,
            result: Some(probe(ProbeStatus::NotReady)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_probe_result(now(0.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::Ready);
    }

    #[test]
    fn current_probe_fence_applies_result() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.probe = Some(InFlight {
            fence,
            result: Some(probe(ProbeStatus::NotReady)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_probe_result(now(0.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::ReadyProofUnavailable);
    }

    #[test]
    fn fence_checked_only_at_apply_not_dispatch() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        state.generation = 99;
        let mut workers = RecordingWorkers::default();
        let mut sink = VecEventSink::default();
        coordinator.submit_start_if_needed(
            now(0.0),
            &mut state,
            &[],
            &mut workers,
            &mut sink,
            None,
        );
        assert_eq!(*workers.calls.borrow(), ["start-dispatch"]);
    }

    #[test]
    fn incarnation_is_process_wide_not_per_provider() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("process-one");
        let mut local = state_for(ProviderName::Local);
        let parakeet = state_for(ProviderName::Parakeet);
        let local_fence = coordinator.fence(&local, 1);
        let parakeet_fence = coordinator.fence(&parakeet, 1);
        local.generation += 1;
        assert!(!coordinator.fence_matches(&local, &local_fence, 1));
        assert!(coordinator.fence_matches(&parakeet, &parakeet_fence, 1));
        let restarted = ProviderRuntimeCoordinator::with_incarnation("process-two");
        assert!(!restarted.fence_matches(&local, &local_fence, 1));
        assert!(!restarted.fence_matches(&parakeet, &parakeet_fence, 1));
    }

    #[test]
    fn stop_shape_deferred_target() {
        let mut state = state_for(ProviderName::Local);
        defer_target_stop(&mut state, RuntimePhase::NotDesired, None, false);
        assert_eq!(state.latest_phase, RuntimePhase::StopDeferred);
        assert_eq!(state.pending_stop_target_phase, RuntimePhase::NotDesired);
    }

    #[test]
    fn stop_shape_duplicate_owned_process_cleanup() {
        let state = state_for(ProviderName::Local);
        let request =
            duplicate_owned_process_request(&state, &[managed("new"), managed("old")]).unwrap();
        assert_eq!(request.reason_code.as_str(), "duplicate-owned-process");
        assert_eq!(request.managed.id, "old");
    }

    #[test]
    fn stop_shape_before_replace() {
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        let request = stop_before_replace_request(&state, &[managed("old")]).unwrap();
        assert_eq!(request.target_phase, RuntimePhase::Stopped);
        assert_eq!(request.reason_code.as_str(), "target-changed");
    }

    #[test]
    fn stop_shape_admission_exclusive() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        let request = make_stop_request(
            &state,
            managed("old"),
            ReasonCode::known("admission-exclusive-stop"),
            RuntimePhase::Stopped,
            None,
            true,
        );
        let fence = coordinator.fence(&state, 0);
        state.pending_stop_request = Some(request);
        state.stop_cleanup = Some(InFlight {
            fence,
            result: Some(stop(StopCleanupStatus::StopDeferred)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        let mut processes = vec![managed("old")];
        coordinator.handle_stop_cleanup_result(
            now(0.0),
            &mut state,
            &mut processes,
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(state.latest_phase, RuntimePhase::StopDeferred);
    }

    #[test]
    fn stop_shape_cleanup_failed_retry() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        let fence = coordinator.fence(&state, 0);
        state.stop_cleanup = Some(InFlight {
            fence,
            result: Some(stop(StopCleanupStatus::CleanupFailed)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_stop_cleanup_result(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(state.latest_phase, RuntimePhase::CleanupFailed);
        assert_eq!(state.cleanup_next_at, 2.0);
    }

    #[test]
    fn stop_shape_cancellation() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        let fence = coordinator.fence(&state, 0);
        state.start = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        state.stop_cleanup = Some(InFlight {
            fence,
            result: None,
        });
        cancel_start(&mut state);
        cancel_stop(&mut state);
        assert!(state.start_cancelled);
        assert!(state.stop_cancelled);
    }

    #[test]
    fn cleanup_failure_retries_own_schedule_distinguishable_from_clean_stop() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        let fence = coordinator.fence(&state, 0);
        state.stop_cleanup = Some(InFlight {
            fence,
            result: Some(stop(StopCleanupStatus::CleanupFailed)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_stop_cleanup_result(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(state.latest_phase, RuntimePhase::CleanupFailed);
        assert_eq!(
            state.pending_stop_target_reason_code.unwrap().as_str(),
            "cleanup-attempt-failed"
        );
        assert_eq!(
            state.cleanup_next_at,
            PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS[0]
        );
    }

    #[test]
    fn start_retry_schedule_exact_and_exhausts_to_failed() {
        let mut state = state_for(ProviderName::Local);
        let mut sink = VecEventSink::default();
        for (attempt, delay) in PROVIDER_RETRY_SCHEDULE_SECONDS.into_iter().enumerate() {
            state.retry.attempt_count = attempt as u32;
            assert!(schedule_launch_retry(&mut state, now(100.0), &mut sink));
            assert_eq!(state.retry.next_at, 100.0 + delay);
        }
        state.retry.attempt_count = PROVIDER_RETRY_SCHEDULE_SECONDS.len() as u32;
        assert!(!schedule_launch_retry(&mut state, now(100.0), &mut sink));
        assert_eq!(state.latest_phase, RuntimePhase::Failed);
    }

    #[test]
    fn cleanup_retry_schedule_exact_and_clamps_forever() {
        let mut state = state_for(ProviderName::Local);
        let mut sink = VecEventSink::default();
        for (attempt, delay) in PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS
            .into_iter()
            .enumerate()
        {
            state.cleanup_attempt_count = attempt as u32;
            schedule_cleanup_retry(&mut state, now(100.0), &mut sink);
            assert_eq!(state.cleanup_next_at, 100.0 + delay);
        }
        for _ in 0..3 {
            schedule_cleanup_retry(&mut state, now(100.0), &mut sink);
            assert_eq!(state.cleanup_next_at, 130.0);
            assert_eq!(state.latest_phase, RuntimePhase::CleanupFailed);
        }
    }

    #[test]
    fn probe_and_truth_intervals_derive_from_one_tick_constant() {
        assert_eq!(
            PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS,
            GATE_TICK_INTERVAL_SECONDS
        );
        assert_eq!(PROVIDER_PROBE_INTERVAL_SECONDS, GATE_TICK_INTERVAL_SECONDS);
    }

    fn cortex(kind: CortexEventKind, use_id: &str) -> CortexOutcomeEvent {
        CortexOutcomeEvent {
            kind,
            use_id: use_id.to_owned(),
            provider: Some(ProviderName::Local),
            reason_code: Some("provider_unavailable".to_owned()),
        }
    }

    #[test]
    fn wedge_threshold_fires_recycle_at_exact_count() {
        let mut wedge = WedgeState::default();
        for use_id in ["one", "two", "three"] {
            wedge.observe(cortex(CortexEventKind::Start, use_id), now(0.0));
        }
        assert_eq!(
            wedge.observe(cortex(CortexEventKind::Error, "one"), now(0.0)),
            None
        );
        assert_eq!(
            wedge.observe(cortex(CortexEventKind::Error, "two"), now(0.0)),
            None
        );
        assert_eq!(
            wedge.observe(cortex(CortexEventKind::Error, "three"), now(0.0)),
            Some(ProviderName::Local)
        );
        assert_eq!(LOCAL_WEDGE_THRESHOLD, 3);
    }

    #[test]
    fn wedge_recycle_grace_120s_honored() {
        let mut wedge = WedgeState::default();
        for use_id in ["one", "two", "three", "four", "five", "six"] {
            wedge.observe(cortex(CortexEventKind::Start, use_id), now(0.0));
        }
        for use_id in ["one", "two"] {
            wedge.observe(cortex(CortexEventKind::Error, use_id), now(0.0));
        }
        assert_eq!(
            wedge.observe(cortex(CortexEventKind::Error, "three"), now(0.0)),
            Some(ProviderName::Local)
        );
        for use_id in ["four", "five", "six"] {
            assert_eq!(
                wedge.observe(
                    cortex(CortexEventKind::Error, use_id),
                    now(LOCAL_WEDGE_RECYCLE_GRACE_SECONDS - 1.0)
                ),
                None
            );
        }
        assert_eq!(wedge.failure_count(), 0);
    }

    #[test]
    fn wedge_map_evicts_fifo_at_cap_512_and_names_what_is_lost() {
        let mut wedge = WedgeState::default();
        for index in 0..=LOCAL_WEDGE_PROVIDER_MAP_CAP {
            wedge.observe(
                cortex(CortexEventKind::Start, &format!("use-{index}")),
                now(0.0),
            );
        }
        assert_eq!(wedge.provider_count(), LOCAL_WEDGE_PROVIDER_MAP_CAP);
        assert!(!wedge.contains_use_id("use-0"));
        assert!(wedge.contains_use_id("use-512"));
        // The evicted, still-in-flight use id loses provider attribution; its later error cannot count.
        assert_eq!(
            wedge.observe(cortex(CortexEventKind::Error, "use-0"), now(0.0)),
            None
        );
        assert_eq!(wedge.failure_count(), 0);
    }

    #[test]
    fn startup_gate_tracks_submission_result_and_terminal_phase() {
        let mut gate = ProviderStartupGate::new(now(0.0), [ProviderName::Local]);
        let mut sink = VecEventSink::default();
        gate.on_start_submitted(ProviderName::Local, now(1.0));
        gate.on_start_result(ProviderName::Local, LaunchOutcomeStatus::Ready);
        gate.on_phase(ProviderName::Local, RuntimePhase::Ready);
        assert_eq!(gate.first_start_at, Some(1.0));
        assert!(gate.release_if_ready(now(2.0), &mut sink));
    }

    #[test]
    fn retry_token_is_read_each_reconcile_pass() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Backoff;
        let mut store = InMemoryRuntimeStore::default();
        store.retry_tokens.insert(
            ProviderName::Local,
            RetryToken {
                token_id: "retry".to_owned(),
                desired_fingerprint: state.desired_fingerprint.clone(),
                reason_code: ReasonCode::known("retry-token-requested"),
            },
        );
        let workers = RecordingWorkers::default();
        let mut truth = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut truth,
            &mut lifecycle,
            &mut probe,
            &mut store,
            &mut sink,
            None,
        );
        assert!(!store.retry_tokens.contains_key(&ProviderName::Local));
    }
}
