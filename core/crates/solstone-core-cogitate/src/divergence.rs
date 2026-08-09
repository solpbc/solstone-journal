// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::oracle::sha256_hex;

#[derive(Clone, Copy)]
pub(crate) struct PreambleFingerprint {
    pub byte_length: usize,
    pub sha256: &'static str,
}

#[derive(Clone, Copy)]
pub(crate) struct PreambleDivergence {
    pub case: &'static str,
    pub old: PreambleFingerprint,
    pub new: PreambleFingerprint,
    pub citation: &'static str,
}

pub(crate) const DIVERGENCES: &[PreambleDivergence] = &[PreambleDivergence {
    case: "runtime_preamble",
    old: PreambleFingerprint {
        byte_length: 1989,
        sha256: "6614e3fd060f29e7c3cb0ed063e43e370a7efefd2579974080fc8caec65ca591",
    },
    new: PreambleFingerprint {
        byte_length: 2163,
        sha256: "39011e2c0c5b2f144b082aaae9a9a5a564571b6346412902b8c349b41f67a75f",
    },
    citation: "2026-08-09 operator ruling: the raw-read tier's runtime preamble corrects its broad-root framing to match glob/grep_search/list_directory's actual broad_root refusal rule; the frozen v3 oracle (core/fixtures/cogitate_oracle.json) retains the pre-correction text by design.",
}];

pub(crate) fn check_divergence(
    case: &str,
    ledger: &[PreambleDivergence],
    left: &str,
    right: &str,
) -> Result<(), String> {
    let Some(divergence) = ledger.iter().find(|entry| entry.case == case) else {
        return (left == right)
            .then_some(())
            .ok_or_else(|| format!("unrecorded divergence for {case}"));
    };
    if left == right {
        return Err(format!(
            "stale divergence entry for {case}: {}",
            divergence.citation
        ));
    }
    check_fingerprint("left", case, left, divergence.old, divergence.citation)?;
    check_fingerprint("right", case, right, divergence.new, divergence.citation)
}

fn check_fingerprint(
    side: &str,
    case: &str,
    value: &str,
    expected: PreambleFingerprint,
    citation: &str,
) -> Result<(), String> {
    let actual_length = value.len();
    let actual_sha256 = sha256_hex(value.as_bytes());
    (actual_length == expected.byte_length && actual_sha256 == expected.sha256)
        .then_some(())
        .ok_or_else(|| {
            format!("{side} fingerprint does not match divergence entry for {case}: {citation}")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divergence_check_rejects_unrecorded_and_stale_entries() {
        assert!(check_divergence("unrecorded_case", DIVERGENCES, "old", "new").is_err());

        let mut ledger = DIVERGENCES.to_vec();
        ledger.push(PreambleDivergence {
            case: "temporary_probe_case",
            old: PreambleFingerprint {
                byte_length: 1,
                sha256: "old",
            },
            new: PreambleFingerprint {
                byte_length: 1,
                sha256: "new",
            },
            citation: "test-only stale-entry probe",
        });
        assert!(check_divergence("temporary_probe_case", &ledger, "same", "same").is_err());
    }
}
