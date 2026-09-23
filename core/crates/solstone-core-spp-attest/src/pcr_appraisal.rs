// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Application PCR appraisal for SPP composite attestation.
//!
//! This API is dormant; wiring it into the enforced appraisal path is a separate
//! gated cutover (G13). PCRs 0 and 2 are the platform half and are intentionally
//! not appraised here (G13/G16).

use std::collections::BTreeMap;

use crate::{
    error::{ApplicationExpectationsError, QuotePcrsError},
    tpm_quote::parse_quote_pcr_file,
};

const TPM_ALG_SHA256: u16 = 0x000b;

const STRUCTURAL_PCR_ZERO_8: [u8; 32] = [0u8; 32];
const STRUCTURAL_PCR_ZERO_16: [u8; 32] = [0u8; 32];
const STRUCTURAL_PCR_ZERO_23: [u8; 32] = [0u8; 32];
const STRUCTURAL_PCR_FF_22: [u8; 32] = [0xff; 32];

const APPLICATION_PCR_KEYS: [u32; 8] = [4, 7, 9, 11, 12, 13, 14, 15];
const APPRAISAL_ORDER: [u32; 12] = [4, 7, 8, 9, 11, 12, 13, 14, 15, 16, 22, 23];

/// Reason why an individual PCR appraisal check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcrAppraisalReason {
    StructuralMismatch,
    ExpectationMismatch,
    RegisterAbsent,
}

/// A failure record for an individual PCR appraisal check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcrAppraisalFailure {
    pub pcr: u32,
    pub reason: PcrAppraisalReason,
    pub expected: [u8; 32],
    pub observed: Option<[u8; 32]>,
}

/// Verified application PCR expectations for appraisal.
pub struct ApplicationExpectations {
    digests: BTreeMap<u32, [u8; 32]>,
}

impl ApplicationExpectations {
    /// Creates a verified application expectations container.
    pub fn new(digests: BTreeMap<u32, [u8; 32]>) -> Result<Self, ApplicationExpectationsError> {
        if digests.keys().copied().eq(APPLICATION_PCR_KEYS) {
            Ok(Self { digests })
        } else {
            Err(ApplicationExpectationsError::KeySet)
        }
    }
}

/// Parses a quote.pcrs buffer and verifies that its selection matches the expected list.
pub fn parse_quote_pcrs(
    quote_pcrs: &[u8],
    selection: &[u32],
) -> Result<BTreeMap<u32, [u8; 32]>, QuotePcrsError> {
    let pcr_file = parse_quote_pcr_file(quote_pcrs).map_err(|_| QuotePcrsError::Structure)?;

    if pcr_file.selections.len() != 1 || pcr_file.selections[0].hash_alg != TPM_ALG_SHA256 {
        return Err(QuotePcrsError::NotSingleSha256Bank);
    }

    let mut indices = Vec::new();
    for (i, &byte) in pcr_file.selections[0].pcr_select.iter().enumerate() {
        for b in 0..8 {
            if byte & (1 << b) != 0 {
                indices.push((i * 8 + b) as u32);
            }
        }
    }

    if indices.len() != pcr_file.digest_buffers.len() {
        return Err(QuotePcrsError::Structure);
    }

    if indices.as_slice() != selection {
        return Err(QuotePcrsError::SelectionMismatch);
    }

    let mut map = BTreeMap::new();
    for (idx, digest) in indices.into_iter().zip(pcr_file.digest_buffers) {
        map.insert(idx, digest);
    }

    Ok(map)
}

