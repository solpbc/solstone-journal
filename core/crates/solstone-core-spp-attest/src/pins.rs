// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Production PCR pin policy for SPP composite attestation.

use crate::snp::{PcrMode, Policy};

// substrate: spp-engine-01 (processing.solstone.app:9443, Azure Standard_NCC40ads_H100_v5)
// pcr_sha256 pin: b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3
// Provenance: captured live from the production substrate, 2026-07-24,
// operator decision record; observed identical across two fresh RA-TLS sessions
// via the journal-side CPU-leg appraisal.
pub const PRODUCTION_PCR_SHA256_PINS: &[&str] =
    &["b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3"];

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
