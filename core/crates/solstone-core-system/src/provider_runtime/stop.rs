// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stop requests, their distinct outcomes, and replacement convergence.

use super::model::{
    ManagedProcess, ProviderRuntimeState, ProviderStopCleanupRequest, ReasonCode, RuntimePhase,
};

pub fn make_stop_request(
    state: &ProviderRuntimeState,
    managed: ManagedProcess,
    reason_code: ReasonCode,
    target_phase: RuntimePhase,
    target_reason_code: Option<ReasonCode>,
    admission_exclusive: bool,
) -> ProviderStopCleanupRequest {
    let _ = state;
    ProviderStopCleanupRequest {
        managed,
        reason_code,
        target_phase,
        target_reason_code,
        admission_exclusive,
        orphaned_start_outcome: false,
    }
}

/// Deferred target stop: the phase announces desired state while the old process remains owned.
pub fn defer_target_stop(
    state: &mut ProviderRuntimeState,
    target_phase: RuntimePhase,
    target_reason: Option<ReasonCode>,
    admission_exclusive: bool,
) {
    if let Some(request) = state.pending_stop_request.as_mut() {
        request.target_phase = target_phase;
        request.target_reason_code = target_reason.clone();
    }
    state.pending_stop_target_phase = target_phase;
    state.pending_stop_target_reason_code = target_reason;
    state.pending_stop_admission_exclusive = admission_exclusive;
    state.latest_phase = RuntimePhase::StopDeferred;
}

/// Retain a handle returned by a start worker after that outcome is no longer acceptable.
pub(super) fn queue_orphaned_start_cleanup(
    state: &mut ProviderRuntimeState,
    managed: ManagedProcess,
    target_phase: RuntimePhase,
    target_reason_code: Option<ReasonCode>,
) {
    let mut request = make_stop_request(
        state,
        managed,
        ReasonCode::known("launch-failed"),
        target_phase,
        target_reason_code,
        false,
    );
    request.orphaned_start_outcome = true;
    state.orphaned_stop_requests.push(request);
}

pub fn duplicate_owned_process_request(
    state: &ProviderRuntimeState,
    processes: &[ManagedProcess],
) -> Option<ProviderStopCleanupRequest> {
    let mut candidates = processes.iter().filter(|process| process.running).cloned();
    let first = candidates.next()?;
    let stale = candidates.next()?;
    let _ = first;
    Some(make_stop_request(
        state,
        stale,
        ReasonCode::known("duplicate-owned-process"),
        state.latest_phase,
        Some(ReasonCode::known("duplicate-owned-process")),
        false,
    ))
}

pub fn stop_before_replace_request(
    state: &ProviderRuntimeState,
    processes: &[ManagedProcess],
) -> Option<ProviderStopCleanupRequest> {
    if !state.has_plan
        || !matches!(
            state.latest_phase,
            RuntimePhase::Starting
                | RuntimePhase::Backoff
                | RuntimePhase::RetryRequested
                | RuntimePhase::Stopped
        )
    {
        return None;
    }
    let process = processes.iter().find(|process| process.running)?.clone();
    Some(make_stop_request(
        state,
        process,
        ReasonCode::known("target-changed"),
        RuntimePhase::Stopped,
        Some(ReasonCode::known("cleanup-succeeded")),
        false,
    ))
}

pub fn cancel_start(state: &mut ProviderRuntimeState) {
    if state.start.is_some() {
        state.start_cancelled = true;
    }
}
pub fn cancel_stop(state: &mut ProviderRuntimeState) {
    if state.stop_cleanup.is_some() {
        state.stop_cancelled = true;
    }
}
