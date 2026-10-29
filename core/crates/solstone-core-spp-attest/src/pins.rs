// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Production PCR and firmware-signer pin policy for SPP composite attestation.

use std::collections::BTreeMap;

use crate::{
    pcr_appraisal::{ApplicationExpectations, ApplicationPcrPolicy, ApplicationPcrProfiles},
    snp::{PcrMode, Policy},
};

// Image 4 retains both reboot-qualified firmware states for the transition.
// Image 5 was reboot-qualified on 2026-10-29 in the post-roll state only.
// An unqualified prediction never enters this admission set.
// Each pin's authenticated manifests and status mode live in nvgpu/rims.rs.
pub const PRODUCTION_PCR_SHA256_PINS: &[&str] = &[
    "84edaf3d0205a8280068ab485bf45edfc81f371ab7a7dcccaef8538728ccd8a3",
    "0486d5a350467cfa28dea41076659debee9c2fea8c6423828efb53270cbb4641",
    "8779bff6e5cd2c9eadc41098fb073a425654bc10d505b4257125b750cd25eebf",
];

// The key that signs Azure's confidential-VM firmware (the paravisor that
// implements the vTPM behind every PCR). The PCR pins say what the engine
// booted; this pin says whose firmware measured it. Every Azure SNP report
// held on 2026-10-05 carries this one digest, across every launch value seen
// (aa7c9da5…, 11920f61…, af9e20e1…), so a firmware roll keeps it. A re-key
// fails closed until a release adds the new digest.
pub const PRODUCTION_ID_KEY_DIGEST_PINS: &[[u8; 48]] = &[decode_hex(
    "942fd93ebde6ea7a96efadeafc60f1c6b3d10e703b1dafd7555b92f7f3d32d0e006767648cba5b102af3d65756af4177",
)];

/// The register selection the published engine build's quote carries.
pub const PUBLISHED_QUOTE_PCR_SELECTION: [u32; 14] =
    [0, 2, 4, 7, 8, 9, 11, 12, 13, 14, 15, 16, 22, 23];

// The application registers of the published engine build, appraised one by
// one on top of the fingerprint pins. Each value is carried by the build's
// public transparency record (transparency.solstone.app/software/spp/8e4291d/):
// 4, 9 and 11-15 by derived-pcrs.json, and 7 by pre-roll-expected-pcrs.json and
// post-roll-expected-pcrs.json, which agree on it across both accepted firmware
// states. The structural registers 8, 16, 22 and 23 those two files publish are
// the fixed values pcr_appraisal checks.
pub type ApplicationPcrValues = [(u32, [u8; 32]); 8];

pub const IMAGE_4_APPLICATION_PCRS: ApplicationPcrValues = [
    (
        4,
        decode_hex("fd9521ba7ed6f4ebc8ec7befecc3f6f6e3564d432b64dd24a4639858633084e7"),
    ),
    (
        7,
        decode_hex("6ea866422436b46a50e6f2313be40c782a57aee0c71833f9f3c0b6ccfb67c73d"),
    ),
    (
        9,
        decode_hex("b9bf575bb2d456503a816de4fc4a49d78bcc6cdbdad0c2c832f74d90c21dd9e6"),
    ),
    (
        11,
        decode_hex("a9057c8445f8e6e8bcae5dae1b81ac06d9b6eb01968880f56a3c2357add4b679"),
    ),
    (12, [0u8; 32]),
    (13, [0u8; 32]),
    (14, [0u8; 32]),
    (
        15,
        decode_hex("ee22ecf6651b12ade6fd7ab54a8743ea2606e8ce130446ff86792fdb55d3f0e6"),
    ),
];

