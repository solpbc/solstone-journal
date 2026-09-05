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
pub const DIVERGENCES: &[RuntimeDivergence] = &[];