/// Appraises observed PCR digests against application expectations and structural invariants.
pub fn appraise_application_pcrs(
    observed: &BTreeMap<u32, [u8; 32]>,
    expected: &ApplicationExpectations,
) -> Result<(), Vec<PcrAppraisalFailure>> {
    let mut failures = Vec::new();

    for &pcr in &APPRAISAL_ORDER {
        match pcr {
            8 | 16 | 23 => {
                let structural = match pcr {
                    8 => STRUCTURAL_PCR_ZERO_8,
                    16 => STRUCTURAL_PCR_ZERO_16,
                    23 => STRUCTURAL_PCR_ZERO_23,
                    _ => unreachable!(),
                };
                match observed.get(&pcr) {
                    Some(obs) => {
                        if *obs != structural {
                            failures.push(PcrAppraisalFailure {
                                pcr,
                                reason: PcrAppraisalReason::StructuralMismatch,
                                expected: structural,
                                observed: Some(*obs),
                            });
                        }
                    }
                    None => {
                        failures.push(PcrAppraisalFailure {
                            pcr,
                            reason: PcrAppraisalReason::RegisterAbsent,
                            expected: structural,
                            observed: None,
                        });
                    }
                }
            }
            22 => {
                let structural = STRUCTURAL_PCR_FF_22;
                match observed.get(&pcr) {
                    Some(obs) => {
                        if *obs != structural {
                            failures.push(PcrAppraisalFailure {
                                pcr,
                                reason: PcrAppraisalReason::StructuralMismatch,
                                expected: structural,
                                observed: Some(*obs),
                            });
                        }
                    }
                    None => {
                        failures.push(PcrAppraisalFailure {
                            pcr,
                            reason: PcrAppraisalReason::RegisterAbsent,
                            expected: structural,
                            observed: None,
                        });
                    }
                }
            }
            4 | 7 | 9 | 11 | 12 | 13 | 14 | 15 => {
                let exp = expected.digests[&pcr];
                match observed.get(&pcr) {
                    Some(obs) => {
                        if *obs != exp {
                            failures.push(PcrAppraisalFailure {
                                pcr,
                                reason: PcrAppraisalReason::ExpectationMismatch,
                                expected: exp,
                                observed: Some(*obs),
                            });
                        }
                    }
                    None => {
                        failures.push(PcrAppraisalFailure {
                            pcr,
                            reason: PcrAppraisalReason::RegisterAbsent,
                            expected: exp,
                            observed: None,
                        });
                    }
                }
            }
            _ => unreachable!(),
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ApplicationExpectations, PcrAppraisalFailure, PcrAppraisalReason,
        appraise_application_pcrs, parse_quote_pcrs,
    };
    use crate::{
        error::{ApplicationExpectationsError, QuotePcrsError},
        test_support::fixture_bytes,
    };

    fn hex_decode_32(s: &str) -> [u8; 32] {
        assert_eq!(s.len(), 64);
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex byte");
        }
        out
    }

    fn fixture_pcr_digests() -> BTreeMap<u32, [u8; 32]> {
        let mut map = BTreeMap::new();
        map.insert(
            0,
            hex_decode_32("13177a6535badf19415c06589705dd5a1890f73545c4a9fef7acfe2c6177a2b7"),
        );
        map.insert(
            2,
            hex_decode_32("3d458cfe55cc03ea1f443f1562beec8df51c75e14a9fcf9a7234a13f198e7969"),
        );
        map.insert(
            4,
            hex_decode_32("c8057ff798ed666ab7728bfa2c43788cc7f464a7be493fe21f8bc975fad726ad"),
        );
        map.insert(
            7,
            hex_decode_32("f014e5cbfa297ee787a976abc51bc1d67e23e1b9e4a60128a1106dd0adef0c5b"),
        );
        map.insert(8, [0u8; 32]);
        map.insert(
            9,
            hex_decode_32("af76fc0fd1295c3528270ff2d634f2ec4c31a5ec085b9eaba6185851f4f0ce9b"),
        );
        map.insert(15, [0u8; 32]);
        map.insert(16, [0u8; 32]);
        map.insert(22, [0xff; 32]);
        map.insert(23, [0u8; 32]);
        map
    }

    const SYNTHETIC_11: [u8; 32] = [0x11; 32];
    const SYNTHETIC_12: [u8; 32] = [0x12; 32];
    const SYNTHETIC_13: [u8; 32] = [0x13; 32];
    const SYNTHETIC_14: [u8; 32] = [0x14; 32];

    const FIXTURE_SELECTION: [u32; 10] = [0, 2, 4, 7, 8, 9, 15, 16, 22, 23];

    #[test]
    fn parse_quote_pcrs_parses_fixture_correctly() {
        let fixture = fixture_bytes("quote.pcrs");
        let parsed =
            parse_quote_pcrs(&fixture, &FIXTURE_SELECTION).expect("fixture parses successfully");
        let expected = fixture_pcr_digests();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn parse_quote_pcrs_rejects_raw_buffer_as_structure_error() {
        let raw_buffer = vec![0u8; FIXTURE_SELECTION.len() * 32];
        let err = parse_quote_pcrs(&raw_buffer, &FIXTURE_SELECTION).unwrap_err();
        assert_eq!(err, QuotePcrsError::Structure);
    }

    #[test]
    fn parse_quote_pcrs_rejects_wrong_selection() {
        let fixture = fixture_bytes("quote.pcrs");
        // Dropped PCR 23
        let dropped = [0, 2, 4, 7, 8, 9, 15, 16, 22];
        assert_eq!(
            parse_quote_pcrs(&fixture, &dropped).unwrap_err(),
            QuotePcrsError::SelectionMismatch
        );

        // Swapped indices 0 and 2
        let swapped = [2, 0, 4, 7, 8, 9, 15, 16, 22, 23];
        assert_eq!(
            parse_quote_pcrs(&fixture, &swapped).unwrap_err(),
            QuotePcrsError::SelectionMismatch
        );
    }

    #[test]
    fn parse_quote_pcrs_parses_synthetic_fourteen_register_blob() {
        // Selection bytes for 14 registers:
        // Byte 0: 0x95 (0, 2, 4, 7)
        // Byte 1: 0xfb (8, 9, 11, 12, 13, 14, 15) -> 1 + 2 + 8 + 16 + 32 + 64 + 128 = 251 = 0xfb
        // Byte 2: 0xc1 (16, 22, 23)
        let select_bytes = [0x95, 0xfb, 0xc1, 0x00, 0x00, 0x00, 0x00, 0x00];
        let selection_14 = [0, 2, 4, 7, 8, 9, 11, 12, 13, 14, 15, 16, 22, 23];

        let mut digests_14 = Vec::new();
        let fix = fixture_pcr_digests();
        digests_14.push(fix[&0]);
        digests_14.push(fix[&2]);
        digests_14.push(fix[&4]);
        digests_14.push(fix[&7]);
        digests_14.push(fix[&8]);
        digests_14.push(fix[&9]);
        digests_14.push(SYNTHETIC_11);
        digests_14.push(SYNTHETIC_12);
        digests_14.push(SYNTHETIC_13);
        digests_14.push(SYNTHETIC_14);
        digests_14.push(fix[&15]);
        digests_14.push(fix[&16]);
        digests_14.push(fix[&22]);
        digests_14.push(fix[&23]);

        assert_eq!(digests_14.len(), 14);

        let mut blob = Vec::new();
        // u32le selection_count = 1
        blob.extend_from_slice(&1_u32.to_le_bytes());
        // Slot 0 active
        blob.extend_from_slice(&0x000b_u16.to_le_bytes()); // hash_alg SHA256
        blob.push(3_u8); // sizeofSelect
        blob.extend_from_slice(&select_bytes);
        blob.extend_from_slice(&[0u8; 5]); // pad
        // Slots 1..7 inactive (16 zero bytes each)
        for _ in 1..8 {
            blob.extend_from_slice(&[0u8; 16]);
        }

        // u32le digest_list_count = 2
        blob.extend_from_slice(&2_u32.to_le_bytes());

        // Digest list 0: count = 8
        blob.extend_from_slice(&8_u32.to_le_bytes());
        for digest in &digests_14[..8] {
            blob.extend_from_slice(&32_u16.to_le_bytes()); // size 32
            blob.extend_from_slice(digest);
            blob.extend_from_slice(&[0u8; 32]); // remaining 32 zero
        }

        // Digest list 1: count = 6
        blob.extend_from_slice(&6_u32.to_le_bytes());
        for digest in &digests_14[8..14] {
            blob.extend_from_slice(&32_u16.to_le_bytes()); // size 32
            blob.extend_from_slice(digest);
            blob.extend_from_slice(&[0u8; 32]); // remaining 32 zero
        }
        // Inactive slots 6 and 7 in list 1
        for _ in 6..8 {
            blob.extend_from_slice(&0_u16.to_le_bytes()); // size 0
            blob.extend_from_slice(&[0u8; 64]);
        }

        let parsed = parse_quote_pcrs(&blob, &selection_14).expect("synthetic 14 blob parses");
        assert_eq!(parsed.len(), 14);
        for (&idx, digest) in selection_14.iter().zip(digests_14) {
            assert_eq!(parsed.get(&idx), Some(&digest));
        }
    }

    fn sample_expectations() -> ApplicationExpectations {
        let fix = fixture_pcr_digests();
        let mut map = BTreeMap::new();
        map.insert(4, fix[&4]);
        map.insert(7, fix[&7]);
        map.insert(9, fix[&9]);
        map.insert(11, SYNTHETIC_11);
        map.insert(12, SYNTHETIC_12);
        map.insert(13, SYNTHETIC_13);
        map.insert(14, SYNTHETIC_14);
        map.insert(15, fix[&15]);
        ApplicationExpectations::new(map).expect("valid expectations")
    }

    fn sample_good_observed() -> BTreeMap<u32, [u8; 32]> {
        let fix = fixture_pcr_digests();
        let mut map = BTreeMap::new();
        map.insert(4, fix[&4]);
        map.insert(7, fix[&7]);
        map.insert(8, [0u8; 32]);
        map.insert(9, fix[&9]);
        map.insert(11, SYNTHETIC_11);
        map.insert(12, SYNTHETIC_12);
        map.insert(13, SYNTHETIC_13);
        map.insert(14, SYNTHETIC_14);
        map.insert(15, fix[&15]);
        map.insert(16, [0u8; 32]);
        map.insert(22, [0xff; 32]);
        map.insert(23, [0u8; 32]);
        map
    }

    #[test]
    fn appraisal_succeeds_on_good_map_and_ignores_platform_pcrs() {
        let expected = sample_expectations();
        let good_map = sample_good_observed();
        assert!(appraise_application_pcrs(&good_map, &expected).is_ok());

        let mut with_platform = good_map;
        with_platform.insert(0, [0xaa; 32]);
        with_platform.insert(2, [0xbb; 32]);
        assert!(appraise_application_pcrs(&with_platform, &expected).is_ok());
    }

    #[test]
    fn appraisal_reports_single_register_mismatch_table_driven() {
        let expected = sample_expectations();
        let base_observed = sample_good_observed();

        let test_registers: [(u32, PcrAppraisalReason); 12] = [
            (4, PcrAppraisalReason::ExpectationMismatch),
            (7, PcrAppraisalReason::ExpectationMismatch),
            (8, PcrAppraisalReason::StructuralMismatch),
            (9, PcrAppraisalReason::ExpectationMismatch),
            (11, PcrAppraisalReason::ExpectationMismatch),
            (12, PcrAppraisalReason::ExpectationMismatch),
            (13, PcrAppraisalReason::ExpectationMismatch),
            (14, PcrAppraisalReason::ExpectationMismatch),
            (15, PcrAppraisalReason::ExpectationMismatch),
            (16, PcrAppraisalReason::StructuralMismatch),
            (22, PcrAppraisalReason::StructuralMismatch),
            (23, PcrAppraisalReason::StructuralMismatch),
        ];

        for (pcr, expected_reason) in test_registers {
            let mut mutated = base_observed.clone();
            let unmutated_val = mutated[&pcr];
            let mut mutated_val = unmutated_val;
            mutated_val[0] ^= 0x01; // flip one byte
            mutated.insert(pcr, mutated_val);

            let err = appraise_application_pcrs(&mutated, &expected).unwrap_err();
            assert_eq!(
                err,
                vec![PcrAppraisalFailure {
                    pcr,
                    reason: expected_reason,
                    expected: unmutated_val,
                    observed: Some(mutated_val),
                }],
                "testing pcr {pcr}"
            );
        }
    }

    #[test]
    fn appraisal_reports_multiple_failures_in_order() {
        let expected = sample_expectations();
        let mut mutated = sample_good_observed();

        let unmutated_4 = mutated[&4];
        let mut mutated_4 = unmutated_4;
        mutated_4[0] ^= 0x01;
        mutated.insert(4, mutated_4);

        let unmutated_22 = mutated[&22];
        let mut mutated_22 = unmutated_22;
        mutated_22[0] ^= 0x01;
        mutated.insert(22, mutated_22);

        let err = appraise_application_pcrs(&mutated, &expected).unwrap_err();
        assert_eq!(
            err,
            vec![
                PcrAppraisalFailure {
                    pcr: 4,
                    reason: PcrAppraisalReason::ExpectationMismatch,
                    expected: unmutated_4,
                    observed: Some(mutated_4),
                },
                PcrAppraisalFailure {
                    pcr: 22,
                    reason: PcrAppraisalReason::StructuralMismatch,
                    expected: unmutated_22,
                    observed: Some(mutated_22),
                },
            ]
        );
    }

    #[test]
    fn appraisal_reports_register_absent() {
        let expected = sample_expectations();
        let base = sample_good_observed();

        // Remove PCR 9
        let mut missing_9 = base.clone();
        missing_9.remove(&9);
        let err9 = appraise_application_pcrs(&missing_9, &expected).unwrap_err();
        assert_eq!(
            err9,
            vec![PcrAppraisalFailure {
                pcr: 9,
                reason: PcrAppraisalReason::RegisterAbsent,
                expected: base[&9],
                observed: None,
            }]
        );

        // Remove PCR 22
        let mut missing_22 = base;
        missing_22.remove(&22);
        let err22 = appraise_application_pcrs(&missing_22, &expected).unwrap_err();
        assert_eq!(
            err22,
            vec![PcrAppraisalFailure {
                pcr: 22,
                reason: PcrAppraisalReason::RegisterAbsent,
                expected: [0xff; 32],
                observed: None,
            }]
        );
    }

    #[test]
    fn application_expectations_constructor_validates_key_set() {
        let fix = fixture_pcr_digests();
        let mut map = BTreeMap::new();
        map.insert(4, fix[&4]);
        map.insert(7, fix[&7]);
        map.insert(9, fix[&9]);
        map.insert(11, SYNTHETIC_11);
        map.insert(12, SYNTHETIC_12);
        map.insert(13, SYNTHETIC_13);
        map.insert(14, SYNTHETIC_14);
        // Missing PCR 15
        assert!(matches!(
            ApplicationExpectations::new(map.clone()),
            Err(ApplicationExpectationsError::KeySet)
        ));

        // Add 15 -> valid
        map.insert(15, fix[&15]);
        assert!(ApplicationExpectations::new(map.clone()).is_ok());

        // Add extra key (e.g. PCR 0 or 8)
        let mut extra = map;
        extra.insert(0, fix[&0]);
        assert!(matches!(
            ApplicationExpectations::new(extra),
            Err(ApplicationExpectationsError::KeySet)
        ));
    }
}
