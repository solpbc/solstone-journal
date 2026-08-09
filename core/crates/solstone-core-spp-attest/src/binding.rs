// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Composite CPU/GPU evidence binding checks.

use ring::digest::{Context, SHA256, digest};

use crate::{error::BindingError, tlv::GpuEnvelope};

pub const BINDING_DOMAIN: &[u8] = b"sol-spp-option-a-bind-v1";

/// Hashes the verifier nonce, channel binding, and GPU envelope fingerprint.
pub fn composite_binding_hash(
    nonce: &[u8],
    channel_binding: &[u8],
    envelope_tlv: &[u8],
    domain: &[u8],
) -> Result<[u8; 32], BindingError> {
    if nonce.len() != 32 {
        return Err(BindingError::NonceLength);
    }
    if channel_binding.is_empty() {
        return Err(BindingError::ChannelBindingEmpty);
    }
    if envelope_tlv.is_empty() {
        return Err(BindingError::EnvelopeEmpty);
    }
    if domain.is_empty() {
        return Err(BindingError::DomainEmpty);
    }

    let envelope_digest = digest(&SHA256, envelope_tlv);
    let mut binding = Context::new(&SHA256);
    binding.update(domain);
    binding.update(nonce);
    binding.update(channel_binding);
    binding.update(envelope_digest.as_ref());

    let mut result = [0; 32];
    result.copy_from_slice(binding.finish().as_ref());
    Ok(result)
}

/// Validates that both GPU envelope nonce locations match the owner nonce.
pub fn check_envelope_nonce(
    envelope: &GpuEnvelope,
    owner_nonce: &[u8],
) -> Result<(), BindingError> {
    if owner_nonce.len() != 32 {
        return Err(BindingError::NonceLength);
    }
    if envelope.nonce.as_slice() != owner_nonce {
        return Err(BindingError::EnvelopeNonceMismatch);
    }
    if envelope.spdm_nonce != envelope.nonce {
        return Err(BindingError::SpdmNonceMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BINDING_DOMAIN, BindingError, check_envelope_nonce, composite_binding_hash};
    use crate::{test_support::fixture_bytes, tlv::decode_gpu_envelope};

    const EXPECTED_BINDING: [u8; 32] = [
        0x26, 0x89, 0x01, 0x92, 0x2d, 0x7b, 0x84, 0x44, 0x13, 0x9f, 0x3d, 0x3e, 0x3e, 0xdf, 0xcc,
        0x3d, 0xd8, 0x60, 0x49, 0x1e, 0x31, 0x3b, 0x24, 0x3d, 0x94, 0xfb, 0x97, 0xba, 0x5b, 0x31,
        0x2e, 0xa2,
    ];

    fn nonce() -> Vec<u8> {
        String::from_utf8(fixture_bytes("nonce.hex"))
            .expect("nonce fixture is UTF-8")
            .split_whitespace()
            .collect::<String>()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("nonce hex is ASCII"), 16)
                    .expect("nonce hex is valid")
            })
            .collect()
    }

    fn field_value_start(data: &[u8], target_field_id: u16) -> usize {
        let count = usize::from(u16::from_be_bytes([data[8], data[9]]));
        let mut offset = 10;
        for _ in 0..count {
            let field_id = u16::from_be_bytes([data[offset], data[offset + 1]]);
            let length = u32::from_be_bytes([
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
            ]) as usize;
            if field_id == target_field_id {
                return offset + 6;
            }
            offset += 6 + length;
        }
        panic!("fixture field is present");
    }

    #[test]
    fn binding_hash_matches_fixture_vector() {
        let nonce = nonce();
        let envelope = fixture_bytes("gpu-envelope.tlv");
        let channel_binding = fixture_bytes("guest_x25519.pub.der");

        assert_eq!(
            composite_binding_hash(&nonce, &channel_binding, &envelope, BINDING_DOMAIN),
            Ok(EXPECTED_BINDING)
        );
    }

    #[test]
    fn binding_accepts_matching_owner_and_spdm_nonce() {
        let nonce = nonce();
        let envelope = decode_gpu_envelope(&fixture_bytes("gpu-envelope.tlv"))
            .expect("fixture envelope decodes");

        assert_eq!(check_envelope_nonce(&envelope, &nonce), Ok(()));
    }

    #[test]
    fn binding_rejects_foreign_field_one_nonce() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        let nonce_start = field_value_start(&data, 1);
        data[nonce_start] ^= 1;
        let envelope = decode_gpu_envelope(&data).expect("mutated envelope still decodes");

        assert_eq!(
            check_envelope_nonce(&envelope, &nonce()),
            Err(BindingError::EnvelopeNonceMismatch)
        );
    }

    #[test]
    fn binding_rejects_spdm_nonce_splice() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        let spdm_start = field_value_start(&data, 2);
        data[spdm_start + 4] ^= 1;
        let envelope = decode_gpu_envelope(&data).expect("mutated envelope still decodes");

        assert_eq!(
            check_envelope_nonce(&envelope, &nonce()),
            Err(BindingError::SpdmNonceMismatch)
        );
    }

    #[test]
    fn binding_rejects_wrong_owner_nonce() {
        let envelope = decode_gpu_envelope(&fixture_bytes("gpu-envelope.tlv"))
            .expect("fixture envelope decodes");

        assert_eq!(
            check_envelope_nonce(&envelope, &[0; 32]),
            Err(BindingError::EnvelopeNonceMismatch)
        );
    }
}
