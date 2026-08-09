// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded TPM2 quote parsing and verification.

use ring::{
    digest::{Context, SHA256},
    signature::{RSA_PKCS1_2048_8192_SHA256, RSA_PSS_2048_8192_SHA256, RsaPublicKeyComponents},
};
use subtle::ConstantTimeEq;
use x509_parser::{
    pem::parse_x509_pem,
    prelude::{FromDer, SubjectPublicKeyInfo},
    public_key::PublicKey,
};

use crate::error::TpmQuoteError;

const TPM_GENERATED_VALUE: u32 = 0xff54_4347;
const TPM_ST_ATTEST_QUOTE: u16 = 0x8018;
const TPM_ALG_SHA256: u16 = 0x000b;
const TPM_ALG_RSASSA: u16 = 0x0014;
const TPM_ALG_RSAPSS: u16 = 0x0016;
const SHA256_DIGEST_SIZE: usize = 32;
const PCR_SELECTION_SLOT_COUNT: usize = 8;
const PCR_DIGEST_SLOT_COUNT: usize = 8;
const PCR_DIGEST_BUFFER_SIZE: usize = 64;
const PCR_DIGEST_SLOT_SIZE: usize = 66;
const PCR_DIGEST_LIST_SIZE: usize = 4 + (PCR_DIGEST_SLOT_COUNT * PCR_DIGEST_SLOT_SIZE);

/// All byte inputs required to verify a TPM2 quote.
pub struct TpmQuoteInput<'a> {
    pub ak_public_key_pem: &'a [u8],
    pub quote_msg: &'a [u8],
    pub quote_sig: &'a [u8],
    pub quote_pcrs: &'a [u8],
    pub expected_binding: &'a [u8; SHA256_DIGEST_SIZE],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PcrSelection {
    hash_alg: u16,
    sizeof_select: u8,
    pcr_select: Vec<u8>,
}

impl PcrSelection {
    fn selected_count(&self) -> usize {
        self.pcr_select
            .iter()
            .map(|byte| byte.count_ones() as usize)
            .sum()
    }
}

struct QuoteInfo {
    selections: Vec<PcrSelection>,
    pcr_digest: [u8; SHA256_DIGEST_SIZE],
}

struct SignatureInfo<'a> {
    sig_alg: u16,
    signature: &'a [u8],
}

struct PcrFile {
    selections: Vec<PcrSelection>,
    digest_buffers: Vec<[u8; SHA256_DIGEST_SIZE]>,
}

