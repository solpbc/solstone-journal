// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod bus;
mod config;
mod host;
pub mod receipt;
mod runtime;
mod shutdown;
mod tick;

pub use host::{
    InstallationBindingRefusal, LifecycleBootError, ShutdownCause, SiblingBinaryResolutionError,
    SupervisorBootRefusal, SupervisorHostOutcome, SupervisorSignal, SyncFailureKind, run_hosted,
};
pub use solstone_core_system::lifecycle::ShutdownDisposition;
