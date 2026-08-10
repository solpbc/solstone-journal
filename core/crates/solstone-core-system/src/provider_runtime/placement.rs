// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Parakeet GPU auto-placement decision.
//!
//! A faithful, complete port of `parakeet_placement.py`'s
//! `decide_parakeet_auto_placement`: whether co-locating parakeet.cpp on the
//! same discrete GPU the local brain lane already occupies is safe, or
//! whether it must fall back to CPU. `decide_parakeet_auto_placement` has
//! exactly one production caller outside its own module (the supervisor's
//! truth observation) and is not forked across languages the way
//! [`crate::stt_backend_choice`] is, so unlike that decision this one needs
//! no cross-language differential -- it is ported directly and completely.
//!
//! Deliberately does not port `is_discrete`, `discrete_hardware_gpu_count`,
//! or `cpu_placement_suffix`: those depend on `local_vulkan` device
//! enumeration types that are not part of this port, and (`cpu_placement_suffix`
//! specifically) remain used by Python callers outside the supervisor
//! (`solstone/think/check.py`, `solstone/think/providers/fit_report.py`) that
//! this port does not touch. Only the pure arithmetic this module's one
//! caller needs is ported here; the caller is responsible for classifying
//! devices and counting discrete GPUs before calling in.

/// 2947 MiB is the measured peak ordinary segment residency under
/// local-128 attention, not full attention. The 1024 MiB margin covers
/// display framebuffer, compositor allocations, driver overhead, and
/// allocator fragmentation on the same monitor-driving GPU.
pub const PARAKEET_WORST_CASE_MIB: u32 = 2947;
pub const CO_FIT_MARGIN_MIB: u32 = 1024;

const CAPABLE_TIER_MIN_VRAM_MIB: u32 = 16000;

struct ServerTier {
    name: &'static str,
    // `None` on the capable tier is load-bearing: unmeasured residency
    // means the co-location predicate can never fire at >=16 GiB.
    resident_mib: Option<u32>,
}

const CAPABLE_TIER: ServerTier = ServerTier {
    name: "capable",
    resident_mib: None,
};
const FLOOR_TIER: ServerTier = ServerTier {
    name: "floor",
    resident_mib: Some(4147),
};

