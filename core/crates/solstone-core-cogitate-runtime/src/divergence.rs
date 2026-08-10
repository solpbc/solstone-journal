// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A documented structural adaptation from the Python reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeDivergence {
    pub case: &'static str,
    pub reference: &'static str,
    pub native: &'static str,
}

/// Generate-wire deliberately exposes normalized token counts rather than an
/// SDK price accumulator. Therefore native always uses the reference fallback
/// formula from `openhands.py:744-756`; it has no provider price-table path.
pub const DIVERGENCES: &[RuntimeDivergence] = &[RuntimeDivergence {
    case: "cost_pricing",
    reference: "use accumulated SDK cost when positive, otherwise token fallback",
    native: "always use token fallback because no SDK price accumulator exists",
}];
