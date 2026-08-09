// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Provider runtime reconciliation primitives without supervisor wiring.

mod events;
mod gate;
mod model;
mod reconcile;
mod retry;
mod seams;
mod stop;
mod wedge;

pub use events::{ProviderRuntimeEvent, ProviderRuntimeEventSink, VecEventSink};
pub use gate::ProviderStartupGate;
pub use model::*;
pub use reconcile::ProviderRuntimeCoordinator;
pub use retry::{schedule_cleanup_retry, schedule_launch_retry};
pub use seams::{
    InMemoryRuntimeStore, LifecycleSeam, NoopWorkers, ProbeSeam, RetryToken, RuntimeStore,
    RuntimeStoreError, TruthObservationSeam,
};
pub use stop::{
    cancel_start, cancel_stop, defer_target_stop, duplicate_owned_process_request,
    stop_before_replace_request,
};
pub use wedge::{CortexEventKind, CortexOutcomeEvent, WedgeState};
