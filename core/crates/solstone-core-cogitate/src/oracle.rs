// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

const ORACLE_JSON: &str = include_str!("../../../fixtures/cogitate_oracle.json");
const GENERATED_CONTRACT_JSON: &str = include_str!("../../../fixtures/cogitate_contract.json");

static ORACLE: OnceLock<OracleFixture> = OnceLock::new();
static GENERATED_CONTRACT: OnceLock<Value> = OnceLock::new();

pub(crate) fn fixture() -> &'static OracleFixture {
    ORACLE.get_or_init(|| {
        serde_json::from_str(ORACLE_JSON).expect("core/fixtures/cogitate_oracle.json must be valid")
    })
}

pub(crate) fn generated_contract_fixture() -> &'static Value {
    GENERATED_CONTRACT.get_or_init(|| {
        serde_json::from_str(GENERATED_CONTRACT_JSON)
            .expect("core/fixtures/cogitate_contract.json must be valid")
    })
}

pub(crate) fn assert_preamble(actual: &str, expected: &PreambleFixture, name: &str) {
    assert_eq!(actual, expected.text, "{name} text");
    assert_eq!(actual.len(), expected.byte_length, "{name} byte length");
    assert_eq!(
        sha256_hex(actual.as_bytes()),
        expected.digest,
        "{name} digest"
    );
    assert_eq!(expected.algorithm, "sha256", "{name} algorithm");
    assert_eq!(expected.encoding, "utf-8", "{name} encoding");
}

pub(crate) fn sha256_hex(input: &[u8]) -> String {
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_length = (input.len() as u64).wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes(chunk[index * 4..index * 4 + 4].try_into().expect("word"));
        }
        for index in 16..64 {
            let sigma0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let sigma1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(sigma0)
                .wrapping_add(words[index - 7])
                .wrapping_add(sigma1);
        }

        let mut working = state;
        for (index, constant) in SHA256_CONSTANTS.iter().enumerate() {
            let sigma1 = working[4].rotate_right(6)
                ^ working[4].rotate_right(11)
                ^ working[4].rotate_right(25);
            let choice = (working[4] & working[5]) ^ (!working[4] & working[6]);
            let temporary1 = working[7]
                .wrapping_add(sigma1)
                .wrapping_add(choice)
                .wrapping_add(*constant)
                .wrapping_add(words[index]);
            let sigma0 = working[0].rotate_right(2)
                ^ working[0].rotate_right(13)
                ^ working[0].rotate_right(22);
            let majority =
                (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
            let temporary2 = sigma0.wrapping_add(majority);
            working = [
                temporary1.wrapping_add(temporary2),
                working[0],
                working[1],
                working[2],
                working[3].wrapping_add(temporary1),
                working[4],
                working[5],
                working[6],
            ];
        }
        for (target, value) in state.iter_mut().zip(working) {
            *target = target.wrapping_add(value);
        }
    }

    state.iter().map(|word| format!("{word:08x}")).collect()
}

const SHA256_CONSTANTS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[derive(Deserialize)]
pub(crate) struct OracleFixture {
    pub access_tiers: AccessTiersFixture,
    pub expects_emit_final: Vec<FinalizationVector>,
    pub failure_caps: Vec<FailureCapVector>,
    pub policy_commands: Vec<PolicyCommandVector>,
    pub preambles: PreamblesFixture,
    pub prompt_assembly: Vec<PromptAssemblyVector>,
    pub read_scope: Vec<ReadScopeVector>,
    pub vocabularies: VocabulariesFixture,
}

#[derive(Deserialize)]
pub(crate) struct VocabulariesFixture {
    pub journal_commands: Vec<String>,
    pub finalization_modes: Vec<String>,
    pub deterministic_failure_reason_codes: Vec<String>,
    pub deterministic_failure_caps: BTreeMap<String, usize>,
}

#[derive(Deserialize)]
pub(crate) struct AccessTiersFixture {
    pub tiers: Vec<String>,
    pub talent_tiers: Vec<String>,
    pub future_tiers: Vec<String>,
    pub capabilities: BTreeMap<String, CapabilityFixture>,
    pub unknown_tier: BTreeMap<String, UnknownTierFixture>,
    pub tiers_with_reads_and_submit: Vec<String>,
    pub submit_tiers: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct CapabilityFixture {
    pub sol: bool,
    pub reads: bool,
    pub submit: bool,
}

#[derive(Deserialize)]
pub(crate) struct UnknownTierFixture {
    pub raises: bool,
    pub error: String,
}

#[derive(Deserialize)]
pub(crate) struct FinalizationVector {
    pub id: String,
    pub config: serde_json::Map<String, Value>,
    pub expect: bool,
}

#[derive(Deserialize)]
pub(crate) struct PromptAssemblyVector {
    pub id: String,
    pub config: serde_json::Map<String, Value>,
    pub sol_tool_name: Option<String>,
    pub diagnostic: bool,
    pub expect: PromptAssemblyExpectation,
}

#[derive(Deserialize)]
pub(crate) struct PromptAssemblyExpectation {
    pub prompt_body: Option<String>,
    pub system_instruction: PromptSystemInstructionFixture,
}

#[derive(Deserialize)]
pub(crate) struct PromptSystemInstructionFixture {
    pub parts: Vec<PromptPartFixture>,
    pub order: Option<Vec<String>>,
    pub separator: String,
    pub byte_length: Option<usize>,
    pub sha256: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PromptPartFixture {
    pub role: String,
    pub text: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ReadScopeVector {
    pub id: String,
    pub talent_config: serde_json::Map<String, Value>,
    pub day: String,
    pub span: i64,
    pub expect: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct FailureCapVector {
    pub id: String,
    pub reason_code: Option<String>,
    pub count: usize,
    pub expect: bool,
}

#[derive(Deserialize)]
pub(crate) struct PolicyCommandVector {
    pub id: String,
    pub command: String,
    pub access_tier: String,
    pub outbound_approval: Option<String>,
    pub expect: CommandExpectation,
}

#[derive(Deserialize)]
pub(crate) struct CommandExpectation {
    pub allowed: bool,
    pub reason: String,
    pub argv: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub(crate) struct PreamblesFixture {
    pub runtime: PreambleFixture,
    pub diagnostic: PreambleFixture,
}

#[derive(Deserialize)]
pub(crate) struct PreambleFixture {
    pub algorithm: String,
    pub encoding: String,
    pub byte_length: usize,
    pub digest: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        COGITATE_JOURNAL_COMMANDS, DETERMINISTIC_FAILURE_CAPS, DETERMINISTIC_FAILURE_REASON_CODES,
        TALENT_FINALIZATION_MODES,
    };

    #[test]
    fn vocabularies_match_the_owned_contract_constants() {
        let fixture = fixture();
        assert_eq!(
            fixture.vocabularies.journal_commands,
            COGITATE_JOURNAL_COMMANDS.map(str::to_owned)
        );
        assert_eq!(
            fixture.vocabularies.finalization_modes,
            TALENT_FINALIZATION_MODES.map(str::to_owned)
        );
        assert_eq!(
            fixture.vocabularies.deterministic_failure_reason_codes,
            DETERMINISTIC_FAILURE_REASON_CODES.map(str::to_owned)
        );
        let expected = DETERMINISTIC_FAILURE_CAPS
            .into_iter()
            .map(|(reason, cap)| (reason.to_owned(), cap))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(fixture.vocabularies.deterministic_failure_caps, expected);
    }
}
