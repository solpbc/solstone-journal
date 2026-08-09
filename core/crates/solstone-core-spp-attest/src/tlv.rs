// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decoder for the fixed SPPGPU1 GPU evidence envelope.

use crate::error::TlvError;

pub const GPU_ENVELOPE_MAGIC: &[u8; 8] = b"SPPGPU1\0";
pub const GPU_ENVELOPE_FIELD_COUNT: usize = 7;
pub const SPDM_GET_MEASUREMENTS_HEADER: &[u8; 4] = b"\x11\xe0\x01\xff";
pub const SPDM_NONCE_OFFSET: usize = 4;
pub const SPDM_NONCE_SIZE: usize = 32;

const FIELD_HEADER_SIZE: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuField {
    pub field_id: u16,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuEnvelope {
    pub fields: Vec<GpuField>,
    pub nonce: [u8; SPDM_NONCE_SIZE],
    pub spdm_nonce: [u8; SPDM_NONCE_SIZE],
    pub raw: Vec<u8>,
}

impl GpuEnvelope {
    pub fn field(&self, field_id: u16) -> Option<&[u8]> {
        self.fields
            .iter()
            .find(|field| field.field_id == field_id)
            .map(|field| field.value.as_slice())
    }
}

/// Decodes an SPPGPU1 TLV envelope and validates its fixed field set.
pub fn decode_gpu_envelope(data: &[u8]) -> Result<GpuEnvelope, TlvError> {
    let header_end = GPU_ENVELOPE_MAGIC.len() + 2;
    if data.len() < header_end {
        return Err(TlvError::TruncatedHeader);
    }
    if &data[..GPU_ENVELOPE_MAGIC.len()] != GPU_ENVELOPE_MAGIC {
        return Err(TlvError::MagicMismatch);
    }

    let field_count = usize::from(u16::from_be_bytes([data[8], data[9]]));
    if field_count != GPU_ENVELOPE_FIELD_COUNT {
        return Err(TlvError::FieldCountMismatch);
    }

    let mut fields = Vec::with_capacity(field_count);
    let mut seen = [false; GPU_ENVELOPE_FIELD_COUNT];
    let mut offset = header_end;
    let mut last_field_id = 0;

    for _ in 0..field_count {
        let header_end = offset
            .checked_add(FIELD_HEADER_SIZE)
            .ok_or(TlvError::TruncatedHeader)?;
        if header_end > data.len() {
            return Err(TlvError::TruncatedHeader);
        }

        let field_id = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let length = u32::from_be_bytes([
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
        ]) as usize;
        offset = header_end;

        if field_id < last_field_id {
            return Err(TlvError::FieldOutOfOrder);
        }

        let value_end = offset
            .checked_add(length)
            .ok_or(TlvError::FieldLengthOverrun)?;
        if value_end > data.len() {
            return Err(TlvError::FieldLengthOverrun);
        }

        if let Some(index) = field_id
            .checked_sub(1)
            .map(usize::from)
            .filter(|index| *index < GPU_ENVELOPE_FIELD_COUNT)
        {
            seen[index] = true;
        }
        fields.push(GpuField {
            field_id,
            value: data[offset..value_end].to_vec(),
        });
        last_field_id = field_id;
        offset = value_end;
    }

    if offset != data.len() {
        return Err(TlvError::TrailingBytes);
    }

    if fields
        .iter()
        .any(|field| !(1..=GPU_ENVELOPE_FIELD_COUNT as u16).contains(&field.field_id))
    {
        return Err(TlvError::UnknownFieldId);
    }

    for field_id in 1..=GPU_ENVELOPE_FIELD_COUNT as u16 {
        if fields
            .iter()
            .filter(|field| field.field_id == field_id)
            .nth(1)
            .is_some()
        {
            return Err(TlvError::DuplicateFieldId);
        }
    }

    if seen.iter().any(|present| !present) {
        return Err(TlvError::MissingFieldId);
    }

    let nonce_bytes = fields
        .iter()
        .find(|field| field.field_id == 1)
        .ok_or(TlvError::MissingFieldId)?
        .value
        .as_slice();
    let nonce = nonce_bytes.try_into().map_err(|_| TlvError::NonceLength)?;
    let spdm_report = fields
        .iter()
        .find(|field| field.field_id == 2)
        .ok_or(TlvError::MissingFieldId)?
        .value
        .as_slice();
    let spdm_nonce = extract_spdm_nonce(spdm_report)?;

    Ok(GpuEnvelope {
        fields,
        nonce,
        spdm_nonce,
        raw: data.to_vec(),
    })
}

/// Returns the SPDM GET_MEASUREMENTS nonce at its structural offset.
pub fn extract_spdm_nonce(spdm_report: &[u8]) -> Result<[u8; SPDM_NONCE_SIZE], TlvError> {
    let nonce_end = SPDM_NONCE_OFFSET + SPDM_NONCE_SIZE;
    if spdm_report.len() < nonce_end {
        return Err(TlvError::SpdmTooShort);
    }
    if &spdm_report[..SPDM_GET_MEASUREMENTS_HEADER.len()] != SPDM_GET_MEASUREMENTS_HEADER {
        return Err(TlvError::SpdmHeaderMismatch);
    }
    spdm_report[SPDM_NONCE_OFFSET..nonce_end]
        .try_into()
        .map_err(|_| TlvError::SpdmTooShort)
}

#[cfg(test)]
mod tests {
    use super::{TlvError, decode_gpu_envelope, extract_spdm_nonce};
    use crate::test_support::fixture_bytes;

    fn field_spans(data: &[u8]) -> Vec<(u16, usize, usize, usize)> {
        let count = usize::from(u16::from_be_bytes([data[8], data[9]]));
        let mut offset = 10;
        let mut spans = Vec::with_capacity(count);
        for _ in 0..count {
            let header_start = offset;
            let field_id = u16::from_be_bytes([data[offset], data[offset + 1]]);
            let length = u32::from_be_bytes([
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
            ]) as usize;
            let value_start = offset + 6;
            let value_end = value_start + length;
            spans.push((field_id, header_start, value_start, value_end));
            offset = value_end;
        }
        spans
    }

    fn span(data: &[u8], field_id: u16) -> (usize, usize, usize) {
        field_spans(data)
            .into_iter()
            .find(|(id, _, _, _)| *id == field_id)
            .map(|(_, header_start, value_start, value_end)| (header_start, value_start, value_end))
            .expect("fixture field is present")
    }

    fn replace_field(data: &[u8], field_id: u16, value: &[u8]) -> Vec<u8> {
        let (header_start, value_start, value_end) = span(data, field_id);
        let mut updated = data.to_vec();
        updated[header_start + 2..header_start + 6]
            .copy_from_slice(&(value.len() as u32).to_be_bytes());
        updated.splice(value_start..value_end, value.iter().copied());
        updated
    }

    #[test]
    fn tlv_decodes_fixture_positive() {
        let data = fixture_bytes("gpu-envelope.tlv");
        let envelope = decode_gpu_envelope(&data).expect("fixture decodes");

        assert_eq!(
            envelope
                .fields
                .iter()
                .map(|field| field.field_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(envelope.nonce, envelope.spdm_nonce);
        assert_eq!(
            extract_spdm_nonce(envelope.field(2).expect("SPDM report")),
            Ok(envelope.nonce)
        );
        assert_eq!(envelope.raw, data);
    }

    #[test]
    fn tlv_rejects_wrong_magic() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        data[0] ^= 1;

        assert_eq!(decode_gpu_envelope(&data), Err(TlvError::MagicMismatch));
    }

    #[test]
    fn tlv_rejects_field_count_mismatch() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        data[8..10].copy_from_slice(&6_u16.to_be_bytes());

        assert_eq!(
            decode_gpu_envelope(&data),
            Err(TlvError::FieldCountMismatch)
        );
    }

    #[test]
    fn tlv_rejects_duplicate_field_id() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        let (header_start, _, _) = span(&data, 7);
        data[header_start..header_start + 2].copy_from_slice(&6_u16.to_be_bytes());

        assert_eq!(decode_gpu_envelope(&data), Err(TlvError::DuplicateFieldId));
    }

    #[test]
    fn tlv_rejects_out_of_order_field_id() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        let (field_three, _, _) = span(&data, 3);
        let (field_four, _, _) = span(&data, 4);
        data[field_three..field_three + 2].copy_from_slice(&4_u16.to_be_bytes());
        data[field_four..field_four + 2].copy_from_slice(&3_u16.to_be_bytes());

        assert_eq!(decode_gpu_envelope(&data), Err(TlvError::FieldOutOfOrder));
    }

    #[test]
    fn tlv_rejects_unknown_field_id() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        let (header_start, _, _) = span(&data, 7);
        data[header_start..header_start + 2].copy_from_slice(&8_u16.to_be_bytes());

        assert_eq!(decode_gpu_envelope(&data), Err(TlvError::UnknownFieldId));
    }

    #[test]
    fn tlv_rejects_trailing_bytes() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        data.push(0);

        assert_eq!(decode_gpu_envelope(&data), Err(TlvError::TrailingBytes));
    }

    #[test]
    fn tlv_rejects_length_overrun() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        let (header_start, value_start, value_end) = span(&data, 7);
        let length = value_end - value_start;
        data[header_start + 2..header_start + 6]
            .copy_from_slice(&((length as u32) + 1).to_be_bytes());

        assert_eq!(
            decode_gpu_envelope(&data),
            Err(TlvError::FieldLengthOverrun)
        );
    }

    #[test]
    fn tlv_rejects_short_nonce_field() {
        let data = replace_field(&fixture_bytes("gpu-envelope.tlv"), 1, b"short");

        assert_eq!(decode_gpu_envelope(&data), Err(TlvError::NonceLength));
    }

    #[test]
    fn tlv_rejects_bad_spdm_header() {
        let mut data = fixture_bytes("gpu-envelope.tlv");
        let (_, value_start, _) = span(&data, 2);
        data[value_start] ^= 1;

        assert_eq!(
            decode_gpu_envelope(&data),
            Err(TlvError::SpdmHeaderMismatch)
        );
    }

    #[test]
    fn tlv_rejects_short_spdm_header() {
        let data = replace_field(&fixture_bytes("gpu-envelope.tlv"), 2, b"\x11\xe0\x01\xff");

        assert_eq!(decode_gpu_envelope(&data), Err(TlvError::SpdmTooShort));
    }
}
