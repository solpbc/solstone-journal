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
// RunningUse: shared LaunchAuthority from CortexState::running for RunningUsesGuard / immediate-stop
pub use crate::process::{spawn_one, stop_group_with_grace};
pub use crate::state::{CortexState, RunningUse, Work};
pub use crate::storage::CortexStore;

pub fn new_state(store: CortexStore) -> CortexState {
    let (spawn_tx, _) = mpsc::channel();
    let (cancel_tx, _) = mpsc::channel();
    let (outbound_tx, _) = mpsc::channel();
    CortexState::new(store, spawn_tx, cancel_tx, outbound_tx)
}

/// Retain the queued work receiver so component tests can drive lifecycle transitions.
pub fn queued_state(store: CortexStore) -> (CortexState, mpsc::Receiver<Work>) {
    let (spawn_tx, spawn_rx) = mpsc::channel();
    let (cancel_tx, _) = mpsc::channel();
    let (outbound_tx, _) = mpsc::channel();
    (
        CortexState::new(store, spawn_tx, cancel_tx, outbound_tx),
        spawn_rx,
    )
}

pub fn cancel_use(state: &CortexState, use_id: &str, reason: &str) -> Option<RunningUse> {
    state.cancel_running(use_id, reason)
}

pub fn append_event(
    state: &CortexState,
    use_id: &str,
    active: &std::path::Path,
    event: serde_json::Map<String, serde_json::Value>,
) {
    state.append_and_relay(use_id, active, event);
}

pub fn recover_store(store: &CortexStore) {
    store.recover().expect("recover component fixture");
}
