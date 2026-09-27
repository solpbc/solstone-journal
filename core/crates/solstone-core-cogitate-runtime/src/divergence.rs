// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A documented structural adaptation from the Python reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeDivergence {
    pub case: &'static str,
    pub reference: &'static str,
    pub native: &'static str,
}

/// Adaptations of this runtime from the reference cogitate runtime; each entry
/// records the case, the reference behavior, and the native behavior.
pub const DIVERGENCES: &[RuntimeDivergence] = &[
    RuntimeDivergence {
        case: "the stuck detector trips",
        reference: "ends the run as agent_stuck on the first trip",
        native: "the first trip in a run, on a text-only turn or on the last call of a turn, is answered with one warning message; the second trip, or a first trip before the last call of a turn, ends the run as agent_stuck",
    },
    RuntimeDivergence {
        case: "the run nears its wall-clock deadline",
        reference: "gives the model no time warning and ends the run at the deadline",
        native: "publishes one warning at 70% of wall_clock_deadline(), once, after that turn's tools, and has no time final-turn or force-stop",
    },
];
