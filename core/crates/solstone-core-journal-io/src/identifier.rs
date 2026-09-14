// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Identifier minting. Callers own any durable write of the returned value.

use std::error::Error;
use std::fmt;

/// Failure while minting an identifier.
#[derive(Debug)]
pub enum IdentifierMintError {
    /// The operating system could not provide entropy.
    Entropy(getrandom::Error),
}

impl fmt::Display for IdentifierMintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Entropy(error) => {
                write!(
                    formatter,
                    "could not obtain random bytes for identifier: {error}"
                )
            }
        }
    }
}

impl Error for IdentifierMintError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Entropy(_) => None,
        }
    }
}

/// Return whether `id` is a canonical lowercase RFC 4122 UUIDv4 string.
pub fn is_uuid_v4(id: &str) -> bool {
    if id.len() != 36 {
        return false;
    }
    let bytes = id.as_bytes();
    if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
        return false;
    }
    if bytes[14] != b'4' {
        return false;
    }
    if !matches!(bytes[19], b'8' | b'9' | b'a' | b'b') {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        if i == 8 || i == 13 || i == 18 || i == 23 {
            continue;
        }
        if !b.is_ascii_hexdigit() || b.is_ascii_uppercase() {
            return false;
        }
    }
    true
}

/// Mint one random RFC 4122 UUIDv4 identifier string.
pub fn mint_uuid_v4() -> Result<String, IdentifierMintError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(IdentifierMintError::Entropy)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

#[cfg(test)]
mod tests {
    use super::{is_uuid_v4, mint_uuid_v4};

    #[test]
    fn minted_identifier_is_well_formed_and_not_constant() {
        let first = mint_uuid_v4().expect("entropy");
        let second = mint_uuid_v4().expect("entropy");
        assert!(is_uuid_v4(&first), "{first}");
        assert!(is_uuid_v4(&second), "{second}");
        assert_ne!(first, second);
    }

    #[test]
    fn uuid_v4_shape_rejects_malformed_values() {
        assert!(is_uuid_v4("a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"));
        assert!(is_uuid_v4("00000000-0000-4000-8000-000000000000"));
        assert!(is_uuid_v4("ffffffff-ffff-4fff-bfff-ffffffffffff"));
        assert!(!is_uuid_v4("a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5"));
        assert!(!is_uuid_v4("a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5de"));
        assert!(!is_uuid_v4("a1b2c3d4-e5f6-1a7b-8c9d-0e1f2a3b4c5d"));
        assert!(!is_uuid_v4("a1b2c3d4-e5f6-5a7b-8c9d-0e1f2a3b4c5d"));
        assert!(!is_uuid_v4("a1b2c3d4-e5f6-4a7b-0c9d-0e1f2a3b4c5d"));
        assert!(!is_uuid_v4("a1b2c3d4-e5f6-4a7b-7c9d-0e1f2a3b4c5d"));
        assert!(!is_uuid_v4("a1b2c3d4-e5f6-4a7b-cc9d-0e1f2a3b4c5d"));
        assert!(!is_uuid_v4("A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D"));
        assert!(!is_uuid_v4("g1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"));
    }
}
