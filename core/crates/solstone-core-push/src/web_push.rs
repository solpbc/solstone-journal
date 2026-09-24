// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Web Push RFC 8291 message encryption.

use hkdf::Hkdf;
use p256::PublicKey;
#[cfg(test)]
use p256::SecretKey;
use p256::ecdh::EphemeralSecret;
use p256::elliptic_curve::rand_core;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::Sha256;

pub(crate) const PADDING_DELIMITER: u8 = 0x02;
const RECORD_SIZE: u32 = 4096;
const KEY_ID_LEN: u8 = 65;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WebPushError {
    InvalidKey,
    CryptoError,
}

impl std::fmt::Display for WebPushError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKey => formatter.write_str("invalid web push receiver public key"),
            Self::CryptoError => formatter.write_str("web push encryption failed"),
        }
    }
}

impl std::error::Error for WebPushError {}

/// Encrypt a plaintext message for Web Push using RFC 8291 `aes128gcm` with freshly generated
/// ephemeral key and salt.
pub(crate) fn encrypt_web_push(
    receiver_p256dh: &[u8; 65],
    receiver_auth: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>, WebPushError> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| WebPushError::CryptoError)?;

    let ephemeral_secret = EphemeralSecret::random(&mut OsRngAdapter(&rng));
    let ephemeral_public = ephemeral_secret.public_key();
    let as_public = ephemeral_public.to_encoded_point(false);
    let as_public_bytes: [u8; 65] = as_public
        .as_bytes()
        .try_into()
        .map_err(|_| WebPushError::CryptoError)?;

    let ua_public =
        PublicKey::from_sec1_bytes(receiver_p256dh).map_err(|_| WebPushError::InvalidKey)?;

    let shared_secret = ephemeral_secret.diffie_hellman(&ua_public);
    let ecdh_secret_bytes = shared_secret.raw_secret_bytes();

    encrypt_with_ecdh_and_salt(
        ecdh_secret_bytes,
        receiver_p256dh,
        &as_public_bytes,
        receiver_auth,
        &salt,
        plaintext,
    )
}

struct OsRngAdapter<'a>(&'a SystemRandom);

impl rand_core::CryptoRng for OsRngAdapter<'_> {}
impl rand_core::RngCore for OsRngAdapter<'_> {
    fn next_u32(&mut self) -> u32 {
        let mut buf = [0u8; 4];
        self.0.fill(&mut buf).expect("system random");
        u32::from_le_bytes(buf)
    }

    fn next_u64(&mut self) -> u64 {
        let mut buf = [0u8; 8];
        self.0.fill(&mut buf).expect("system random");
        u64::from_le_bytes(buf)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill(dest).expect("system random");
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0
            .fill(dest)
            .map_err(|_| rand_core::Error::from(core::num::NonZeroU32::new(1).unwrap()))
    }
}

fn encrypt_with_ecdh_and_salt(
    ecdh_secret_bytes: &[u8],
    ua_public: &[u8; 65],
    as_public: &[u8; 65],
    auth_secret: &[u8; 16],
    salt: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>, WebPushError> {
    // HKDF-Extract(salt=auth, IKM=ecdh)
    let hk_auth = Hkdf::<Sha256>::new(Some(auth_secret), ecdh_secret_bytes);

    // HKDF-Expand with info "WebPush: info\0" || ua_public || as_public to 32 bytes (IKM)
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);

    let mut ikm = [0u8; 32];
    hk_auth
        .expand(&key_info, &mut ikm)
        .map_err(|_| WebPushError::CryptoError)?;

    // HKDF-Extract(salt=salt, IKM=ikm)
    let hk_salt = Hkdf::<Sha256>::new(Some(salt), &ikm);

    let mut cek = [0u8; 16];
    hk_salt
        .expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| WebPushError::CryptoError)?;

    let mut nonce = [0u8; 12];
    hk_salt
        .expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| WebPushError::CryptoError)?;

    let mut record_plaintext = Vec::with_capacity(plaintext.len() + 1);
    record_plaintext.extend_from_slice(plaintext);
    record_plaintext.push(PADDING_DELIMITER);

    let unbound_key =
        UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| WebPushError::CryptoError)?;
    let less_safe_key = LessSafeKey::new(unbound_key);
    let aead_nonce = Nonce::assume_unique_for_key(nonce);

    let mut ciphertext = record_plaintext;
    less_safe_key
        .seal_in_place_append_tag(aead_nonce, Aad::empty(), &mut ciphertext)
        .map_err(|_| WebPushError::CryptoError)?;

    // Header: salt(16) || rs(4 u32 BE) || idlen(1 u8) || keyid(65)
    let mut header = Vec::with_capacity(16 + 4 + 1 + 65 + ciphertext.len());
    header.extend_from_slice(salt);
    header.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    header.push(KEY_ID_LEN);
    header.extend_from_slice(as_public);
    header.extend_from_slice(&ciphertext);

    Ok(header)
}

