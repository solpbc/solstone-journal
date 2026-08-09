// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Provider runtime reconciliation primitives without supervisor wiring.

mod admission;
mod events;
mod gate;
mod launch;
mod model;
mod parakeet;
mod parakeet_truth;
mod placement;
mod reconcile;
mod retry;
mod seams;
mod stop;
mod store;
mod wedge;

pub use admission::{
    AvailableBytesReader, ParakeetAdmissionInput, ParakeetAdmissionLatch, admission_retry_epoch,
    bump_admission_retry_epoch, parakeet_stt_admission_latch,
};
pub use events::{ProviderRuntimeEvent, ProviderRuntimeEventSink, VecEventSink};
pub use gate::ProviderStartupGate;
pub use launch::{
    LocalLaunchCommon, LocalLaunchConfig, LocalLifecycleSeam, LocalProbeSeam, LocalTruthConfig,
    LocalTruthSeam, ReservedPort,
};
pub use model::*;
pub use parakeet::{
    PARAKEET_SERVER_PROCESS_NAME, ParakeetLaunchConfig, ParakeetLifecycleSeam, ParakeetPlacement,
    ParakeetProbeSeam, ParakeetRuntimeShared,
};
pub use parakeet_truth::{
    admission_blocked_observation, admission_not_desired_observation, parakeet_platform_can_host,
    platform_cannot_host_not_desired, remote_mode_not_desired,
};
pub use placement::{
    CO_FIT_MARGIN_MIB, PARAKEET_WORST_CASE_MIB, ParakeetPlacementDecision,
    decide_parakeet_auto_placement,
};
pub use reconcile::{ProviderRuntimeCoordinator, ReconcileContext};
pub use retry::{schedule_cleanup_retry, schedule_launch_retry};
pub use seams::{
    InMemoryRuntimeStore, LifecycleSeam, NoopWorkers, ProbeSeam, RetryToken, RuntimeStore,
    RuntimeStoreError, TruthObservationSeam,
};
pub use stop::{
    cancel_start, cancel_stop, defer_target_stop, duplicate_owned_process_request,
    stop_before_replace_request,
};
pub use store::{
    FenceKey, FileRuntimeStore, LocalReadySideEffect, LocalRuntimeShared, ReadyProcess,
    ReadyProcessLookup, RuntimeClock, SystemRuntimeClock, read_current_detail,
};
pub use wedge::{CortexEventKind, CortexOutcomeEvent, WedgeState, observe_cortex_outcome};
