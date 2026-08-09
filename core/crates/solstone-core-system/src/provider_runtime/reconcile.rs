// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Single-provider reconciliation in the same ordered phases as the supervisor.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::events::{ProviderRuntimeEvent, ProviderRuntimeEventSink};
use super::gate::ProviderStartupGate;
use super::model::{
    ADMISSION_ONLY_REASON_CODES, InFlight, LaunchOutcomeStatus, ManagedProcess,
    PROVIDER_PROBE_INTERVAL_SECONDS, PROVIDER_START_CANCEL_PHASES,
    PROVIDER_STARTUP_TERMINAL_PHASES, PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS,
    PROVIDER_TRUTH_PRESERVED_PHASES, ProbeStatus, ProviderFence, ProviderRuntimeNow,
    ProviderRuntimeState, RuntimePhase, StopCleanupStatus, phase_in,
};
use super::retry::{retry_token_phase, schedule_cleanup_retry, schedule_launch_retry};
use super::seams::{
    LifecycleSeam, ProbeSeam, RuntimeStore, RuntimeStoreError, TruthObservationSeam, reset_retry,
};
use super::stop::{
    cancel_start, cancel_stop, defer_target_stop, duplicate_owned_process_request,
    make_stop_request, queue_orphaned_start_cleanup, stop_before_replace_request,
};

static NEXT_INCARNATION: AtomicU64 = AtomicU64::new(1);

/// Owns the process-wide incarnation used by all provider fences in this process.
#[derive(Debug, Clone)]
pub struct ProviderRuntimeCoordinator {
    incarnation: String,
}

/// Caller-owned seams and optional startup state for one reconciliation pass.
pub struct ReconcileContext<'a> {
    pub truth: &'a mut dyn TruthObservationSeam,
    pub lifecycle: &'a mut dyn LifecycleSeam,
    pub probe: &'a mut dyn ProbeSeam,
    pub store: &'a mut dyn RuntimeStore,
    pub sink: &'a mut dyn ProviderRuntimeEventSink,
    pub gate: Option<&'a mut ProviderStartupGate>,
}

