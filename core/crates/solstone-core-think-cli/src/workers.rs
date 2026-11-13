// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_local::LocalEndpointResolution;

/// The bundled engine's serving capacity. Linux reads it with the engine's
/// per-launch key; Windows keeps its single-slot sizing.
pub(crate) fn bundled_slots(journal: &Path) -> Option<u32> {
    let mut authority = if cfg!(target_os = "linux") {
        solstone_core_callosum::local_inference_refresh(journal).map(|refresh| {
            solstone_core_local::LocalInferenceAuthority::new(0, 0, String::new(), Some(refresh))
        })
    } else {
        None
    };
    Some(solstone_core_local::bundled_parallel_slots_with_authority(
        journal,
        authority.as_mut(),
    ))
}

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
