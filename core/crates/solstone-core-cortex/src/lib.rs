// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Cortex service lifecycle and Callosum adapter.

mod process;
mod renewal;
mod service;
mod state;
mod storage;

pub use service::{
    CortexOptions, CortexServiceError, ShutdownMode, run_native_service,
    run_native_service_with_hosted_parent, run_until,
};

// Gated on `test-hooks` as well as `test` so `tests/cortex_child_supervisor.rs`
// and `tests/bin/controller.rs` can drive crate-private spawn/stop surfaces.
// `cfg(test)`-only would leave those items unreachable outside this crate even
// for a harness that asked for them by name.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod test_hooks;