// Complete application registers for conf-proc 9968c21. The qualified
// post-roll fingerprint selects this vector as one indivisible policy.
pub const IMAGE_5_APPLICATION_PCRS: ApplicationPcrValues = [
    (
        4,
        decode_hex("9559fd2056f187bb0d13d0de5e131bf3a4df94dbdaa9d5b991725ae8adbde852"),
    ),
    (
        7,
        decode_hex("6ea866422436b46a50e6f2313be40c782a57aee0c71833f9f3c0b6ccfb67c73d"),
    ),
    (
        9,
        decode_hex("b7a989edefac316f7ea2bea1eba878baedcd4b0c3ea67d1e13df70911ff932be"),
    ),
    (
        11,
        decode_hex("3215d3f6decf54daa36dcf6a625829feb506e74ad9d4cfd9a0d5081bad3d5fc9"),
    ),
    (12, [0u8; 32]),
    (13, [0u8; 32]),
    (14, [0u8; 32]),
    (
        15,
        decode_hex("ee22ecf6651b12ade6fd7ab54a8743ea2606e8ce130446ff86792fdb55d3f0e6"),
    ),
];

/// Each admitted fingerprint and its complete published register values.
pub const PUBLISHED_APPLICATION_PCR_PROFILES: &[(&str, &ApplicationPcrValues)] = &[
    (
        "8779bff6e5cd2c9eadc41098fb073a425654bc10d505b4257125b750cd25eebf",
        &IMAGE_5_APPLICATION_PCRS,
    ),
    (
        "84edaf3d0205a8280068ab485bf45edfc81f371ab7a7dcccaef8538728ccd8a3",
        &IMAGE_4_APPLICATION_PCRS,
    ),
    (
        "0486d5a350467cfa28dea41076659debee9c2fea8c6423828efb53270cbb4641",
        &IMAGE_4_APPLICATION_PCRS,
    ),
];

/// Complete register policies for the admitted published builds.
pub fn published_application_pcr_profiles() -> ApplicationPcrProfiles {
    ApplicationPcrProfiles::from_profiles(
        PUBLISHED_APPLICATION_PCR_PROFILES
            .iter()
            .map(|(pin, registers)| {
                let digests: BTreeMap<u32, [u8; 32]> = registers.iter().copied().collect();
                (
                    (*pin).to_owned(),
                    ApplicationPcrPolicy {
                        selection: PUBLISHED_QUOTE_PCR_SELECTION.to_vec(),
                        expectations: ApplicationExpectations::new(digests).expect(
                            "the published application registers are exactly the appraised set",
                        ),
                    },
                )
            })
            .collect(),
    )
}

/// Returns the pinned production policy with all other policy defaults intact.
///
/// A quote must match a fingerprint pin and, register by register, the
/// published build.
pub fn production_policy() -> Policy {
    Policy {
        pcr_mode: PcrMode::Pin,
        pcr_pins: PRODUCTION_PCR_SHA256_PINS
            .iter()
            .map(|pin| (*pin).to_owned())
            .collect(),
        id_key_digests: Some(PRODUCTION_ID_KEY_DIGEST_PINS.iter().copied().collect()),
        application_pcrs: Some(published_application_pcr_profiles()),
        ..Policy::default()
    }
}

