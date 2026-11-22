// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The key a space credential is bound to, and the HTTP message signature
//! (RFC 9421) a member presents with the credential when it reads a writer's
//! records in the space.

use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};

const SIGNATURE_INPUT: &str =
    r#"("authorization" "atproto-space-audience");tag="atproto-space-credential""#;

/// A one-use P-256 keypair a space credential is bound to.
pub struct CredentialKey {
    key_pair: EcdsaKeyPair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialKeyError;

impl std::fmt::Display for CredentialKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("failed to make or use a space credential key")
    }
}

impl std::error::Error for CredentialKeyError {}

impl CredentialKey {
    pub fn generate() -> Result<Self, CredentialKeyError> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| CredentialKeyError)?;
        let key_pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                .map_err(|_| CredentialKeyError)?;
        Ok(Self { key_pair })
    }

    /// The key as a `did:key`: multicodec `p256-pub` (0x1200) over the
    /// compressed point, base58btc.
    pub fn did_key(&self) -> String {
        did_key_for_uncompressed(self.key_pair.public_key().as_ref())
    }

    /// Headers that present `credential` to `audience` (the DID of the
    /// account whose repo is read), signed with this key.
    pub fn signed_headers(
        &self,
        credential: &str,
        audience: &str,
    ) -> Result<Vec<(String, String)>, CredentialKeyError> {
        let authorization = format!("Atproto-Space-Credential {credential}");
        let base = signature_base(&authorization, audience);
        let signature = self
            .key_pair
            .sign(&SystemRandom::new(), base.as_bytes())
            .map_err(|_| CredentialKeyError)?;
        Ok(vec![
            ("Authorization".to_owned(), authorization),
            ("Atproto-Space-Audience".to_owned(), audience.to_owned()),
            (
                "Signature-Input".to_owned(),
                format!("sig={SIGNATURE_INPUT}"),
            ),
            (
                "Signature".to_owned(),
                format!("sig=:{}:", base64_padded(signature.as_ref())),
            ),
        ])
    }

    #[cfg(test)]
    fn public_key(&self) -> &[u8] {
        self.key_pair.public_key().as_ref()
    }
}

fn signature_base(authorization: &str, audience: &str) -> String {
    format!(
        "\"authorization\": {}\n\"atproto-space-audience\": {}\n\"@signature-params\": {SIGNATURE_INPUT}",
        authorization.trim(),
        audience.trim()
    )
}

fn did_key_for_uncompressed(point: &[u8]) -> String {
    let mut bytes = vec![0x80, 0x24];
    let y_odd = point[64] & 1 == 1;
    bytes.push(if y_odd { 0x03 } else { 0x02 });
    bytes.extend_from_slice(&point[1..33]);
    format!("did:key:z{}", base58btc(&bytes))
}

fn base58btc(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut digits: Vec<u8> = Vec::new();
    for &byte in input {
        let mut carry = u32::from(byte);
        for digit in digits.iter_mut() {
            carry += u32::from(*digit) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let zeros = input.iter().take_while(|&&b| b == 0).count();
    std::iter::repeat_n(b'1', zeros)
        .chain(digits.iter().rev().map(|&d| ALPHABET[d as usize]))
        .map(char::from)
        .collect()
}

fn base64_padded(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[((n >> (18 - 6 * index)) & 63) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;

    #[test]
    fn base58_and_base64_match_known_vectors() {
        assert_eq!(base58btc(b"hello world"), "StV1DL6CwTryKyV");
        assert_eq!(base58btc(&[0, 0, 1]), "112");
        assert_eq!(
            base64_padded(b"any carnal pleas"),
            "YW55IGNhcm5hbCBwbGVhcw=="
        );
        assert_eq!(
            base64_padded(b"any carnal pleasu"),
            "YW55IGNhcm5hbCBwbGVhc3U="
        );
    }

    #[test]
    fn a_p256_did_key_has_the_p256_prefix() {
        // P-256 did:keys over a compressed point always start `did:key:zDn`.
        let key = CredentialKey::generate().unwrap();
        assert!(
            key.did_key().starts_with("did:key:zDn"),
            "{}",
            key.did_key()
        );
    }

    #[test]
    fn the_signature_covers_the_credential_and_audience() {
        let key = CredentialKey::generate().unwrap();
        let headers = key.signed_headers("cred.jwt", "did:plc:alice").unwrap();
        let get = |name: &str| headers.iter().find(|(k, _)| k == name).unwrap().1.clone();
        assert_eq!(get("Authorization"), "Atproto-Space-Credential cred.jwt");
        assert_eq!(
            get("Signature-Input"),
            r#"sig=("authorization" "atproto-space-audience");tag="atproto-space-credential""#
        );
        let encoded = get("Signature");
        let encoded = encoded
            .strip_prefix("sig=:")
            .unwrap()
            .strip_suffix(':')
            .unwrap();
        let signature = decode_base64(encoded);
        let base = signature_base("Atproto-Space-Credential cred.jwt", "did:plc:alice");
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key.public_key())
            .verify(base.as_bytes(), &signature)
            .expect("signature verifies over the RFC 9421 base");
    }

    fn decode_base64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut count = 0;
        let mut out = Vec::new();
        for byte in text.bytes().filter(|&b| b != b'=') {
            bits = (bits << 6) | ALPHABET.iter().position(|&a| a == byte).unwrap() as u32;
            count += 6;
            if count >= 8 {
                count -= 8;
                out.push((bits >> count) as u8);
            }
        }
        out
    }
}