enum Structure {
    Attest,
    Signature,
    PcrFile,
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
    structure: Structure,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], structure: Structure) -> Self {
        Self {
            data,
            offset: 0,
            structure,
        }
    }

    fn read(&mut self, len: usize) -> Result<&'a [u8], TpmQuoteError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| self.truncated_error())?;
        let value = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| self.truncated_error())?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, TpmQuoteError> {
        Ok(*self
            .read(1)?
            .first()
            .ok_or_else(|| self.truncated_error())?)
    }

    fn u16be(&mut self) -> Result<u16, TpmQuoteError> {
        let bytes = self.read(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32be(&mut self) -> Result<u32, TpmQuoteError> {
        let bytes = self.read(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64be(&mut self) -> Result<u64, TpmQuoteError> {
        let bytes = self.read(8)?;
        Ok(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn u16le(&mut self) -> Result<u16, TpmQuoteError> {
        let bytes = self.read(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32le(&mut self) -> Result<u32, TpmQuoteError> {
        let bytes = self.read(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn require_consumed(&self) -> Result<(), TpmQuoteError> {
        if self.offset == self.data.len() {
            Ok(())
        } else {
            Err(match self.structure {
                Structure::Attest => TpmQuoteError::TrailingAttestBytes,
                Structure::Signature => TpmQuoteError::TrailingSignatureBytes,
                Structure::PcrFile => TpmQuoteError::TrailingPcrBytes,
            })
        }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    fn truncated_error(&self) -> TpmQuoteError {
        match self.structure {
            Structure::Attest => TpmQuoteError::TruncatedAttest,
            Structure::Signature => TpmQuoteError::TruncatedSignature,
            Structure::PcrFile => TpmQuoteError::TruncatedPcrFile,
        }
    }
}

/// Verifies a TPM2 quote's binding, PCR data, and signature.
pub fn verify_quote(input: TpmQuoteInput<'_>) -> Result<(), TpmQuoteError> {
    let key = load_ak_public_key(input.ak_public_key_pem)?;
    let quote = parse_quote_msg(input.quote_msg, input.expected_binding)?;
    check_pcrs(input.quote_pcrs, &quote)?;
    let signature = parse_quote_sig(input.quote_sig, key.n.len())?;
    verify_signature(&key, input.quote_msg, signature)
}

fn load_ak_public_key(pem_bytes: &[u8]) -> Result<RsaPublicKeyComponents<Vec<u8>>, TpmQuoteError> {
    let (remaining, pem) = parse_x509_pem(pem_bytes).map_err(|_| TpmQuoteError::AkPemInvalid)?;
    if !remaining.is_empty() || pem.label != "PUBLIC KEY" {
        return Err(TpmQuoteError::AkPemInvalid);
    }

    let (remaining, spki) =
        SubjectPublicKeyInfo::from_der(&pem.contents).map_err(|_| TpmQuoteError::AkPemInvalid)?;
    if !remaining.is_empty() {
        return Err(TpmQuoteError::AkPemInvalid);
    }

    let PublicKey::RSA(key) = spki.parsed().map_err(|_| TpmQuoteError::AkPemInvalid)? else {
        return Err(TpmQuoteError::AkNotRsa);
    };
    Ok(RsaPublicKeyComponents {
        n: without_leading_zeros(key.modulus),
        e: without_leading_zeros(key.exponent),
    })
}

fn without_leading_zeros(value: &[u8]) -> Vec<u8> {
    value
        .iter()
        .skip_while(|byte| **byte == 0)
        .copied()
        .collect()
}

fn parse_quote_msg(quote_msg: &[u8], expected_binding: &[u8]) -> Result<QuoteInfo, TpmQuoteError> {
    if expected_binding.len() != SHA256_DIGEST_SIZE {
        return Err(TpmQuoteError::ExpectedBindingLength);
    }

    let mut reader = Reader::new(quote_msg, Structure::Attest);
    if reader.u32be()? != TPM_GENERATED_VALUE {
        return Err(TpmQuoteError::MagicMismatch);
    }
    if reader.u16be()? != TPM_ST_ATTEST_QUOTE {
        return Err(TpmQuoteError::AttestationTypeMismatch);
    }

    let qualified_signer_size = usize::from(reader.u16be()?);
    reader.read(qualified_signer_size)?;

    let extra_data_size = usize::from(reader.u16be()?);
    let extra_data = reader.read(extra_data_size)?;
    if !bool::from(extra_data.ct_eq(expected_binding)) {
        return Err(TpmQuoteError::ExtraDataMismatch);
    }

    reader.u64be()?;
    reader.u32be()?;
    reader.u32be()?;
    reader.u8()?;
    reader.u64be()?;

    if reader.u32be()? != 1 {
        return Err(TpmQuoteError::PcrSelectionCount);
    }

    let hash_alg = reader.u16be()?;
    if hash_alg != TPM_ALG_SHA256 {
        return Err(TpmQuoteError::PcrHashAlgorithm);
    }
    let sizeof_select = reader.u8()?;
    if !(1..=PCR_SELECTION_SLOT_COUNT as u8).contains(&sizeof_select) {
        return Err(TpmQuoteError::PcrSelectionSize);
    }
    let pcr_select = reader.read(usize::from(sizeof_select))?.to_vec();
    let selection = PcrSelection {
        hash_alg,
        sizeof_select,
        pcr_select,
    };
    if selection.selected_count() == 0 {
        return Err(TpmQuoteError::PcrSelectionEmpty);
    }

    if reader.u16be()? != SHA256_DIGEST_SIZE as u16 {
        return Err(TpmQuoteError::PcrDigestSize);
    }
    let pcr_digest: [u8; SHA256_DIGEST_SIZE] = reader
        .read(SHA256_DIGEST_SIZE)?
        .try_into()
        .map_err(|_| TpmQuoteError::TruncatedAttest)?;
    reader.require_consumed()?;

    Ok(QuoteInfo {
        selections: vec![selection],
        pcr_digest,
    })
}

fn parse_quote_sig(
    quote_sig: &[u8],
    key_size_bytes: usize,
) -> Result<SignatureInfo<'_>, TpmQuoteError> {
    let mut reader = Reader::new(quote_sig, Structure::Signature);
    let sig_alg = reader.u16be()?;
    if !matches!(sig_alg, TPM_ALG_RSASSA | TPM_ALG_RSAPSS) {
        return Err(TpmQuoteError::SignatureAlgorithm);
    }
    if reader.u16be()? != TPM_ALG_SHA256 {
        return Err(TpmQuoteError::SignatureHashAlgorithm);
    }
    if usize::from(reader.u16be()?) != key_size_bytes {
        return Err(TpmQuoteError::SignatureLength);
    }
    let signature = reader.read(key_size_bytes)?;
    reader.require_consumed()?;
    Ok(SignatureInfo { sig_alg, signature })
}

fn verify_signature(
    key: &RsaPublicKeyComponents<Vec<u8>>,
    quote_msg: &[u8],
    signature: SignatureInfo<'_>,
) -> Result<(), TpmQuoteError> {
    let algorithm = match signature.sig_alg {
        TPM_ALG_RSASSA => &RSA_PKCS1_2048_8192_SHA256,
        TPM_ALG_RSAPSS => &RSA_PSS_2048_8192_SHA256,
        _ => return Err(TpmQuoteError::SignatureAlgorithm),
    };
    key.verify(algorithm, quote_msg, signature.signature)
        .map_err(|_| TpmQuoteError::SignatureInvalid)
}

fn check_pcrs(quote_pcrs: &[u8], quote: &QuoteInfo) -> Result<(), TpmQuoteError> {
    let pcr_file = parse_pcrs(quote_pcrs)?;
    if pcr_file.selections != quote.selections {
        return Err(TpmQuoteError::PcrSelectionMismatch);
    }
    let selected_count: usize = pcr_file
        .selections
        .iter()
        .map(PcrSelection::selected_count)
        .sum();
    if selected_count != pcr_file.digest_buffers.len() {
        return Err(TpmQuoteError::PcrDigestCountMismatch);
    }

    let mut digest = Context::new(&SHA256);
    for buffer in &pcr_file.digest_buffers {
        digest.update(buffer);
    }
    if digest.finish().as_ref() != quote.pcr_digest {
        return Err(TpmQuoteError::PcrDigestMismatch);
    }
    Ok(())
}

fn parse_pcrs(quote_pcrs: &[u8]) -> Result<PcrFile, TpmQuoteError> {
    let mut reader = Reader::new(quote_pcrs, Structure::PcrFile);
    let selection_count = reader.u32le()?;
    if !(1..=PCR_SELECTION_SLOT_COUNT as u32).contains(&selection_count) {
        return Err(TpmQuoteError::PcrFileSelectionCount);
    }
    let selection_count =
        usize::try_from(selection_count).map_err(|_| TpmQuoteError::PcrFileSelectionCount)?;

    let mut selections = Vec::with_capacity(selection_count);
    for index in 0..PCR_SELECTION_SLOT_COUNT {
        let hash_alg = reader.u16le()?;
        let sizeof_select = reader.u8()?;
        let pcr_select_slot = reader.read(PCR_SELECTION_SLOT_COUNT)?;
        let pad = reader.read(5)?;
        if pad.iter().any(|byte| *byte != 0) {
            return Err(TpmQuoteError::PcrSelectionPaddingNonZero);
        }
        if index >= selection_count {
            if hash_alg != 0 || sizeof_select != 0 || pcr_select_slot.iter().any(|byte| *byte != 0)
            {
                return Err(TpmQuoteError::PcrInactiveSlotNonZero);
            }
            continue;
        }
        if hash_alg != TPM_ALG_SHA256 {
            return Err(TpmQuoteError::PcrHashAlgorithm);
        }
        if !(1..=PCR_SELECTION_SLOT_COUNT as u8).contains(&sizeof_select) {
            return Err(TpmQuoteError::PcrSelectionSize);
        }
        let sizeof_select = usize::from(sizeof_select);
        if pcr_select_slot[sizeof_select..]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(TpmQuoteError::PcrSelectionPaddingNonZero);
        }
        let selection = PcrSelection {
            hash_alg,
            sizeof_select: sizeof_select as u8,
            pcr_select: pcr_select_slot[..sizeof_select].to_vec(),
        };
        if selection.selected_count() == 0 {
            return Err(TpmQuoteError::PcrSelectionEmpty);
        }
        selections.push(selection);
    }

    let digest_list_count = reader.u32le()?;
    let digest_list_count =
        usize::try_from(digest_list_count).map_err(|_| TpmQuoteError::PcrDigestListCount)?;
    if digest_list_count > reader.remaining() / PCR_DIGEST_LIST_SIZE {
        return Err(TpmQuoteError::PcrDigestListCount);
    }

    let mut digest_buffers = Vec::new();
    for _ in 0..digest_list_count {
        let count =
            usize::try_from(reader.u32le()?).map_err(|_| TpmQuoteError::PcrDigestListCount)?;
        if count > PCR_DIGEST_SLOT_COUNT {
            return Err(TpmQuoteError::PcrDigestListCount);
        }
        for digest_index in 0..PCR_DIGEST_SLOT_COUNT {
            let size = usize::from(reader.u16le()?);
            let buffer = reader.read(PCR_DIGEST_BUFFER_SIZE)?;
            if size > PCR_DIGEST_BUFFER_SIZE {
                return Err(TpmQuoteError::PcrDigestSlotInvalid);
            }
            if digest_index >= count {
                if size != 0 || buffer.iter().any(|byte| *byte != 0) {
                    return Err(TpmQuoteError::PcrDigestSlotInvalid);
                }
                continue;
            }
            if size != SHA256_DIGEST_SIZE {
                return Err(TpmQuoteError::PcrDigestSlotInvalid);
            }
            digest_buffers.push(
                buffer[..SHA256_DIGEST_SIZE]
                    .try_into()
                    .map_err(|_| TpmQuoteError::TruncatedPcrFile)?,
            );
        }
    }
    reader.require_consumed()?;
    Ok(PcrFile {
        selections,
        digest_buffers,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PCR_SELECTION_SLOT_COUNT, TpmQuoteInput, parse_pcrs, parse_quote_msg, verify_quote,
    };
    use crate::{
        binding::{BINDING_DOMAIN, composite_binding_hash},
        error::TpmQuoteError,
        test_support::fixture_bytes,
    };

    fn binding() -> [u8; 32] {
        let nonce_hex = String::from_utf8(fixture_bytes("nonce.hex")).expect("nonce is UTF-8");
        let nonce: Vec<u8> = nonce_hex
            .split_whitespace()
            .collect::<String>()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("valid hex")
            })
            .collect();
        composite_binding_hash(
            &nonce,
            &fixture_bytes("guest_x25519.pub.der"),
            &fixture_bytes("gpu-envelope.tlv"),
            BINDING_DOMAIN,
        )
        .expect("fixture binding")
    }

    fn verify(
        quote_msg: Option<&[u8]>,
        quote_sig: Option<&[u8]>,
        quote_pcrs: Option<&[u8]>,
        expected_binding: Option<&[u8; 32]>,
    ) -> Result<(), TpmQuoteError> {
        let default_binding = binding();
        let default_quote_msg = fixture_bytes("quote.msg");
        let default_quote_sig = fixture_bytes("quote.sig");
        let default_quote_pcrs = fixture_bytes("quote.pcrs");
        let ak_public_key_pem = fixture_bytes("akpub.pem");
        verify_quote(TpmQuoteInput {
            ak_public_key_pem: &ak_public_key_pem,
            quote_msg: quote_msg.unwrap_or(&default_quote_msg),
            quote_sig: quote_sig.unwrap_or(&default_quote_sig),
            quote_pcrs: quote_pcrs.unwrap_or(&default_quote_pcrs),
            expected_binding: expected_binding.unwrap_or(&default_binding),
        })
    }

    fn first_digest_size_offset() -> usize {
        4 + (PCR_SELECTION_SLOT_COUNT * 16) + 4 + 4
    }

    fn quote_msg_with_selection(sizeof_select: u8, pcr_select: &[u8]) -> Vec<u8> {
        let binding = binding();
        let mut quote = Vec::new();
        quote.extend_from_slice(&0xff54_4347_u32.to_be_bytes());
        quote.extend_from_slice(&0x8018_u16.to_be_bytes());
        quote.extend_from_slice(&0_u16.to_be_bytes());
        quote.extend_from_slice(&(binding.len() as u16).to_be_bytes());
        quote.extend_from_slice(&binding);
        quote.extend_from_slice(&0_u64.to_be_bytes());
        quote.extend_from_slice(&0_u32.to_be_bytes());
        quote.extend_from_slice(&0_u32.to_be_bytes());
        quote.push(1);
        quote.extend_from_slice(&0_u64.to_be_bytes());
        quote.extend_from_slice(&1_u32.to_be_bytes());
        quote.extend_from_slice(&0x000b_u16.to_be_bytes());
        quote.push(sizeof_select);
        quote.extend_from_slice(pcr_select);
        quote.extend_from_slice(&32_u16.to_be_bytes());
        quote.extend_from_slice(ring::digest::digest(&ring::digest::SHA256, b"").as_ref());
        quote
    }

    fn pcrs_with_selection(sizeof_select: u8, pcr_select_slot: &[u8; 8]) -> Vec<u8> {
        let mut pcrs = Vec::new();
        pcrs.extend_from_slice(&1_u32.to_le_bytes());
        pcrs.extend_from_slice(&0x000b_u16.to_le_bytes());
        pcrs.push(sizeof_select);
        pcrs.extend_from_slice(pcr_select_slot);
        pcrs.extend_from_slice(&[0; 5]);
        pcrs.extend_from_slice(&[0; 16 * 7]);
        pcrs.extend_from_slice(&0_u32.to_le_bytes());
        pcrs
    }

    #[test]
    fn tpm_quote_accepts_fixture_positive() {
        assert_eq!(verify(None, None, None, None), Ok(()));
    }

    #[test]
    fn tpm_quote_rejects_flipped_signature_byte() {
        let mut signature = fixture_bytes("quote.sig");
        *signature.last_mut().expect("fixture signature is nonempty") ^= 1;
        assert_eq!(
            verify(None, Some(&signature), None, None),
            Err(TpmQuoteError::SignatureInvalid)
        );
    }

    #[test]
    fn tpm_quote_rejects_wrong_binding() {
        let mut expected_binding = binding();
        expected_binding[0] ^= 1;
        assert_eq!(
            verify(None, None, None, Some(&expected_binding)),
            Err(TpmQuoteError::ExtraDataMismatch)
        );
    }

    #[test]
    fn tpm_quote_rejects_mutated_pcr_value() {
        let mut pcrs = fixture_bytes("quote.pcrs");
        pcrs[first_digest_size_offset() + 2] ^= 1;
        assert_eq!(
            verify(None, None, Some(&pcrs), None),
            Err(TpmQuoteError::PcrDigestMismatch)
        );
    }

    #[test]
    fn tpm_quote_rejects_pcr_selection_mismatch() {
        let mut pcrs = fixture_bytes("quote.pcrs");
        let selection_start = 4 + 2 + 1;
        pcrs[selection_start] = (pcrs[selection_start] & !1) | 2;
        assert_eq!(
            verify(None, None, Some(&pcrs), None),
            Err(TpmQuoteError::PcrSelectionMismatch)
        );
    }

    #[test]
    fn tpm_quote_rejects_empty_quote_pcr_selection() {
        assert_eq!(
            parse_quote_msg(&quote_msg_with_selection(0, b""), &binding()).map(|_| ()),
            Err(TpmQuoteError::PcrSelectionSize)
        );
    }

    #[test]
    fn tpm_quote_rejects_zero_quote_pcr_bitmap() {
        assert_eq!(
            parse_quote_msg(&quote_msg_with_selection(3, &[0; 3]), &binding()).map(|_| ()),
            Err(TpmQuoteError::PcrSelectionEmpty)
        );
    }

    #[test]
    fn tpm_quote_rejects_empty_pcr_file_selection() {
        assert_eq!(
            parse_pcrs(&pcrs_with_selection(0, &[0; 8])).map(|_| ()),
            Err(TpmQuoteError::PcrSelectionSize)
        );
    }

    #[test]
    fn tpm_quote_rejects_zero_pcr_file_bitmap() {
        assert_eq!(
            parse_pcrs(&pcrs_with_selection(3, &[0; 8])).map(|_| ()),
            Err(TpmQuoteError::PcrSelectionEmpty)
        );
    }

    #[test]
    fn tpm_quote_rejects_unsupported_signature_algorithm() {
        let mut signature = fixture_bytes("quote.sig");
        signature[0..2].copy_from_slice(&0x0015_u16.to_be_bytes());
        assert_eq!(
            verify(None, Some(&signature), None, None),
            Err(TpmQuoteError::SignatureAlgorithm)
        );
    }

    #[test]
    fn tpm_quote_rejects_unsupported_hash_algorithm() {
        let mut signature = fixture_bytes("quote.sig");
        signature[2..4].copy_from_slice(&0x0004_u16.to_be_bytes());
        assert_eq!(
            verify(None, Some(&signature), None, None),
            Err(TpmQuoteError::SignatureHashAlgorithm)
        );
    }

    #[test]
    fn tpm_quote_rejects_trailing_quote_msg_bytes() {
        let mut quote = fixture_bytes("quote.msg");
        quote.push(0);
        assert_eq!(
            verify(Some(&quote), None, None, None),
            Err(TpmQuoteError::TrailingAttestBytes)
        );
    }

    #[test]
    fn tpm_quote_rejects_oversize_pcr_digest() {
        let mut pcrs = fixture_bytes("quote.pcrs");
        pcrs[first_digest_size_offset()..first_digest_size_offset() + 2]
            .copy_from_slice(&65_u16.to_le_bytes());
        assert_eq!(
            verify(None, None, Some(&pcrs), None),
            Err(TpmQuoteError::PcrDigestSlotInvalid)
        );
    }

    #[test]
    fn tpm_quote_rejects_trailing_pcr_bytes() {
        let mut pcrs = fixture_bytes("quote.pcrs");
        pcrs.push(0);
        assert_eq!(
            verify(None, None, Some(&pcrs), None),
            Err(TpmQuoteError::TrailingPcrBytes)
        );
    }

    #[test]
    fn tpm_quote_rejects_truncated_quote_msg() {
        let quote = fixture_bytes("quote.msg");
        assert_eq!(
            verify(Some(&quote[..quote.len() - 1]), None, None, None),
            Err(TpmQuoteError::TruncatedAttest)
        );
    }
}
