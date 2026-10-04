// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Production PCR pin policy for SPP composite attestation.

use crate::snp::{PcrMode, Policy};

// The current engine stays admitted during the sealed-appliance overlap.
// Its fingerprint was captured live on 2026-07-24 and matched across two
// fresh RA-TLS sessions. The sealed image's two fingerprints were qualified
// on 2026-10-03/04, one per Azure firmware state, with reboot-identical quotes.
// Each pin's authenticated manifests and status mode live in nvgpu/rims.rs.
pub const PRODUCTION_PCR_SHA256_PINS: &[&str] = &[
    "b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3",
    "78d2cb684535a82591ef69490542ee9f1501b463675523ac76c834413e616180",
    "96e66fc57838c29daa2b2f3b9301f5aeed3a9285fea2b4439053a7c7c7908952",
];

/// Returns the pinned production policy with all non-PCR policy defaults intact.
pub fn production_policy() -> Policy {
    Policy {
        pcr_mode: PcrMode::Pin,
        pcr_pins: PRODUCTION_PCR_SHA256_PINS
            .iter()
            .map(|pin| (*pin).to_owned())
            .collect(),
        ..Policy::default()
    }
}

#[cfg(test)]
mod tests {
    use super::production_policy;
    use crate::snp::PcrMode;

    #[test]
    fn production_policy_pins_the_production_fingerprint() {
        let policy = production_policy();

        assert_eq!(policy.pcr_mode, PcrMode::Pin);
    }

    #[test]
    fn production_policy_constructs_independent_pin_sets() {
        let mut first = production_policy();
        let second = production_policy();
        first.pcr_pins.insert("different".to_owned());

        assert!(!second.pcr_pins.contains("different"));
    }
}
