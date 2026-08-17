// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Gated on `test-hooks` as well as `test` so `tests/cortex_child_supervisor.rs`
// and `tests/bin/controller.rs` can drive crate-private spawn/stop surfaces.
// `cfg(test)`-only would leave those items unreachable outside this crate even
// for a harness that asked for them by name.

#![doc(hidden)]

use std::sync::mpsc;

// spawn_one: sibling-selection, three cwd cases (via controller), stdin-failure/reap
// stop_group_with_grace: process-group cleanup, graceful stop, forced cleanup, immediate stop
// CortexState: every spawn_one case plus drain/immediate-stop harness
// CortexStore: claim + journal path those cases need
// Work: construct the spawn request those cases pass in
// RunningUse: pgid from CortexState::running for RunningUsesGuard / immediate-stop
pub use crate::process::{spawn_one, stop_group_with_grace};
pub use crate::state::{CortexState, RunningUse, Work};
pub use crate::storage::CortexStore;

pub fn new_state(store: CortexStore) -> CortexState {
    let (spawn_tx, _) = mpsc::channel();
    let (cancel_tx, _) = mpsc::channel();
    let (outbound_tx, _) = mpsc::channel();
    CortexState::new(store, spawn_tx, cancel_tx, outbound_tx)
}
