// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Portal handoff nonce: one alphabet, one length, one mint.

use getrandom::fill as fill_random;

pub const NONCE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
pub const NONCE_LENGTH_CHARS: usize = 52;

pub fn mint_nonce() -> Result<String, String> {
    let mut bytes = [0_u8; NONCE_LENGTH_CHARS];
    fill_random(&mut bytes).map_err(|_| "system CSPRNG unavailable".to_owned())?;
    Ok(bytes
        .into_iter()
        .map(|byte| NONCE_ALPHABET[(byte as usize) % NONCE_ALPHABET.len()] as char)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_has_the_exact_external_shape() {
        let first = mint_nonce().expect("nonce mints");
        let second = mint_nonce().expect("nonce mints");
        assert_eq!(first.len(), NONCE_LENGTH_CHARS);
        assert!(first.bytes().all(|byte| NONCE_ALPHABET.contains(&byte)));
        assert_ne!(first, second);
    }
}