struct SubmissionSeams<'a> {
    lifecycle: &'a mut dyn LifecycleSeam,
    store: &'a mut dyn RuntimeStore,
    sink: &'a mut dyn ProviderRuntimeEventSink,
    gate: Option<&'a mut ProviderStartupGate>,
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
        context: &mut ReconcileContext<'_>,
    ) {
        context
            .sink
            .emit(ProviderRuntimeEvent::Step("handle-retry-token"));
        self.handle_retry_token(
            now,
            state,
            context.store,
            context.sink,
            context.gate.as_deref_mut(),
        );

        context
            .sink
            .emit(ProviderRuntimeEvent::Step("handle-truth-result"));
        self.handle_truth_result(
            now,
            state,
            context.store,
            context.sink,
            context.gate.as_deref_mut(),
        );

        context
            .sink
            .emit(ProviderRuntimeEvent::Step("handle-start-result"));
        self.handle_start_result(
            now,
            state,
            processes,
            context.store,
            context.sink,
            context.gate.as_deref_mut(),
        );

        context
            .sink
            .emit(ProviderRuntimeEvent::Step("handle-stop-cleanup-result"));
        let stop_result_handled = self.handle_stop_cleanup_result(
            now,
            state,
            processes,
            context.store,
            context.sink,
            context.gate.as_deref_mut(),
        );

        context
            .sink
            .emit(ProviderRuntimeEvent::Step("handle-probe-result"));
        self.handle_probe_result(
            now,
            state,
            context.store,
            context.sink,
            context.gate.as_deref_mut(),
        );

        if !stop_result_handled {
            context
                .sink
                .emit(ProviderRuntimeEvent::Step("submit-stop-cleanup-if-needed"));
            self.submit_stop_cleanup_if_needed(
                now,
                state,
                processes,
                &mut SubmissionSeams {
                    lifecycle: context.lifecycle,
                    store: context.store,
                    sink: context.sink,
                    gate: None,
                },
            );

            context
                .sink
                .emit(ProviderRuntimeEvent::Step("submit-start-if-needed"));
            self.submit_start_if_needed(
                now,
                state,
                processes,
                &mut SubmissionSeams {
                    lifecycle: context.lifecycle,
                    store: context.store,
                    sink: context.sink,
                    gate: context.gate.as_deref_mut(),
                },
            );
        }

        context
            .sink
            .emit(ProviderRuntimeEvent::Step("submit-probe-if-needed"));
        self.submit_probe_if_needed(now, state, context.probe, context.sink);

        context
            .sink
            .emit(ProviderRuntimeEvent::Step("submit-truth-if-needed"));
        self.submit_truth_if_needed(now, state, context.truth, context.store, context.sink);

        if let Some(gate) = context.gate.as_deref_mut() {
            gate.release_if_ready(now, context.sink);
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
        gate: Option<&mut ProviderStartupGate>,
    ) {
        let token = match store.read_retry_token(state.provider) {
            Ok(token) => token,
            Err(error) => {
                state.latest_phase = store_error_phase(error);
                self.note_terminal(state, gate);
                return;
            }
        };
        let Some(token) = token else { return };
        if token.desired_fingerprint != state.desired_fingerprint {
            return;
        }
        state.latest_phase = retry_token_phase(state.latest_phase);
        state.latest_reason_code = Some(super::model::ReasonCode::known("retry-token-requested"));
        // supervisor.py:5765-5800 publishes this transition before consuming the token.
        if let Err(error) = store.publish_state(state) {
            state.latest_phase = store_error_phase(error);
            self.note_terminal(state, gate);
            return;
        }
        match store.consume_retry_token(state.provider, &token.token_id) {
            Ok(()) => {
                reset_retry(state);
                state.next_truth_at = now.monotonic_seconds;
            }
            // Another owner consumed it; retain the durable retry-requested transition.
            Err(RuntimeStoreError::Conflict) => {}
            Err(RuntimeStoreError::Corrupt) => {
                state.latest_phase = RuntimePhase::StateCorrupt;
                self.note_terminal(state, gate);
            }
            Err(RuntimeStoreError::Unavailable) => {
                state.latest_phase = RuntimePhase::StateUnavailable;
                self.note_terminal(state, gate);
            }
        }
        let _ = sink;
    }

    fn handle_truth_result(
        &self,
        now: ProviderRuntimeNow,
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
            state.next_truth_at = now.monotonic_seconds;
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("stale-result-ignored"));
            self.persist(state, store);
            return;
        }
        if result.provider != state.provider {
            return;
        }
        if result.phase != RuntimePhase::ArtifactNotReady {
            state.replacement_artifact_not_ready_fingerprint = None;
        }
        let fingerprint_changed = state.desired_fingerprint != result.desired_fingerprint;
        let pending_target = state
            .pending_stop_request
            .as_ref()
            .map_or(state.pending_stop_target_phase, |request| {
                request.target_phase
            });
        if !fingerprint_changed
            && matches!(
                state.latest_phase,
                RuntimePhase::StopDeferred | RuntimePhase::Stopping | RuntimePhase::CleanupFailed
            )
            && result.phase == pending_target
        {
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("stale-result-ignored"));
            self.persist(state, store);
            return;
        }
        if !fingerprint_changed
            && state.has_plan
            && ((result.phase == RuntimePhase::Starting
                && matches!(
                    state.latest_phase,
                    RuntimePhase::Starting
                        | RuntimePhase::Ready
                        | RuntimePhase::ReadyProofUnavailable
                ))
                || (result.phase == RuntimePhase::HostBlocked
                    && state.latest_phase == RuntimePhase::Starting))
        {
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("stale-result-ignored"));
            self.persist(state, store);
            return;
        }
        if state.stop_cleanup.is_some()
            && (fingerprint_changed || result.phase == RuntimePhase::Starting)
        {
            cancel_stop(state);
        }
        if state.start.is_some()
            && (fingerprint_changed || phase_in(&PROVIDER_START_CANCEL_PHASES, result.phase))
        {
            cancel_start(state);
        }
        if fingerprint_changed
            && result.phase == RuntimePhase::Starting
            && matches!(
                state.latest_phase,
                RuntimePhase::Ready | RuntimePhase::ReadyProofUnavailable
            )
        {
            state.generation += 1;
            state.desired_fingerprint = result.desired_fingerprint.clone();
            reset_retry(state);
            state.has_plan = result.has_plan;
            defer_target_stop(
                state,
                RuntimePhase::Starting,
                result.reason_code.clone(),
                false,
            );
            state.latest_reason_code = Some(super::model::ReasonCode::known("target-changed"));
            self.persist(state, store);
            return;
        }
        if fingerprint_changed
            && result.phase == RuntimePhase::ArtifactNotReady
            && matches!(
                state.latest_phase,
                RuntimePhase::Ready | RuntimePhase::ReadyProofUnavailable
            )
        {
            state.replacement_artifact_not_ready_fingerprint = result.desired_fingerprint.clone();
            state.latest_phase = result.phase;
            state.latest_reason_code = result.reason_code;
            self.note_terminal(state, gate);
            self.persist(state, store);
            return;
        }
        if !fingerprint_changed
            && result.phase == RuntimePhase::HostBlocked
            && result
                .reason_code
                .as_ref()
                .is_some_and(|reason| ADMISSION_ONLY_REASON_CODES.contains(&reason.as_str()))
            && matches!(
                state.latest_phase,
                RuntimePhase::Ready | RuntimePhase::ReadyProofUnavailable
            )
        {
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("stale-result-ignored"));
            self.persist(state, store);
            return;
        }
        if matches!(
            result.phase,
            RuntimePhase::NotDesired | RuntimePhase::HostBlocked
        ) && matches!(
            state.latest_phase,
            RuntimePhase::Ready | RuntimePhase::ReadyProofUnavailable
        ) {
            if fingerprint_changed {
                state.generation += 1;
                state.desired_fingerprint = result.desired_fingerprint.clone();
                reset_retry(state);
            }
            defer_target_stop(state, result.phase, result.reason_code, true);
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("admission-exclusive-stop"));
            self.persist(state, store);
            return;
        }
        if fingerprint_changed {
            state.generation += 1;
            state.desired_fingerprint = result.desired_fingerprint.clone();
            reset_retry(state);
        }
        state.has_plan = result.has_plan;
        state.boot_required = result.boot_required;
        state.latest_phase = result.phase;
        state.latest_reason_code = result.reason_code;
        if state.latest_phase == RuntimePhase::Observing
            && state
                .latest_reason_code
                .as_ref()
                .is_some_and(|reason| reason.as_str() == "observation-raced")
        {
            state.next_truth_at = now.monotonic_seconds;
        }
        self.note_terminal(state, gate);
        self.persist(state, store);
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
        if let Some(gate) = gate.as_deref_mut() {
            gate.on_start_result(state.provider, result.status);
        }
        if !self.fence_matches(state, &in_flight.fence, state.retry.attempt_count) {
            sink.emit(ProviderRuntimeEvent::StaleResultDiscarded {
                operation: "start",
                provider: state.provider,
            });
            if let Some(managed) = result.managed {
                // This crate decides cleanup; the LifecycleSeam performs concrete termination.
                queue_orphaned_start_cleanup(
                    state,
                    managed,
                    state.latest_phase,
                    state.latest_reason_code.clone(),
                );
            }
            return;
        }
        if state.start_cancelled || phase_in(&PROVIDER_START_CANCEL_PHASES, state.latest_phase) {
            state.start_cancelled = false;
            if let Some(managed) = result.managed {
                // A cancelled start may still have produced a process that must be cleaned up.
                queue_orphaned_start_cleanup(
                    state,
                    managed,
                    state.latest_phase,
                    state.latest_reason_code.clone(),
                );
            }
            return;
        }
        let mut managed = result.managed;
        let result_reason_code = result.reason_code.clone();
        match result.status {
            LaunchOutcomeStatus::Ready => {
                if let Some(managed) = managed.take() {
                    processes.push(managed);
                    state.latest_phase = RuntimePhase::Ready;
                    state.latest_reason_code = Some(result_reason_code.clone());
                    self.note_terminal(state, gate);
                } else {
                    schedule_launch_retry(state, now, sink);
                    state.latest_reason_code = Some(result_reason_code.clone());
                    self.note_terminal(state, gate);
                }
            }
            LaunchOutcomeStatus::HostBlocked => {
                state.latest_phase = RuntimePhase::HostBlocked;
                state.latest_reason_code = Some(result_reason_code.clone());
                self.note_terminal(state, gate);
            }
            _ => {
                schedule_launch_retry(state, now, sink);
                state.latest_reason_code = Some(result_reason_code.clone());
                self.note_terminal(state, gate);
            }
        }
        if let Some(managed) = managed {
            queue_orphaned_start_cleanup(
                state,
                managed,
                state.latest_phase,
                state.latest_reason_code.clone(),
            );
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
        let orphaned_start_outcome = state
            .pending_stop_request
            .as_ref()
            .is_some_and(|request| request.orphaned_start_outcome);
        match result.status {
            StopCleanupStatus::Stopped => {
                if let Some(request) = state.pending_stop_request.take() {
                    processes.retain(|process| process.id != request.managed.id);
                    if !request.orphaned_start_outcome {
                        state.latest_phase = request.target_phase;
                        state.latest_reason_code = Some(result.reason_code.clone());
                        state.pending_stop_target_reason_code = request.target_reason_code;
                    }
                } else {
                    state.latest_phase = RuntimePhase::Stopped;
                }
                state.cleanup_attempt_count = 0;
                self.note_terminal(state, gate);
            }
            StopCleanupStatus::StopDeferred => {
                if !orphaned_start_outcome {
                    state.latest_phase = RuntimePhase::StopDeferred;
                    state.latest_reason_code = Some(result.reason_code.clone());
                }
                sink.emit(ProviderRuntimeEvent::StopDeferred {
                    provider: state.provider,
                });
            }
            StopCleanupStatus::CleanupFailed => {
                state.pending_stop_target_reason_code = Some(result.reason_code);
                schedule_cleanup_retry(state, now, sink);
                state.latest_reason_code = state.pending_stop_target_reason_code.clone();
            }
            StopCleanupStatus::Cancelled => unreachable!("handled above"),
        }
        self.persist(state, store);
        true
    }

    fn handle_probe_result(
        &self,
        now: ProviderRuntimeNow,
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
        if state.replacement_artifact_not_ready_fingerprint.is_some()
            && result.status == ProbeStatus::Ready
        {
            state.next_probe_at = now.monotonic_seconds + PROVIDER_PROBE_INTERVAL_SECONDS;
            return;
        }
        if !self.fence_matches(state, &in_flight.fence, state.retry.attempt_count) {
            sink.emit(ProviderRuntimeEvent::StaleResultDiscarded {
                operation: "probe",
                provider: state.provider,
            });
            return;
        }
        state.next_probe_at = now.monotonic_seconds + PROVIDER_PROBE_INTERVAL_SECONDS;
        if state.pending_stop_request.is_some() {
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("stale-result-ignored"));
            self.persist(state, store);
            return;
        }
        state.latest_phase = match result.status {
            ProbeStatus::Ready => RuntimePhase::Ready,
            ProbeStatus::NotReady | ProbeStatus::Unavailable => RuntimePhase::ReadyProofUnavailable,
        };
        state.latest_reason_code = Some(result.reason_code);
        self.note_terminal(state, gate);
        self.persist(state, store);
    }

    fn submit_stop_cleanup_if_needed(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        processes: &[ManagedProcess],
        seams: &mut SubmissionSeams<'_>,
    ) {
        if state.stop_cleanup.is_some() || now.monotonic_seconds < state.cleanup_next_at {
            return;
        }
        let request = if let Some(request) = state.pending_stop_request.clone() {
            Some(request)
        } else if !state.orphaned_stop_requests.is_empty() {
            Some(state.orphaned_stop_requests.remove(0))
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
        let orphaned_start_outcome = request.orphaned_start_outcome;
        state.pending_stop_request = Some(request);
        if !orphaned_start_outcome {
            state.latest_phase = RuntimePhase::Stopping;
            state.latest_reason_code = state
                .pending_stop_request
                .as_ref()
                .map(|request| request.reason_code.clone());
        }
        let fence = self.fence(state, state.cleanup_attempt_count);
        state.stop_cleanup = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        if !orphaned_start_outcome {
            self.persist(state, seams.store);
        }
        seams.lifecycle.dispatch_stop(state, &fence);
        seams.sink.emit(ProviderRuntimeEvent::Dispatched {
            operation: "stop-cleanup",
            provider: state.provider,
        });
    }

    fn submit_start_if_needed(
        &self,
        now: ProviderRuntimeNow,
        state: &mut ProviderRuntimeState,
        processes: &[ManagedProcess],
        seams: &mut SubmissionSeams<'_>,
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
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("launch-budget-exhausted"));
            self.note_terminal(state, seams.gate.as_deref_mut());
            self.persist(state, seams.store);
            return;
        }
        state.retry.attempt_count += 1;
        state.latest_phase = RuntimePhase::Starting;
        state.latest_reason_code = Some(super::model::ReasonCode::known("launch-requested"));
        let fence = self.fence(state, state.retry.attempt_count);
        state.start = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        if let Some(gate) = seams.gate.as_deref_mut() {
            gate.on_start_submitted(state.provider, now);
        }
        self.persist(state, seams.store);
        seams.lifecycle.dispatch_start(state, &fence);
        seams.sink.emit(ProviderRuntimeEvent::Dispatched {
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
        store: &mut dyn RuntimeStore,
        sink: &mut dyn ProviderRuntimeEventSink,
    ) {
        if state.truth.is_some() || now.monotonic_seconds < state.next_truth_at {
            return;
        }
        state.next_truth_at = now.monotonic_seconds + PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS;
        let phase_changed = !phase_in(&PROVIDER_TRUTH_PRESERVED_PHASES, state.latest_phase);
        if phase_changed {
            state.latest_phase = RuntimePhase::Observing;
            state.latest_reason_code =
                Some(super::model::ReasonCode::known("truth-observation-started"));
        }
        let fence = self.fence(state, state.retry.attempt_count);
        state.truth = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        if phase_changed {
            self.persist(state, store);
        }
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
        PROVIDER_RETRY_SCHEDULE_SECONDS, PROVIDER_STARTUP_GATE_CEILING_SECONDS,
        PROVIDER_STARTUP_GATE_WINDOW_SECONDS, ProviderLaunchOutcome, ProviderName,
        ProviderProbeOutcome, ProviderStopCleanupOutcome, ProviderTruthObservation, ReasonCode,
        RetryToken, VecEventSink, WedgeState, cancel_start, cancel_stop, defer_target_stop,
        duplicate_owned_process_request, schedule_cleanup_retry, schedule_launch_retry,
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

    struct RetryStore {
        token: Option<RetryToken>,
        consume_error: Option<RuntimeStoreError>,
        calls: Vec<&'static str>,
        published: Vec<RuntimePhase>,
    }

    impl RetryStore {
        fn with_token(token: RetryToken) -> Self {
            Self {
                token: Some(token),
                consume_error: None,
                calls: Vec::new(),
                published: Vec::new(),
            }
        }
    }

    impl RuntimeStore for RetryStore {
        fn read_retry_token(
            &mut self,
            _: ProviderName,
        ) -> Result<Option<RetryToken>, RuntimeStoreError> {
            self.calls.push("read");
            Ok(self.token.clone())
        }

        fn consume_retry_token(
            &mut self,
            _: ProviderName,
            _: &str,
        ) -> Result<(), RuntimeStoreError> {
            self.calls.push("consume");
            if let Some(error) = self.consume_error.clone() {
                return Err(error);
            }
            self.token = None;
            Ok(())
        }

        fn publish_state(&mut self, state: &ProviderRuntimeState) -> Result<(), RuntimeStoreError> {
            self.calls.push("publish");
            self.published.push(state.latest_phase);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct OrderedWorkers {
        calls: Rc<RefCell<Vec<&'static str>>>,
    }

    impl TruthObservationSeam for OrderedWorkers {
        fn dispatch_truth(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("truth-dispatch");
        }
    }

    impl LifecycleSeam for OrderedWorkers {
        fn dispatch_start(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("start-dispatch");
        }

        fn dispatch_stop(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("stop-dispatch");
        }
    }

    impl ProbeSeam for OrderedWorkers {
        fn dispatch_probe(&mut self, _: &ProviderRuntimeState, _: &ProviderFence) {
            self.calls.borrow_mut().push("probe-dispatch");
        }
    }

    struct OrderedStore {
        calls: Rc<RefCell<Vec<&'static str>>>,
    }

    impl RuntimeStore for OrderedStore {
        fn read_retry_token(
            &mut self,
            _: ProviderName,
        ) -> Result<Option<RetryToken>, RuntimeStoreError> {
            Ok(None)
        }

        fn consume_retry_token(
            &mut self,
            _: ProviderName,
            _: &str,
        ) -> Result<(), RuntimeStoreError> {
            Ok(())
        }

        fn publish_state(&mut self, _: &ProviderRuntimeState) -> Result<(), RuntimeStoreError> {
            self.calls.borrow_mut().push("publish");
            Ok(())
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
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
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
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
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
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert!(workers.calls.borrow().is_empty());
    }

    #[test]
    fn reconcile_preserves_distinct_store_and_probe_runtime_phases() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let workers = RecordingWorkers::default();
        let mut truth = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe_worker = workers.clone();
        let mut unavailable = state_for(ProviderName::Local);
        unavailable.latest_phase = RuntimePhase::Backoff;
        unavailable.next_truth_at = 100.0;
        let mut unavailable_store = InMemoryRuntimeStore {
            failure: Some(RuntimeStoreError::Unavailable),
            ..InMemoryRuntimeStore::default()
        };
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(0.0),
            &mut unavailable,
            &mut vec![],
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe_worker,
                store: &mut unavailable_store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(unavailable.latest_phase, RuntimePhase::StateUnavailable);

        let mut corrupt = state_for(ProviderName::Local);
        corrupt.latest_phase = RuntimePhase::Backoff;
        corrupt.next_truth_at = 100.0;
        let mut corrupt_store = InMemoryRuntimeStore {
            failure: Some(RuntimeStoreError::Corrupt),
            ..InMemoryRuntimeStore::default()
        };
        coordinator.reconcile(
            now(0.0),
            &mut corrupt,
            &mut vec![],
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe_worker,
                store: &mut corrupt_store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(corrupt.latest_phase, RuntimePhase::StateCorrupt);

        for (status, expected) in [
            (ProbeStatus::Ready, RuntimePhase::Ready),
            (
                ProbeStatus::Unavailable,
                RuntimePhase::ReadyProofUnavailable,
            ),
        ] {
            let mut state = state_for(ProviderName::Local);
            state.latest_phase = RuntimePhase::Ready;
            state.next_truth_at = 100.0;
            state.next_probe_at = 100.0;
            let fence = coordinator.fence(&state, 0);
            state.probe = Some(InFlight {
                fence,
                result: Some(probe(status)),
            });
            let mut store = InMemoryRuntimeStore::default();
            coordinator.reconcile(
                now(0.0),
                &mut state,
                &mut vec![],
                &mut ReconcileContext {
                    truth: &mut truth,
                    lifecycle: &mut lifecycle,
                    probe: &mut probe_worker,
                    store: &mut store,
                    sink: &mut sink,
                    gate: None,
                },
            );
            assert_eq!(state.latest_phase, expected);
        }
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
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        coordinator.reconcile(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
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
        assert_eq!(
            state.latest_reason_code.as_ref().map(ReasonCode::as_str),
            Some("launch-failed")
        );
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
        assert_eq!(
            state.latest_reason_code.as_ref().map(ReasonCode::as_str),
            Some("cleanup-attempt-failed")
        );
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
        assert_eq!(
            state.latest_reason_code.as_ref().map(ReasonCode::as_str),
            Some("probe-not-ready")
        );
    }

    #[test]
    fn ready_probe_preserves_replacement_artifact_and_pending_cleanup_phase() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();

        let mut artifact_state = state_for(ProviderName::Local);
        artifact_state.latest_phase = RuntimePhase::ArtifactNotReady;
        artifact_state.replacement_artifact_not_ready_fingerprint = Some("desired-b".to_owned());
        let fence = coordinator.fence(&artifact_state, 0);
        artifact_state.probe = Some(InFlight {
            fence,
            result: Some(probe(ProbeStatus::Ready)),
        });
        coordinator.handle_probe_result(
            now(10.0),
            &mut artifact_state,
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(artifact_state.latest_phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            artifact_state.next_probe_at,
            10.0 + PROVIDER_PROBE_INTERVAL_SECONDS
        );

        let mut cleanup_state = state_for(ProviderName::Local);
        cleanup_state.latest_phase = RuntimePhase::Stopping;
        let request = make_stop_request(
            &cleanup_state,
            managed("old"),
            ReasonCode::known("intent-removed"),
            RuntimePhase::Stopped,
            None,
            false,
        );
        cleanup_state.pending_stop_request = Some(request);
        let fence = coordinator.fence(&cleanup_state, 0);
        cleanup_state.probe = Some(InFlight {
            fence,
            result: Some(probe(ProbeStatus::Ready)),
        });
        coordinator.handle_probe_result(now(10.0), &mut cleanup_state, &mut store, &mut sink, None);
        assert_eq!(cleanup_state.latest_phase, RuntimePhase::Stopping);
        assert_eq!(
            cleanup_state
                .latest_reason_code
                .as_ref()
                .map(ReasonCode::as_str),
            Some("stale-result-ignored")
        );
    }

    #[test]
    fn probe_interval_starts_when_the_result_lands() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let mut workers = RecordingWorkers::default();
        let mut sink = VecEventSink::default();
        coordinator.submit_probe_if_needed(now(0.0), &mut state, &mut workers, &mut sink);
        assert_eq!(state.next_probe_at, 0.0);
        state.probe.as_mut().unwrap().result = Some(probe(ProbeStatus::Ready));
        let mut store = InMemoryRuntimeStore::default();
        coordinator.handle_probe_result(now(10.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.next_probe_at, 10.0 + PROVIDER_PROBE_INTERVAL_SECONDS);
        coordinator.submit_probe_if_needed(now(10.0), &mut state, &mut workers, &mut sink);
        assert_eq!(*workers.calls.borrow(), ["probe-dispatch"]);
    }

    #[test]
    fn fence_checked_only_at_apply_not_dispatch() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        state.generation = 99;
        let mut workers = RecordingWorkers::default();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.submit_start_if_needed(
            now(0.0),
            &mut state,
            &[],
            &mut SubmissionSeams {
                lifecycle: &mut workers,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(*workers.calls.borrow(), ["start-dispatch"]);
        state.generation += 1;
        state.start.as_mut().unwrap().result = Some(launch(LaunchOutcomeStatus::Ready));
        coordinator.handle_start_result(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(state.latest_phase, RuntimePhase::Starting);
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

    fn truth(
        provider: ProviderName,
        phase: RuntimePhase,
        fingerprint: Option<&str>,
        reason: Option<&'static str>,
    ) -> ProviderTruthObservation {
        ProviderTruthObservation {
            provider,
            phase,
            reason_code: reason.map(ReasonCode::known),
            desired_fingerprint: fingerprint.map(str::to_owned),
            has_plan: true,
            boot_required: false,
        }
    }

    #[test]
    fn truth_wrong_provider_is_a_noop() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Parakeet,
                RuntimePhase::NotDesired,
                Some("desired-b"),
                Some("intent-disabled"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::Ready);
        assert!(store.published.is_empty());
    }

    #[test]
    fn pending_cleanup_and_starting_noise_keep_authoritative_phase() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::StopDeferred;
        state.pending_stop_target_phase = RuntimePhase::NotDesired;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::NotDesired,
                Some("desired-a"),
                Some("intent-disabled"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::StopDeferred);

        state.latest_phase = RuntimePhase::Starting;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::HostBlocked,
                Some("desired-a"),
                Some("ram-insufficient"),
            )),
        });
        coordinator.handle_truth_result(now(2.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::Starting);
    }

    #[test]
    fn replacement_start_defers_cleanup_without_overwriting_ready() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::Starting,
                Some("desired-b"),
                Some("launch-requested"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::StopDeferred);
        assert_eq!(state.pending_stop_target_phase, RuntimePhase::Starting);
        assert_eq!(state.desired_fingerprint.as_deref(), Some("desired-b"));
    }

    #[test]
    fn replacement_artifact_not_ready_remains_visible() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::ArtifactNotReady,
                Some("desired-b"),
                Some("artifact-missing"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            state.replacement_artifact_not_ready_fingerprint.as_deref(),
            Some("desired-b")
        );
    }

    #[test]
    fn ready_provider_ignores_unchanged_ram_admission_block() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::HostBlocked,
                Some("desired-a"),
                Some("ram-insufficient"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::Ready);
        assert_eq!(
            state.latest_reason_code.as_ref().map(ReasonCode::as_str),
            Some("stale-result-ignored")
        );
    }

    #[test]
    fn ready_not_desired_defers_an_admission_exclusive_stop_via_reconcile() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        state.next_truth_at = 100.0;
        state.next_probe_at = 100.0;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::NotDesired,
                Some("desired-a"),
                Some("intent-disabled"),
            )),
        });
        let workers = RecordingWorkers::default();
        let mut truth_worker = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(1.0),
            &mut state,
            &mut vec![managed("old")],
            &mut ReconcileContext {
                truth: &mut truth_worker,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(*workers.calls.borrow(), ["stop-dispatch"]);
        assert!(
            state
                .pending_stop_request
                .as_ref()
                .unwrap()
                .admission_exclusive
        );
        assert_eq!(state.pending_stop_target_phase, RuntimePhase::NotDesired);
    }

    #[test]
    fn successive_truth_deferrals_retarget_the_existing_stop_request() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        state.pending_stop_request = Some(make_stop_request(
            &state,
            managed("old"),
            ReasonCode::known("target-changed"),
            RuntimePhase::Stopped,
            Some(ReasonCode::known("cleanup-succeeded")),
            false,
        ));
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::NotDesired,
                Some("desired-a"),
                Some("intent-disabled"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(
            state.pending_stop_request.as_ref().unwrap().target_phase,
            RuntimePhase::NotDesired
        );

        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::Starting,
                Some("desired-b"),
                Some("launch-requested"),
            )),
        });
        coordinator.handle_truth_result(now(2.0), &mut state, &mut store, &mut sink, None);
        let request = state.pending_stop_request.as_ref().unwrap();
        assert_eq!(request.target_phase, RuntimePhase::Starting);
        assert_eq!(
            request.target_reason_code.as_ref().map(ReasonCode::as_str),
            Some("launch-requested")
        );
    }

    #[test]
    fn orphaned_cleanup_completion_does_not_rollback_newer_truth() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let mut request = make_stop_request(
            &state,
            managed("orphan"),
            ReasonCode::known("launch-failed"),
            RuntimePhase::Stopped,
            Some(ReasonCode::known("cleanup-succeeded")),
            false,
        );
        request.orphaned_start_outcome = true;
        state.pending_stop_request = Some(request);
        let fence = coordinator.fence(&state, 0);
        state.stop_cleanup = Some(InFlight {
            fence: fence.clone(),
            result: Some(stop(StopCleanupStatus::Stopped)),
        });
        state.truth = Some(InFlight {
            fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::ReadyProofUnavailable,
                Some("desired-a"),
                Some("proof-observation-unavailable"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        coordinator.handle_stop_cleanup_result(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(state.latest_phase, RuntimePhase::ReadyProofUnavailable);
        assert_eq!(
            state.latest_reason_code.as_ref().map(ReasonCode::as_str),
            Some("proof-observation-unavailable")
        );
    }

    #[test]
    fn truth_result_signals_in_flight_start_and_stop_cancellation() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        let fence = coordinator.fence(&state, 0);
        state.start = Some(InFlight {
            fence: fence.clone(),
            result: None,
        });
        state.stop_cleanup = Some(InFlight {
            fence,
            result: None,
        });
        let truth_fence = coordinator.fence(&state, 0);
        state.truth = Some(InFlight {
            fence: truth_fence,
            result: Some(truth(
                ProviderName::Local,
                RuntimePhase::NotDesired,
                Some("desired-b"),
                Some("intent-disabled"),
            )),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_truth_result(now(1.0), &mut state, &mut store, &mut sink, None);
        assert!(state.start_cancelled);
        assert!(state.stop_cancelled);
    }

    #[test]
    fn cancelled_ready_start_is_routed_to_stop_cleanup_not_ready() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        state.next_truth_at = 100.0;
        state.retry.attempt_count = 1;
        let fence = coordinator.fence(&state, 1);
        state.start = Some(InFlight {
            fence,
            result: Some(launch(LaunchOutcomeStatus::Ready)),
        });
        state.start_cancelled = true;
        let workers = RecordingWorkers::default();
        let mut truth_worker = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        let mut processes = vec![];
        coordinator.reconcile(
            now(1.0),
            &mut state,
            &mut processes,
            &mut ReconcileContext {
                truth: &mut truth_worker,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(*workers.calls.borrow(), ["stop-dispatch"]);
        assert_ne!(state.latest_phase, RuntimePhase::Ready);
        assert!(processes.is_empty());
    }

    #[test]
    fn stale_ready_start_is_cleaned_without_overwriting_current_generation_phase() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        state.next_truth_at = 100.0;
        state.next_probe_at = 100.0;
        state.retry.attempt_count = 1;
        let fence = coordinator.fence(&state, 1);
        state.generation += 1;
        state.start = Some(InFlight {
            fence,
            result: Some(launch(LaunchOutcomeStatus::Ready)),
        });
        let workers = RecordingWorkers::default();
        let mut truth_worker = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        let mut processes = vec![];
        coordinator.reconcile(
            now(1.0),
            &mut state,
            &mut processes,
            &mut ReconcileContext {
                truth: &mut truth_worker,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(state.latest_phase, RuntimePhase::Ready);
        assert_eq!(*workers.calls.borrow(), ["stop-dispatch"]);
        assert!(processes.is_empty());
    }

    #[test]
    fn orphaned_cleanup_failure_uses_normal_cleanup_retry_schedule() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        state.start_cancelled = true;
        state.retry.attempt_count = 1;
        let fence = coordinator.fence(&state, 1);
        state.start = Some(InFlight {
            fence,
            result: Some(launch(LaunchOutcomeStatus::Ready)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_start_result(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        let mut workers = RecordingWorkers::default();
        coordinator.submit_stop_cleanup_if_needed(
            now(0.0),
            &mut state,
            &[],
            &mut SubmissionSeams {
                lifecycle: &mut workers,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        state.stop_cleanup.as_mut().unwrap().result = Some(stop(StopCleanupStatus::CleanupFailed));
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
            state.cleanup_next_at,
            PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS[0]
        );
    }

    #[test]
    fn pending_and_orphaned_cleanups_are_drained_without_losing_a_handle() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        state.start_cancelled = true;
        state.retry.attempt_count = 1;
        let fence = coordinator.fence(&state, 1);
        state.start = Some(InFlight {
            fence,
            result: Some(launch(LaunchOutcomeStatus::Ready)),
        });
        let mut store = InMemoryRuntimeStore::default();
        let mut sink = VecEventSink::default();
        coordinator.handle_start_result(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        let mut workers = RecordingWorkers::default();
        coordinator.submit_stop_cleanup_if_needed(
            now(0.0),
            &mut state,
            &[],
            &mut SubmissionSeams {
                lifecycle: &mut workers,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        state.start_cancelled = true;
        let second_fence = coordinator.fence(&state, 1);
        state.start = Some(InFlight {
            fence: second_fence,
            result: Some(launch(LaunchOutcomeStatus::Ready)),
        });
        coordinator.handle_start_result(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        assert_eq!(state.orphaned_stop_requests.len(), 1);
        state.stop_cleanup.as_mut().unwrap().result = Some(stop(StopCleanupStatus::Stopped));
        coordinator.handle_stop_cleanup_result(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut store,
            &mut sink,
            None,
        );
        coordinator.submit_stop_cleanup_if_needed(
            now(1.0),
            &mut state,
            &[],
            &mut SubmissionSeams {
                lifecycle: &mut workers,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert!(state.stop_cleanup.is_some());
        assert!(state.orphaned_stop_requests.is_empty());
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
        for use_id in ["four", "five", "six"] {
            assert_eq!(
                wedge.observe(
                    cortex(CortexEventKind::Error, use_id),
                    now(LOCAL_WEDGE_RECYCLE_GRACE_SECONDS + 1.0)
                ),
                if use_id == "six" {
                    Some(ProviderName::Local)
                } else {
                    None
                }
            );
        }
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
    fn startup_gate_window_waits_for_in_flight_start_but_ceiling_does_not() {
        let mut gate = ProviderStartupGate::new(now(0.0), [ProviderName::Local]);
        let mut sink = VecEventSink::default();
        gate.on_start_submitted(ProviderName::Local, now(1.0));
        assert!(!gate.release_if_ready(now(PROVIDER_STARTUP_GATE_WINDOW_SECONDS), &mut sink));
        gate.on_start_result(ProviderName::Local, LaunchOutcomeStatus::LaunchFailed);
        assert!(gate.release_if_ready(now(PROVIDER_STARTUP_GATE_WINDOW_SECONDS), &mut sink));

        let mut ceiling_gate = ProviderStartupGate::new(now(0.0), [ProviderName::Local]);
        ceiling_gate.on_start_submitted(ProviderName::Local, now(1.0));
        assert!(ceiling_gate.release_if_ready(
            now(1.0 + PROVIDER_STARTUP_GATE_CEILING_SECONDS),
            &mut VecEventSink::default(),
        ));
    }

    #[test]
    fn cortex_outcomes_emit_one_recycle_request_per_threshold() {
        let mut wedge = WedgeState::default();
        let mut sink = VecEventSink::default();
        for use_id in ["one", "two", "three"] {
            super::super::wedge::observe_cortex_outcome(
                &mut wedge,
                cortex(CortexEventKind::Start, use_id),
                now(0.0),
                &mut sink,
            );
        }
        for use_id in ["one", "two"] {
            assert_eq!(
                super::super::wedge::observe_cortex_outcome(
                    &mut wedge,
                    cortex(CortexEventKind::Error, use_id),
                    now(0.0),
                    &mut sink,
                ),
                None
            );
        }
        assert_eq!(
            super::super::wedge::observe_cortex_outcome(
                &mut wedge,
                cortex(CortexEventKind::Error, "three"),
                now(0.0),
                &mut sink,
            ),
            Some(ProviderName::Local)
        );
        assert_eq!(
            sink.events,
            [ProviderRuntimeEvent::RecycleRequested {
                provider: ProviderName::Local
            }]
        );
    }

    #[test]
    fn cortex_finish_clears_wedge_failures() {
        let mut wedge = WedgeState::default();
        wedge.observe(cortex(CortexEventKind::Start, "one"), now(0.0));
        wedge.observe(cortex(CortexEventKind::Error, "one"), now(0.0));
        assert_eq!(wedge.failure_count(), 1);
        wedge.observe(cortex(CortexEventKind::Finish, "one"), now(0.0));
        assert_eq!(wedge.failure_count(), 0);
    }

    #[test]
    fn visible_submit_transitions_publish_before_worker_dispatch() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let workers = OrderedWorkers::default();
        let calls = workers.calls.clone();
        let mut store = OrderedStore {
            calls: calls.clone(),
        };
        let mut sink = VecEventSink::default();

        let mut truth_state = state_for(ProviderName::Local);
        truth_state.latest_phase = RuntimePhase::Stopped;
        coordinator.submit_truth_if_needed(
            now(0.0),
            &mut truth_state,
            &mut workers.clone(),
            &mut store,
            &mut sink,
        );
        assert_eq!(*calls.borrow(), ["publish", "truth-dispatch"]);

        calls.borrow_mut().clear();
        let mut start_state = state_for(ProviderName::Local);
        start_state.latest_phase = RuntimePhase::Starting;
        coordinator.submit_start_if_needed(
            now(0.0),
            &mut start_state,
            &[],
            &mut SubmissionSeams {
                lifecycle: &mut workers.clone(),
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(*calls.borrow(), ["publish", "start-dispatch"]);

        calls.borrow_mut().clear();
        let mut stop_state = state_for(ProviderName::Local);
        defer_target_stop(&mut stop_state, RuntimePhase::NotDesired, None, false);
        coordinator.submit_stop_cleanup_if_needed(
            now(0.0),
            &mut stop_state,
            &[managed("old")],
            &mut SubmissionSeams {
                lifecycle: &mut workers.clone(),
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(*calls.borrow(), ["publish", "stop-dispatch"]);
    }

    #[test]
    fn start_budget_exhaustion_publishes_failed_without_dispatch() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let workers = OrderedWorkers::default();
        let calls = workers.calls.clone();
        let mut store = OrderedStore {
            calls: calls.clone(),
        };
        let mut sink = VecEventSink::default();
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Starting;
        state.retry.attempt_count = PROVIDER_RETRY_SCHEDULE_SECONDS.len() as u32;
        coordinator.submit_start_if_needed(
            now(0.0),
            &mut state,
            &[],
            &mut SubmissionSeams {
                lifecycle: &mut workers.clone(),
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(state.latest_phase, RuntimePhase::Failed);
        assert_eq!(*calls.borrow(), ["publish"]);
    }

    #[test]
    fn ordinary_probe_submission_does_not_publish_runtime_state() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let workers = OrderedWorkers::default();
        let calls = workers.calls.clone();
        let mut sink = VecEventSink::default();
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Ready;
        coordinator.submit_probe_if_needed(now(0.0), &mut state, &mut workers.clone(), &mut sink);
        assert_eq!(*calls.borrow(), ["probe-dispatch"]);
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
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert!(!store.retry_tokens.contains_key(&ProviderName::Local));
    }

    #[test]
    fn retry_token_store_failure_finishes_the_startup_gate() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Backoff;
        state.next_truth_at = 100.0;
        let workers = RecordingWorkers::default();
        let mut truth = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut store = InMemoryRuntimeStore {
            failure: Some(RuntimeStoreError::Unavailable),
            ..InMemoryRuntimeStore::default()
        };
        let mut sink = VecEventSink::default();
        let mut gate = ProviderStartupGate::new(now(0.0), [ProviderName::Local]);
        coordinator.reconcile(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut ReconcileContext {
                truth: &mut truth,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: Some(&mut gate),
            },
        );
        assert_eq!(state.latest_phase, RuntimePhase::StateUnavailable);
        assert!(gate.terminal.contains(&ProviderName::Local));
    }

    #[test]
    fn retry_token_publishes_transition_before_consuming_and_resets_after_success() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Backoff;
        state.retry.attempt_count = 3;
        state.retry.next_at = 99.0;
        let mut store = RetryStore::with_token(RetryToken {
            token_id: "retry".to_owned(),
            desired_fingerprint: state.desired_fingerprint.clone(),
            reason_code: ReasonCode::known("retry-token-requested"),
        });
        let mut sink = VecEventSink::default();
        coordinator.handle_retry_token(now(7.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(store.calls, ["read", "publish", "consume"]);
        assert_eq!(store.published, [RuntimePhase::Observing]);
        assert_eq!(state.retry.attempt_count, 0);
        assert_eq!(state.next_truth_at, 7.0);
    }

    #[test]
    fn retry_token_consume_conflict_keeps_published_transition_without_reset() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Backoff;
        state.retry.attempt_count = 3;
        let mut store = RetryStore::with_token(RetryToken {
            token_id: "retry".to_owned(),
            desired_fingerprint: state.desired_fingerprint.clone(),
            reason_code: ReasonCode::known("retry-token-requested"),
        });
        store.consume_error = Some(RuntimeStoreError::Conflict);
        let mut sink = VecEventSink::default();
        coordinator.handle_retry_token(now(7.0), &mut state, &mut store, &mut sink, None);
        assert_eq!(state.latest_phase, RuntimePhase::Observing);
        assert_eq!(state.retry.attempt_count, 3);
        assert_ne!(state.latest_phase, RuntimePhase::StateUnavailable);
    }

    #[test]
    fn retry_token_is_consumed_once_across_two_reconcile_passes() {
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("test");
        let mut state = state_for(ProviderName::Local);
        state.latest_phase = RuntimePhase::Backoff;
        let mut store = RetryStore::with_token(RetryToken {
            token_id: "retry".to_owned(),
            desired_fingerprint: state.desired_fingerprint.clone(),
            reason_code: ReasonCode::known("retry-token-requested"),
        });
        let workers = RecordingWorkers::default();
        let mut truth_worker = workers.clone();
        let mut lifecycle = workers.clone();
        let mut probe = workers.clone();
        let mut sink = VecEventSink::default();
        coordinator.reconcile(
            now(0.0),
            &mut state,
            &mut vec![],
            &mut ReconcileContext {
                truth: &mut truth_worker,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        coordinator.reconcile(
            now(1.0),
            &mut state,
            &mut vec![],
            &mut ReconcileContext {
                truth: &mut truth_worker,
                lifecycle: &mut lifecycle,
                probe: &mut probe,
                store: &mut store,
                sink: &mut sink,
                gate: None,
            },
        );
        assert_eq!(
            store
                .calls
                .iter()
                .filter(|call| **call == "consume")
                .count(),
            1
        );
        assert_eq!(
            store.calls.iter().filter(|call| **call == "read").count(),
            2
        );
    }
}
