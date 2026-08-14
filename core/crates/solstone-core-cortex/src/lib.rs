// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Cortex service lifecycle and Callosum adapter.

mod process;
mod renewal;
mod service;
mod state;
mod storage;

pub use service::{CortexOptions, CortexServiceError, ShutdownMode, run_native_service, run_until};
