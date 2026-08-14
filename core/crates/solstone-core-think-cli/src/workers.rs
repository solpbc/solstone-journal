// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_local::LocalEndpointResolution;

pub(crate) fn default_segment_workers(
    cpu_count: Option<usize>,
    uses_local: bool,
    endpoint: LocalEndpointResolution,
    bundled_slots: Option<u32>,
) -> usize {
    let formula = (cpu_count.unwrap_or(2) / 2).clamp(1, 8);
    if !uses_local {
        return formula;
    }
    let slots = match endpoint {
        LocalEndpointResolution::Byo(endpoint) => endpoint.parallel_slots,
        LocalEndpointResolution::Bundled => bundled_slots,
    };
    slots.map_or(formula, |slots| formula.min(slots as usize))
}
