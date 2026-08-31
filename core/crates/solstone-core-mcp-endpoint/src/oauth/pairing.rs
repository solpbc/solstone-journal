// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Crockford base32 pairing-code helpers.

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const PAIRING_CODE_CHARS: usize = 8;

/// Encode 5 random bytes as an 8-character uppercase Crockford base32 code.
pub(crate) fn encode_pairing_code(bytes: &[u8; 5]) -> String {
    let mut accumulator = 0_u64;
    for byte in bytes {
        accumulator = (accumulator << 8) | u64::from(*byte);
    }
    let mut encoded = String::with_capacity(PAIRING_CODE_CHARS);
    for shift in (0..PAIRING_CODE_CHARS).rev() {
        let index = ((accumulator >> (shift * 5)) & 31) as usize;
        encoded.push(char::from(CROCKFORD[index]));
    }
    encoded
}

/// Uppercase and admit an 8-character Crockford pairing code.
///
/// Characters outside the Crockford alphabet (including I, L, O, and U) are
/// rejected rather than mapped.
pub(crate) fn canonicalize_pairing_code(raw: &str) -> Option<String> {
    if raw.len() != PAIRING_CODE_CHARS {
        return None;
    }
    let mut canonical = String::with_capacity(PAIRING_CODE_CHARS);
    for byte in raw.bytes() {
        let upper = byte.to_ascii_uppercase();
        if !CROCKFORD.contains(&upper) {
            return None;
        }
        canonical.push(char::from(upper));
    }
    Some(canonical)
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{canonicalize_pairing_code, encode_pairing_code};

    #[test]
    fn encode_is_eight_uppercase_crockford_characters() {
        let encoded = encode_pairing_code(&[0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(encoded, "00000000");
        let encoded = encode_pairing_code(&[0xff, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(encoded.len(), 8);
        assert!(encoded.bytes().all(|byte| super::CROCKFORD.contains(&byte)));
    }

    #[test]
    fn canonicalize_accepts_lowercase_and_rejects_other_bytes() {
        assert_eq!(
            canonicalize_pairing_code("ab12cd3e").as_deref(),
            Some("AB12CD3E")
        );
        assert!(canonicalize_pairing_code("AB12CD3I").is_none());
        assert!(canonicalize_pairing_code("AB12CD3O").is_none());
        assert!(canonicalize_pairing_code("AB12CD3U").is_none());
        assert!(canonicalize_pairing_code("AB12CD3-").is_none());
        assert!(canonicalize_pairing_code("SHORT").is_none());
        assert!(canonicalize_pairing_code("TOOLONG01").is_none());
    }
}
