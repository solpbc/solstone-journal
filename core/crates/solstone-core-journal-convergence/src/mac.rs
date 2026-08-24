// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Domain-separated HMAC-SHA256. Prefix is NUL-terminated ASCII, then payload.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::digest::hex_encode;

/// HMAC-SHA256(key, prefix || payload) as lowercase hex.
///
/// `prefix` is a domain-separation label and must include its trailing NUL.
pub(crate) fn hmac_hex(key: &[u8], prefix: &[u8], payload: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(prefix);
    mac.update(payload);
    hex_encode(&mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::hmac_hex;

    const KEY: [u8; 32] = [1; 32];

    #[test]
    fn known_answer_is_deterministic() {
        let first = hmac_hex(&KEY, b"test-domain\0", b"payload");
        let second = hmac_hex(&KEY, b"test-domain\0", b"payload");
        assert_eq!(
            first,
            "2c2e67ef7c170d5a9311f946fe14f7c84591387e88909aabd0dfd9cc8eda8872"
        );
        assert_eq!(first, second);
    }

    #[test]
    fn domain_prefix_changes_output() {
        let left = hmac_hex(&KEY, b"test-domain\0", b"payload");
        let right = hmac_hex(&KEY, b"other-domain\0", b"payload");
        assert_eq!(
            right,
            "804ffc278039698bb1108f1909b5bf64f041815c978f0472c4b45cef9f3258d9"
        );
        assert_ne!(left, right);
    }
}