fn select_server_tier(vram_mib: u32) -> ServerTier {
    if vram_mib >= CAPABLE_TIER_MIN_VRAM_MIB {
        CAPABLE_TIER
    } else {
        FLOOR_TIER
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetPlacementDecision {
    pub force_cpu: bool,
    pub reason_code: &'static str,
    pub tier_name: Option<&'static str>,
    pub tier_resident_mib: Option<u32>,
    pub parakeet_worst_case_mib: u32,
    pub margin_mib: u32,
    pub required_mib: Option<u32>,
    pub vram_mib: Option<u32>,
}

fn decision(
    force_cpu: bool,
    reason_code: &'static str,
    tier_name: Option<&'static str>,
    tier_resident_mib: Option<u32>,
    required_mib: Option<u32>,
    vram_mib: Option<u32>,
) -> ParakeetPlacementDecision {
    ParakeetPlacementDecision {
        force_cpu,
        reason_code,
        tier_name,
        tier_resident_mib,
        parakeet_worst_case_mib: PARAKEET_WORST_CASE_MIB,
        margin_mib: CO_FIT_MARGIN_MIB,
        required_mib,
        vram_mib,
    }
}

/// Returns whether parakeet.cpp auto-placement must use CPU.
///
/// Intentionally pure: the caller has already classified the selected
/// device, counted discrete hardware GPUs, and probed unified-memory and
/// brain-lane state; this function performs only tier selection and
/// arithmetic over those facts.
pub fn decide_parakeet_auto_placement(
    vram_mib: Option<u32>,
    selected_device_is_discrete: bool,
    discrete_hardware_gpu_count: u32,
    unified_memory: bool,
    brain_lane_active: bool,
) -> ParakeetPlacementDecision {
    if !brain_lane_active {
        return decision(false, "brain_lane_inactive", None, None, None, vram_mib);
    }
    if !selected_device_is_discrete {
        return decision(
            false,
            "selected_device_not_discrete",
            None,
            None,
            None,
            vram_mib,
        );
    }
    if discrete_hardware_gpu_count != 1 {
        return decision(
            false,
            "discrete_gpu_count_not_one",
            None,
            None,
            None,
            vram_mib,
        );
    }
    if unified_memory {
        return decision(false, "unified_memory", None, None, None, vram_mib);
    }
    let Some(vram_mib) = vram_mib else {
        return decision(false, "vram_unknown", None, None, None, None);
    };

    let tier = select_server_tier(vram_mib);
    let Some(tier_resident_mib) = tier.resident_mib else {
        return decision(
            false,
            "tier_residency_unmeasured",
            Some(tier.name),
            None,
            None,
            Some(vram_mib),
        );
    };

    let required_mib = tier_resident_mib + PARAKEET_WORST_CASE_MIB + CO_FIT_MARGIN_MIB;
    if vram_mib < required_mib {
        decision(
            true,
            "co_location_requires_cpu",
            Some(tier.name),
            Some(tier_resident_mib),
            Some(required_mib),
            Some(vram_mib),
        )
    } else {
        decision(
            false,
            "co_location_fits_gpu",
            Some(tier.name),
            Some(tier_resident_mib),
            Some(required_mib),
            Some(vram_mib),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brain_lane_inactive_never_forces_cpu_regardless_of_other_facts() {
        let decision = decide_parakeet_auto_placement(Some(1), true, 1, false, false);
        assert!(!decision.force_cpu);
        assert_eq!(decision.reason_code, "brain_lane_inactive");
    }

    #[test]
    fn a_non_discrete_selected_device_never_forces_cpu() {
        let decision = decide_parakeet_auto_placement(Some(24000), false, 1, false, true);
        assert!(!decision.force_cpu);
        assert_eq!(decision.reason_code, "selected_device_not_discrete");
    }

    #[test]
    fn more_than_one_discrete_gpu_never_forces_cpu() {
        let decision = decide_parakeet_auto_placement(Some(24000), true, 2, false, true);
        assert!(!decision.force_cpu);
        assert_eq!(decision.reason_code, "discrete_gpu_count_not_one");
    }

    #[test]
    fn zero_discrete_gpus_never_forces_cpu() {
        let decision = decide_parakeet_auto_placement(Some(24000), true, 0, false, true);
        assert_eq!(decision.reason_code, "discrete_gpu_count_not_one");
    }

    #[test]
    fn unified_memory_never_forces_cpu() {
        let decision = decide_parakeet_auto_placement(Some(24000), true, 1, true, true);
        assert!(!decision.force_cpu);
        assert_eq!(decision.reason_code, "unified_memory");
    }

    #[test]
    fn unknown_vram_never_forces_cpu_and_clears_vram_in_the_decision() {
        let decision = decide_parakeet_auto_placement(None, true, 1, false, true);
        assert!(!decision.force_cpu);
        assert_eq!(decision.reason_code, "vram_unknown");
        assert_eq!(decision.vram_mib, None);
    }

    #[test]
    fn capable_tier_never_forces_cpu_because_residency_is_unmeasured() {
        let decision = decide_parakeet_auto_placement(Some(16000), true, 1, false, true);
        assert!(!decision.force_cpu);
        assert_eq!(decision.reason_code, "tier_residency_unmeasured");
        assert_eq!(decision.tier_name, Some("capable"));
        assert_eq!(decision.tier_resident_mib, None);
    }

    #[test]
    fn floor_tier_with_ample_vram_fits_gpu() {
        // required = 4147 + 2947 + 1024 = 8118
        let decision = decide_parakeet_auto_placement(Some(8118), true, 1, false, true);
        assert!(!decision.force_cpu);
        assert_eq!(decision.reason_code, "co_location_fits_gpu");
        assert_eq!(decision.tier_name, Some("floor"));
        assert_eq!(decision.required_mib, Some(8118));
    }

    #[test]
    fn floor_tier_one_mib_short_of_required_forces_cpu() {
        let decision = decide_parakeet_auto_placement(Some(8117), true, 1, false, true);
        assert!(decision.force_cpu);
        assert_eq!(decision.reason_code, "co_location_requires_cpu");
        assert_eq!(decision.required_mib, Some(8118));
        assert_eq!(decision.vram_mib, Some(8117));
    }

    #[test]
    fn the_capable_tier_threshold_is_exactly_16000_mib() {
        let just_under = decide_parakeet_auto_placement(Some(15999), true, 1, false, true);
        assert_eq!(just_under.tier_name, Some("floor"));
        let exactly = decide_parakeet_auto_placement(Some(16000), true, 1, false, true);
        assert_eq!(exactly.tier_name, Some("capable"));
    }
}
