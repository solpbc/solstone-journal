// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compile-time embedded verifier for the immutable service-generator corpus.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

pub fn embedded(path: &str) -> Option<&'static [u8]> {
    EMBEDDED
        .iter()
        .find_map(|(candidate, bytes)| (*candidate == path).then_some(*bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn embedded_map() -> BTreeMap<&'static str, &'static [u8]> {
    EMBEDDED.iter().copied().collect()
}
