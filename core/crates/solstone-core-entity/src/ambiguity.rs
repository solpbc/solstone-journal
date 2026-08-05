// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};

/// Return the deterministic public identifier for an ambiguity key.
pub fn ambiguity_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex = format!("{digest:x}");
    format!("amb_{}", &hex[..24])
}