#[cfg(test)]
pub(crate) fn encrypt_web_push_with_params(
    receiver_p256dh: &[u8; 65],
    receiver_auth: &[u8; 16],
    ephemeral_private: &[u8; 32],
    salt: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>, WebPushError> {
    let ephemeral_secret =
        SecretKey::from_slice(ephemeral_private).map_err(|_| WebPushError::InvalidKey)?;
    let ephemeral_public = ephemeral_secret.public_key();
    let as_public = ephemeral_public.to_encoded_point(false);
    let as_public_bytes: [u8; 65] = as_public
        .as_bytes()
        .try_into()
        .map_err(|_| WebPushError::CryptoError)?;

    let ua_public =
        PublicKey::from_sec1_bytes(receiver_p256dh).map_err(|_| WebPushError::InvalidKey)?;

    let shared_secret =
        p256::ecdh::diffie_hellman(ephemeral_secret.to_nonzero_scalar(), ua_public.as_affine());
    let ecdh_secret_bytes = shared_secret.raw_secret_bytes();

    encrypt_with_ecdh_and_salt(
        ecdh_secret_bytes,
        receiver_p256dh,
        &as_public_bytes,
        receiver_auth,
        salt,
        plaintext,
    )
}

#[cfg(test)]
pub(crate) fn decrypt_web_push_for_test(
    receiver_private_bytes: &[u8; 32],
    receiver_auth: &[u8; 16],
    record: &[u8],
) -> Result<Vec<u8>, WebPushError> {
    if record.len() < 16 + 4 + 1 + 65 + 16 {
        return Err(WebPushError::CryptoError);
    }
    let salt: [u8; 16] = record[0..16].try_into().unwrap();
    let rs = u32::from_be_bytes(record[16..20].try_into().unwrap());
    if rs != RECORD_SIZE {
        return Err(WebPushError::CryptoError);
    }
    let idlen = record[20];
    if idlen != KEY_ID_LEN {
        return Err(WebPushError::CryptoError);
    }
    let as_public: [u8; 65] = record[21..21 + 65].try_into().unwrap();
    let mut ciphertext_and_tag = record[21 + 65..].to_vec();

    let receiver_secret =
        SecretKey::from_slice(receiver_private_bytes).map_err(|_| WebPushError::InvalidKey)?;
    let receiver_public = receiver_secret.public_key();
    let ua_public = receiver_public.to_encoded_point(false);
    let ua_public_bytes: [u8; 65] = ua_public
        .as_bytes()
        .try_into()
        .map_err(|_| WebPushError::CryptoError)?;

    let as_pub_key =
        PublicKey::from_sec1_bytes(&as_public).map_err(|_| WebPushError::InvalidKey)?;

    let shared_secret =
        p256::ecdh::diffie_hellman(receiver_secret.to_nonzero_scalar(), as_pub_key.as_affine());
    let ecdh_secret_bytes = shared_secret.raw_secret_bytes();

    let hk_auth = Hkdf::<Sha256>::new(Some(receiver_auth), ecdh_secret_bytes);
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(&ua_public_bytes);
    key_info.extend_from_slice(&as_public);

    let mut ikm = [0u8; 32];
    hk_auth
        .expand(&key_info, &mut ikm)
        .map_err(|_| WebPushError::CryptoError)?;

    let hk_salt = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    hk_salt
        .expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| WebPushError::CryptoError)?;

    let mut nonce = [0u8; 12];
    hk_salt
        .expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| WebPushError::CryptoError)?;

    let unbound_key =
        UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| WebPushError::CryptoError)?;
    let less_safe_key = LessSafeKey::new(unbound_key);
    let aead_nonce = Nonce::assume_unique_for_key(nonce);

    let decrypted = less_safe_key
        .open_in_place(aead_nonce, Aad::empty(), &mut ciphertext_and_tag)
        .map_err(|_| WebPushError::CryptoError)?;

    let mut end = decrypted.len();
    while end > 0 && decrypted[end - 1] == 0x00 {
        end -= 1;
    }
    if end == 0 || decrypted[end - 1] != PADDING_DELIMITER {
        return Err(WebPushError::CryptoError);
    }

    Ok(decrypted[..end - 1].to_vec())
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::*;

    #[test]
    fn rfc8291_appendix_a_matches_bytes() {
        let plaintext_b64 = "V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24";
        let plaintext = URL_SAFE_NO_PAD.decode(plaintext_b64).unwrap();

        let as_pub_b64 = "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
        let _as_pub: [u8; 65] = URL_SAFE_NO_PAD
            .decode(as_pub_b64)
            .unwrap()
            .try_into()
            .unwrap();

        let as_priv_b64 = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
        let as_priv: [u8; 32] = URL_SAFE_NO_PAD
            .decode(as_priv_b64)
            .unwrap()
            .try_into()
            .unwrap();

        let ua_pub_b64 = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
        let ua_pub: [u8; 65] = URL_SAFE_NO_PAD
            .decode(ua_pub_b64)
            .unwrap()
            .try_into()
            .unwrap();

        let ua_priv_b64 = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
        let ua_priv: [u8; 32] = URL_SAFE_NO_PAD
            .decode(ua_priv_b64)
            .unwrap()
            .try_into()
            .unwrap();

        let salt_b64 = "DGv6ra1nlYgDCS1FRnbzlw";
        let salt: [u8; 16] = URL_SAFE_NO_PAD
            .decode(salt_b64)
            .unwrap()
            .try_into()
            .unwrap();

        let auth_b64 = "BTBZMqHH6r4Tts7J_aSIgg";
        let auth: [u8; 16] = URL_SAFE_NO_PAD
            .decode(auth_b64)
            .unwrap()
            .try_into()
            .unwrap();

        // Encrypt with test helper
        let encrypted =
            encrypt_web_push_with_params(&ua_pub, &auth, &as_priv, &salt, &plaintext).unwrap();

        // Header expected:
        let expected_header_b64 = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
        let expected_header = URL_SAFE_NO_PAD.decode(expected_header_b64).unwrap();
        assert_eq!(&encrypted[..86], &expected_header[..]);

        // Ciphertext expected:
        let expected_ct_b64 =
            "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ";
        let expected_ct = URL_SAFE_NO_PAD.decode(expected_ct_b64).unwrap();
        assert_eq!(&encrypted[86..], &expected_ct[..]);

        // Decrypt with test decryptor
        let decrypted = decrypt_web_push_for_test(&ua_priv, &auth, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
