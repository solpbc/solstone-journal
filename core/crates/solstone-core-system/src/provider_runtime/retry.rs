// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Launch and cleanup retry cadence helpers.

use super::events::{ProviderRuntimeEvent, ProviderRuntimeEventSink};
use super::model::{
    PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS, PROVIDER_RETRY_SCHEDULE_SECONDS, ProviderRuntimeNow,
    ProviderRuntimeState, RuntimePhase,
};

pub fn schedule_launch_retry(
    state: &mut ProviderRuntimeState,
    now: ProviderRuntimeNow,
    sink: &mut dyn ProviderRuntimeEventSink,
) -> bool {
    if state.retry.attempt_count as usize >= PROVIDER_RETRY_SCHEDULE_SECONDS.len() {
        state.latest_phase = RuntimePhase::Failed;
        sink.emit(ProviderRuntimeEvent::RetryExhausted {
            provider: state.provider,
        });
        return false;
    }
    let delay = PROVIDER_RETRY_SCHEDULE_SECONDS[state.retry.attempt_count as usize];
    state.retry.next_at = now.monotonic_seconds + delay;
    state.latest_phase = RuntimePhase::Backoff;
    sink.emit(ProviderRuntimeEvent::RetryScheduled {
        provider: state.provider,
    });
    true
}

pub fn schedule_cleanup_retry(
    state: &mut ProviderRuntimeState,
    now: ProviderRuntimeNow,
    sink: &mut dyn ProviderRuntimeEventSink,
) {
    state.cleanup_attempt_count += 1;
    let index = (state.cleanup_attempt_count as usize - 1)
        .min(PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS.len() - 1);
    state.cleanup_next_at = now.monotonic_seconds + PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS[index];
    state.latest_phase = RuntimePhase::CleanupFailed;
    sink.emit(ProviderRuntimeEvent::CleanupRetry {
        provider: state.provider,
    });
}

pub fn retry_token_phase(phase: RuntimePhase) -> RuntimePhase {
    match phase {
        RuntimePhase::NotDesired
        | RuntimePhase::ArtifactNotReady
        | RuntimePhase::HostBlocked
        | RuntimePhase::Stopped
        | RuntimePhase::Failed
        | RuntimePhase::Backoff
        | RuntimePhase::Observing => RuntimePhase::Observing,
        _ => RuntimePhase::RetryRequested,
    }
}
