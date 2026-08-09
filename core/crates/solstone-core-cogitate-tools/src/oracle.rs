// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value};

const ORACLE_JSON: &str = include_str!("../../../fixtures/cogitate_oracle.json");
static ORACLE: OnceLock<OracleFixture> = OnceLock::new();

pub(crate) fn fixture() -> &'static OracleFixture {
    ORACLE.get_or_init(|| serde_json::from_str(ORACLE_JSON).expect("cogitate oracle fixture"))
}

#[derive(Deserialize)]
pub(crate) struct OracleFixture {
    pub read_tools: Vec<ReadToolVector>,
    pub bed_manifest: BedManifest,
    pub read_tool_limits: Value,
    pub refusal_strings: Map<String, Value>,
}
#[derive(Deserialize)]
pub(crate) struct BedManifest {
    pub entries: Vec<Value>,
}
#[derive(Deserialize)]
pub(crate) struct ReadToolVector {
    pub id: String,
    pub tool: String,
    pub args: Map<String, Value>,
    pub expect: ReadExpect,
}
#[derive(Deserialize)]
pub(crate) struct ReadExpect {
    pub ok: bool,
    pub refusal: Option<String>,
    pub truncated: bool,
    pub notice: Option<String>,
    pub payload: Value,
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
            let zero = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let one = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(zero)
                .wrapping_add(words[index - 7])
                .wrapping_add(one);
        }
        let mut work = state;
        for (index, constant) in SHA256_CONSTANTS.iter().enumerate() {
            let one = work[4].rotate_right(6) ^ work[4].rotate_right(11) ^ work[4].rotate_right(25);
            let choice = (work[4] & work[5]) ^ (!work[4] & work[6]);
            let first = work[7]
                .wrapping_add(one)
                .wrapping_add(choice)
                .wrapping_add(*constant)
                .wrapping_add(words[index]);
            let zero =
                work[0].rotate_right(2) ^ work[0].rotate_right(13) ^ work[0].rotate_right(22);
            let majority = (work[0] & work[1]) ^ (work[0] & work[2]) ^ (work[1] & work[2]);
            let second = zero.wrapping_add(majority);
            work = [
                first.wrapping_add(second),
                work[0],
                work[1],
                work[2],
                work[3].wrapping_add(first),
                work[4],
                work[5],
                work[6],
            ];
        }
        for (target, value) in state.iter_mut().zip(work) {
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
