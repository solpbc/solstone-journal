// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A documented structural adaptation from the Python reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeDivergence {
    pub case: &'static str,
    pub reference: &'static str,
    pub native: &'static str,
}

/// Dollar estimation and limits are retired. Raw token usage and context/turn
/// limits remain; no pricing fallback is applied.
///
/// The stuck detector's first trip is answered with one warning message rather
/// than ending the run; see `runtime.rs` and `stuck.rs`.
pub const DIVERGENCES: &[RuntimeDivergence] = &[RuntimeDivergence {
    case: "the stuck detector trips",
    reference: "ends the run as agent_stuck on the first trip",
    native: "the first trip in a run, on a text-only turn or on the last call of a turn, is answered with one warning message; the second trip, or a first trip before the last call of a turn, ends the run as agent_stuck",
}];
