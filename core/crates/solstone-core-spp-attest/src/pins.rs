// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Production PCR and firmware-signer pin policy for SPP composite attestation.

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

// The key that signs Azure's confidential-VM firmware (the paravisor that
// implements the vTPM behind every PCR). The PCR pins say what the engine
// booted; this pin says whose firmware measured it. Every Azure SNP report
// held on 2026-10-05 carries this one digest, across every launch value seen
// (aa7c9da5…, 11920f61…, af9e20e1…), so a firmware roll keeps it. A re-key
// fails closed until a release adds the new digest.
pub const PRODUCTION_ID_KEY_DIGEST_PINS: &[[u8; 48]] = &[decode_sha384_hex(
    "942fd93ebde6ea7a96efadeafc60f1c6b3d10e703b1dafd7555b92f7f3d32d0e006767648cba5b102af3d65756af4177",
)];

/// Returns the pinned production policy with all other policy defaults intact.
pub fn production_policy() -> Policy {
    Policy {
        pcr_mode: PcrMode::Pin,
        pcr_pins: PRODUCTION_PCR_SHA256_PINS
            .iter()
            .map(|pin| (*pin).to_owned())
            .collect(),
        id_key_digests: Some(PRODUCTION_ID_KEY_DIGEST_PINS.iter().copied().collect()),
        ..Policy::default()
    }
}

/// Decodes a lowercase SHA-384 hex constant at compile time.
const fn decode_sha384_hex(hex: &str) -> [u8; 48] {
    const fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("pin is not lowercase hex"),
        }
    }
    let bytes = hex.as_bytes();
    assert!(bytes.len() == 96, "pin is not a SHA-384 digest");
    let mut digest = [0u8; 48];
    let mut index = 0;
    while index < 48 {
        digest[index] = (nibble(bytes[2 * index]) << 4) | nibble(bytes[2 * index + 1]);
        index += 1;
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::{PRODUCTION_ID_KEY_DIGEST_PINS, production_policy};
    use crate::snp::PcrMode;

    #[test]
    fn production_policy_pins_the_production_fingerprint() {
        let policy = production_policy();

        assert_eq!(policy.pcr_mode, PcrMode::Pin);
    }

    #[test]
    fn production_policy_pins_exactly_the_azure_firmware_signer() {
        let policy = production_policy();
        let pinned = policy.id_key_digests.expect("production pins the ID key");

        assert_eq!(pinned.len(), 1);
        assert_eq!(PRODUCTION_ID_KEY_DIGEST_PINS.len(), 1);
        assert_eq!(
            crate::snp::hex_lower(&PRODUCTION_ID_KEY_DIGEST_PINS[0]),
            "942fd93ebde6ea7a96efadeafc60f1c6b3d10e703b1dafd7555b92f7f3d32d0e006767648cba5b102af3d65756af4177"
        );
        assert!(pinned.contains(&PRODUCTION_ID_KEY_DIGEST_PINS[0]));
        assert!(!pinned.contains(&[0; 48]));
    }

    #[test]
    fn production_policy_constructs_independent_pin_sets() {
        let mut first = production_policy();
        let second = production_policy();
        first.pcr_pins.insert("different".to_owned());

        assert!(!second.pcr_pins.contains("different"));
    }
}
