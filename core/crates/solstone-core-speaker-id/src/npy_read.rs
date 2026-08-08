// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Minimal general reader for one NPY blob.

use std::error::Error;
use std::fmt;

/// Decoded NPY header and its exact raw payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NpyBlob<'a> {
    pub(crate) descr: String,
    pub(crate) fortran_order: bool,
    pub(crate) shape: Vec<usize>,
    pub(crate) payload: &'a [u8],
}

/// Failure while decoding an NPY blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NpyReadError(String);

impl NpyReadError {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for NpyReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for NpyReadError {}

/// Parse a self-contained NPY blob and validate its payload length.
pub(crate) fn parse_npy(bytes: &[u8]) -> Result<NpyBlob<'_>, NpyReadError> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(NpyReadError::invalid("invalid NPY magic bytes"));
    }
    let header_len = match (bytes[6], bytes[7]) {
        (1, 0) => usize::from(u16::from_le_bytes([bytes[8], bytes[9]])),
        (2 | 3, 0) => {
            if bytes.len() < 12 {
                return Err(NpyReadError::invalid("truncated NPY header length"));
            }
            usize::try_from(u32::from_le_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11],
            ]))
            .map_err(|_| NpyReadError::invalid("NPY header length is too large"))?
        }
        (major, minor) => {
            return Err(NpyReadError::invalid(format!(
                "unsupported NPY version {major}.{minor}"
            )));
        }
    };
    let header_start: usize = if bytes[6] == 1 { 10 } else { 12 };
    let header_end = header_start
        .checked_add(header_len)
        .ok_or_else(|| NpyReadError::invalid("NPY header length overflow"))?;
    let header = bytes
        .get(header_start..header_end)
        .ok_or_else(|| NpyReadError::invalid("truncated NPY header"))?;
    let header = std::str::from_utf8(header)
        .map_err(|_| NpyReadError::invalid("NPY header is not UTF-8"))?;
    let descr = quoted_field(header, "descr")?;
    let fortran_order = boolean_field(header, "fortran_order")?;
    let shape = shape_field(header)?;
    let elements = shape.iter().try_fold(1_usize, |count, dimension| {
        count
            .checked_mul(*dimension)
            .ok_or_else(|| NpyReadError::invalid("NPY shape element count overflow"))
    })?;
    let item_size = dtype_item_size(&descr)?;
    let payload_len = elements
        .checked_mul(item_size)
        .ok_or_else(|| NpyReadError::invalid("NPY payload length overflow"))?;
    let payload = bytes
        .get(header_end..)
        .ok_or_else(|| NpyReadError::invalid("missing NPY payload"))?;
    if payload.len() != payload_len {
        return Err(NpyReadError::invalid(format!(
            "NPY payload length {} does not match expected {payload_len}",
            payload.len()
        )));
    }
    Ok(NpyBlob {
        descr,
        fortran_order,
        shape,
        payload,
    })
}

fn quoted_field(header: &str, field: &str) -> Result<String, NpyReadError> {
    let marker = format!("'{field}'");
    let (_, remainder) = header
        .split_once(&marker)
        .ok_or_else(|| NpyReadError::invalid(format!("NPY header is missing {field}")))?;
    let value = remainder
        .strip_prefix(':')
        .map(str::trim_start)
        .ok_or_else(|| NpyReadError::invalid(format!("NPY header has invalid {field}")))?;
    let quote = value
        .chars()
        .next()
        .filter(|character| *character == '\'' || *character == '"')
        .ok_or_else(|| NpyReadError::invalid(format!("NPY header has invalid {field}")))?;
    let value = &value[quote.len_utf8()..];
    let end = value
        .find(quote)
        .ok_or_else(|| NpyReadError::invalid(format!("NPY header has unterminated {field}")))?;
    Ok(value[..end].to_owned())
}

fn boolean_field(header: &str, field: &str) -> Result<bool, NpyReadError> {
    let marker = format!("'{field}'");
    let (_, remainder) = header
        .split_once(&marker)
        .ok_or_else(|| NpyReadError::invalid(format!("NPY header is missing {field}")))?;
    let value = remainder
        .strip_prefix(':')
        .map(str::trim_start)
        .ok_or_else(|| NpyReadError::invalid(format!("NPY header has invalid {field}")))?;
    if value.starts_with("False") {
        Ok(false)
    } else if value.starts_with("True") {
        Ok(true)
    } else {
        Err(NpyReadError::invalid(format!(
            "NPY header has invalid {field}"
        )))
    }
}

fn shape_field(header: &str) -> Result<Vec<usize>, NpyReadError> {
    let marker = "'shape'";
    let (_, remainder) = header
        .split_once(marker)
        .ok_or_else(|| NpyReadError::invalid("NPY header is missing shape"))?;
    let value = remainder
        .strip_prefix(':')
        .map(str::trim_start)
        .ok_or_else(|| NpyReadError::invalid("NPY header has invalid shape"))?;
    let value = value
        .strip_prefix('(')
        .ok_or_else(|| NpyReadError::invalid("NPY shape is not a tuple"))?;
    let end = value
        .find(')')
        .ok_or_else(|| NpyReadError::invalid("NPY shape is unterminated"))?;
    let dimensions = value[..end].trim();
    if dimensions.is_empty() {
        return Ok(Vec::new());
    }
    dimensions
        .split(',')
        .filter(|dimension| !dimension.trim().is_empty())
        .map(|dimension| {
            dimension
                .trim()
                .parse::<usize>()
                .map_err(|_| NpyReadError::invalid("NPY shape has invalid dimension"))
        })
        .collect()
}

fn dtype_item_size(descr: &str) -> Result<usize, NpyReadError> {
    let mut characters = descr.chars();
    let endian = characters
        .next()
        .ok_or_else(|| NpyReadError::invalid("NPY dtype is empty"))?;
    if !matches!(endian, '<' | '>' | '=' | '|') {
        return Err(NpyReadError::invalid("NPY dtype has invalid byte order"));
    }
    let kind = characters
        .next()
        .ok_or_else(|| NpyReadError::invalid("NPY dtype is missing kind"))?;
    let width = characters
        .as_str()
        .parse::<usize>()
        .map_err(|_| NpyReadError::invalid("NPY dtype has invalid width"))?;
    if width == 0 {
        return Err(NpyReadError::invalid("NPY dtype has zero width"));
    }
    if kind == 'U' {
        width
            .checked_mul(4)
            .ok_or_else(|| NpyReadError::invalid("NPY unicode dtype width overflow"))
    } else {
        Ok(width)
    }
}