/// Decodes a lowercase hex constant at compile time.
const fn decode_hex<const N: usize>(hex: &str) -> [u8; N] {
    const fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("pin is not lowercase hex"),
        }
    }
    let bytes = hex.as_bytes();
    assert!(bytes.len() == 2 * N, "pin has the wrong length");
    let mut digest = [0u8; N];
    let mut index = 0;
    while index < N {
        digest[index] = (nibble(bytes[2 * index]) << 4) | nibble(bytes[2 * index + 1]);
        index += 1;
    }
    digest
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{PRODUCTION_ID_KEY_DIGEST_PINS, PUBLISHED_QUOTE_PCR_SELECTION, production_policy};
    use crate::{
        error::{ApplicationPcrError, QuotePcrsError},
        pcr_appraisal::check_application_pcrs,
        snp::{PcrMode, check_pcr_fingerprint},
        test_support::fixture_bytes,
    };

    const PUBLISHED_BUILD_QUOTES: &[&str] = &[
        "sealed-pre-roll-quote.pcrs",
        "sealed-post-roll-quote.pcrs",
        "image5-post-roll-quote.pcrs",
    ];

    /// Where register `position` of the selection keeps its digest in a
    /// quote.pcrs file: two TPML_DIGEST lists of up to eight sized slots after
    /// the selection header.
    fn digest_offset(position: usize) -> usize {
        let (list_start, slot) = if position < 8 {
            (140, position)
        } else {
            (672, position - 8)
        };
        list_start + slot * 66 + 2
    }

    #[test]
    fn production_admits_each_qualified_quote_with_its_complete_profile() {
        let policy = production_policy();
        let profiles = policy
            .application_pcrs
            .as_ref()
            .expect("production appraises registers");
        let mut covered = BTreeSet::new();
        for name in PUBLISHED_BUILD_QUOTES {
            let quote = fixture_bytes(name);
            let fingerprint = check_pcr_fingerprint(&quote, &policy).expect("qualified quote");
            assert_eq!(profiles.check(&quote, &fingerprint), Ok(()), "{name}");
            covered.insert(fingerprint);
        }
        assert_eq!(covered, policy.pcr_pins);
    }

    #[test]
    fn qualified_images_cannot_borrow_each_others_application_profile() {
        let policy = production_policy();
        let profiles = policy.application_pcrs.as_ref().unwrap();
        let old = fixture_bytes("sealed-post-roll-quote.pcrs");
        let new = fixture_bytes("image5-post-roll-quote.pcrs");
        let old_pin = check_pcr_fingerprint(&old, &policy).unwrap();
        let new_pin = check_pcr_fingerprint(&new, &policy).unwrap();
        assert_eq!(
            profiles.check(&old, &new_pin),
            Err(ApplicationPcrError::Register { pcr: 4 })
        );
        assert_eq!(
            profiles.check(&new, &old_pin),
            Err(ApplicationPcrError::Register { pcr: 4 })
        );
        let mut mixed = new.clone();
        let offset = digest_offset(5); // PCR 9
        mixed[offset..offset + 32].copy_from_slice(&old[offset..offset + 32]);
        assert!(check_pcr_fingerprint(&mixed, &policy).is_err());
        assert_eq!(
            profiles.check(&mixed, &new_pin),
            Err(ApplicationPcrError::Register { pcr: 9 })
        );
    }

    #[test]
    fn each_appraised_register_wrong_alone_is_refused_with_its_own_reason() {
        let policy = production_policy();
        let application = policy
            .application_pcrs
            .as_ref()
            .expect("production appraises registers")
            .select(super::PRODUCTION_PCR_SHA256_PINS[0])
            .expect("image-4 profile");
        let published = fixture_bytes("sealed-post-roll-quote.pcrs");
        let mut reasons = BTreeSet::new();
        for (position, pcr) in PUBLISHED_QUOTE_PCR_SELECTION.into_iter().enumerate() {
            let mut quote = published.clone();
            quote[digest_offset(position)] ^= 0x01;
            let result = check_application_pcrs(&quote, application);
            if pcr == 0 || pcr == 2 {
                // The platform registers stay bound by the fingerprint alone.
                assert_eq!(result, Ok(()), "pcr {pcr}");
                continue;
            }
            let error = result.expect_err("one wrong register refuses");
            assert_eq!(error, ApplicationPcrError::Register { pcr }, "pcr {pcr}");
            assert_eq!(error.reason_code(), format!("pcr_{pcr}_mismatch"));
            reasons.insert(error.reason_code());
        }
        assert_eq!(reasons.len(), 12);
    }

    #[test]
    fn production_refuses_a_quote_without_the_published_register_set() {
        let policy = production_policy();
        let application = policy
            .application_pcrs
            .as_ref()
            .expect("production appraises registers")
            .select(super::PRODUCTION_PCR_SHA256_PINS[0])
            .expect("image-4 profile");
        // The retired engine's quote carries ten registers, not the published
        // fourteen. Both admission layers refuse it independently.
        let overlap = fixture_bytes("quote.pcrs");
        assert!(check_pcr_fingerprint(&overlap, &policy).is_err());
        let error = check_application_pcrs(&overlap, application).expect_err("refused");
        assert_eq!(
            error,
            ApplicationPcrError::Quote(QuotePcrsError::SelectionMismatch)
        );
        assert_eq!(error.reason_code(), "pcr_selection_mismatch");
    }

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
