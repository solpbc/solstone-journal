// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const EMBEDDING_WIDTH: usize = 256;
const EMBEDDINGS_MEMBER: &str = "embeddings.npy";
const METADATA_MEMBER: &str = "metadata.npy";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VoiceprintArchive {
    pub embeddings: Vec<f32>,
    pub rows: usize,
    pub metadata: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VoiceprintNpzError {
    Archive(String),
    Invalid(String),
}

impl fmt::Display for VoiceprintNpzError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(message) | Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl Error for VoiceprintNpzError {}

pub(crate) fn read_voiceprints_npz(bytes: &[u8]) -> Result<VoiceprintArchive, VoiceprintNpzError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        VoiceprintNpzError::Archive(format!("invalid voiceprint archive: {error}"))
    })?;
    let names = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .map(|file| file.name().to_owned())
                .map_err(|error| {
                    VoiceprintNpzError::Archive(format!("invalid voiceprint archive: {error}"))
                })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = BTreeSet::from([EMBEDDINGS_MEMBER.to_owned(), METADATA_MEMBER.to_owned()]);
    if names != expected || archive.len() != expected.len() {
        return Err(VoiceprintNpzError::Invalid(
            "voiceprint archive must contain exactly embeddings.npy and metadata.npy".to_owned(),
        ));
    }
    let embeddings = read_member(&mut archive, EMBEDDINGS_MEMBER)?;
    let metadata = read_member(&mut archive, METADATA_MEMBER)?;
    let (embedding_rows, values) = parse_embeddings(&embeddings)?;
    let (metadata_rows, metadata) = parse_metadata(&metadata)?;
    if embedding_rows != metadata_rows {
        return Err(VoiceprintNpzError::Invalid(
            "voiceprint embedding and metadata row counts differ".to_owned(),
        ));
    }
    Ok(VoiceprintArchive {
        embeddings: values,
        rows: embedding_rows,
        metadata,
    })
}

pub(crate) fn write_voiceprints_npz(
    embeddings: &[f32],
    metadata: &[String],
) -> Result<Vec<u8>, VoiceprintNpzError> {
    let expected_embeddings = metadata.len().checked_mul(EMBEDDING_WIDTH).ok_or_else(|| {
        VoiceprintNpzError::Invalid("voiceprint row count is too large".to_owned())
    })?;
    if embeddings.len() != expected_embeddings {
        return Err(VoiceprintNpzError::Invalid(format!(
            "voiceprint embeddings length {} does not match {} rows",
            embeddings.len(),
            metadata.len()
        )));
    }
    validate_metadata(metadata)?;
    let embeddings = write_embeddings_npy(embeddings, metadata.len());
    let metadata = write_metadata_npy(metadata)?;
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    writer
        .start_file(EMBEDDINGS_MEMBER, options)
        .map_err(zip_error)?;
    writer.write_all(&embeddings).map_err(io_error)?;
    writer
        .start_file(METADATA_MEMBER, options)
        .map_err(zip_error)?;
    writer.write_all(&metadata).map_err(io_error)?;
    writer
        .finish()
        .map_err(zip_error)
        .map(|cursor| cursor.into_inner())
}

fn read_member(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Vec<u8>, VoiceprintNpzError> {
    let mut member = archive.by_name(name).map_err(zip_error)?;
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

fn parse_embeddings(bytes: &[u8]) -> Result<(usize, Vec<f32>), VoiceprintNpzError> {
    let (header, payload) = parse_npy(bytes)?;
    if header.descr != "<f4"
        || header.fortran_order
        || header.shape.len() != 2
        || header.shape[1] != EMBEDDING_WIDTH
    {
        return Err(VoiceprintNpzError::Invalid(
            "embeddings.npy must be a little-endian float32 C-order (N, 256) array".to_owned(),
        ));
    }
    let rows = header.shape[0];
    let expected = rows
        .checked_mul(EMBEDDING_WIDTH)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| {
            VoiceprintNpzError::Invalid("embeddings.npy shape is too large".to_owned())
        })?;
    if payload.len() != expected {
        return Err(VoiceprintNpzError::Invalid(
            "embeddings.npy payload length does not match its shape".to_owned(),
        ));
    }
    let values = payload
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("exact chunk length")))
        .collect();
    Ok((rows, values))
}

fn parse_metadata(bytes: &[u8]) -> Result<(usize, Vec<String>), VoiceprintNpzError> {
    let (header, payload) = parse_npy(bytes)?;
    if header.fortran_order || header.shape.len() != 1 {
        return Err(VoiceprintNpzError::Invalid(
            "metadata.npy must be a C-order one-dimensional unicode array".to_owned(),
        ));
    }
    let width = header
        .descr
        .strip_prefix("<U")
        .ok_or_else(|| {
            VoiceprintNpzError::Invalid(
                "metadata.npy must use a little-endian unicode dtype, never pickle or object values"
                    .to_owned(),
            )
        })?
        .parse::<usize>()
        .map_err(|_| {
            VoiceprintNpzError::Invalid("metadata.npy has an invalid unicode dtype".to_owned())
        })?;
    let rows = header.shape[0];
    if width == 0 {
        if rows == 0 && payload.is_empty() {
            return Ok((0, Vec::new()));
        }
        return Err(VoiceprintNpzError::Invalid(
            "metadata.npy has an invalid zero-width unicode dtype".to_owned(),
        ));
    }
    let row_width = width.checked_mul(4).ok_or_else(|| {
        VoiceprintNpzError::Invalid("metadata.npy unicode dtype is too large".to_owned())
    })?;
    let expected = rows
        .checked_mul(row_width)
        .ok_or_else(|| VoiceprintNpzError::Invalid("metadata.npy shape is too large".to_owned()))?;
    if payload.len() != expected {
        return Err(VoiceprintNpzError::Invalid(
            "metadata.npy payload length does not match its shape".to_owned(),
        ));
    }
    let mut values = Vec::with_capacity(rows);
    for row in payload.chunks_exact(row_width) {
        let value = row
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("exact chunk length")))
            .take_while(|codepoint| *codepoint != 0)
            .map(|codepoint| {
                char::from_u32(codepoint).ok_or_else(|| {
                    VoiceprintNpzError::Invalid(
                        "metadata.npy contains an invalid unicode code point".to_owned(),
                    )
                })
            })
            .collect::<Result<String, _>>()?;
        values.push(value);
    }
    validate_metadata(&values)?;
    Ok((rows, values))
}

fn write_embeddings_npy(values: &[f32], rows: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(values.len() * 4);
    for value in values {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    write_npy("<f4", &format!("({rows}, {EMBEDDING_WIDTH})"), &payload)
}

fn write_metadata_npy(values: &[String]) -> Result<Vec<u8>, VoiceprintNpzError> {
    let width = values
        .iter()
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0);
    let mut payload = Vec::new();
    for value in values {
        for character in value.chars() {
            payload.extend_from_slice(&(character as u32).to_le_bytes());
        }
        for _ in value.chars().count()..width {
            payload.extend_from_slice(&0_u32.to_le_bytes());
        }
    }
    Ok(write_npy(
        &format!("<U{width}"),
        &format!("({},)", values.len()),
        &payload,
    ))
}

fn write_npy(descr: &str, shape: &str, payload: &[u8]) -> Vec<u8> {
    let mut header = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    let padding = (64 - ((10 + header.len() + 1) % 64)) % 64;
    header.push_str(&" ".repeat(padding));
    header.push('\n');
    let mut bytes = Vec::with_capacity(10 + header.len() + payload.len());
    bytes.extend_from_slice(b"\x93NUMPY");
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

struct NpyHeader {
    descr: String,
    fortran_order: bool,
    shape: Vec<usize>,
}

fn parse_npy(bytes: &[u8]) -> Result<(NpyHeader, &[u8]), VoiceprintNpzError> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(VoiceprintNpzError::Invalid(
            "invalid NPY magic bytes".to_owned(),
        ));
    }
    let version = (bytes[6], bytes[7]);
    let (header_start, header_len): (usize, usize) = match version {
        (1, 0) => (10, u16::from_le_bytes([bytes[8], bytes[9]]) as usize),
        (2, 0) | (3, 0) if bytes.len() >= 12 => (
            12,
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
        ),
        _ => {
            return Err(VoiceprintNpzError::Invalid(
                "unsupported NPY version".to_owned(),
            ));
        }
    };
    let header_end = header_start
        .checked_add(header_len)
        .ok_or_else(|| VoiceprintNpzError::Invalid("NPY header is too large".to_owned()))?;
    let header = bytes
        .get(header_start..header_end)
        .ok_or_else(|| VoiceprintNpzError::Invalid("truncated NPY header".to_owned()))?;
    let header = std::str::from_utf8(header)
        .map_err(|_| VoiceprintNpzError::Invalid("NPY header is not UTF-8".to_owned()))?;
    let descr = header_string_value(header, "descr")?;
    let fortran_order = match header_value(header, "fortran_order")? {
        "False" => false,
        "True" => true,
        _ => {
            return Err(VoiceprintNpzError::Invalid(
                "NPY header has an invalid fortran_order value".to_owned(),
            ));
        }
    };
    let shape_text = header_value(header, "shape")?;
    let shape = shape_text
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| VoiceprintNpzError::Invalid("NPY header has an invalid shape".to_owned()))?
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            (!part.is_empty()).then_some(part)
        })
        .map(|part| {
            part.parse::<usize>().map_err(|_| {
                VoiceprintNpzError::Invalid("NPY header has an invalid shape dimension".to_owned())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if shape.is_empty() {
        return Err(VoiceprintNpzError::Invalid(
            "NPY header must describe an array".to_owned(),
        ));
    }
    Ok((
        NpyHeader {
            descr,
            fortran_order,
            shape,
        },
        &bytes[header_end..],
    ))
}

fn header_string_value(header: &str, key: &str) -> Result<String, VoiceprintNpzError> {
    let value = header_value(header, key)?;
    let value = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .ok_or_else(|| VoiceprintNpzError::Invalid(format!("NPY header has an invalid {key}")))?;
    Ok(value.to_owned())
}

fn header_value<'a>(header: &'a str, key: &str) -> Result<&'a str, VoiceprintNpzError> {
    let prefix = format!("'{key}':");
    let value = header
        .split(&prefix)
        .nth(1)
        .ok_or_else(|| VoiceprintNpzError::Invalid(format!("NPY header is missing {key}")))?
        .trim_start();
    if value.starts_with('(') {
        let end = value.find(')').ok_or_else(|| {
            VoiceprintNpzError::Invalid(format!("NPY header has an invalid {key}"))
        })?;
        return Ok(&value[..=end]);
    }
    Ok(value
        .split(',')
        .next()
        .ok_or_else(|| VoiceprintNpzError::Invalid(format!("NPY header has an invalid {key}")))?
        .trim())
}

fn validate_metadata(values: &[String]) -> Result<(), VoiceprintNpzError> {
    for value in values {
        serde_json::from_str::<serde_json::Value>(value).map_err(|error| {
            VoiceprintNpzError::Invalid(format!("voiceprint metadata must be JSON: {error}"))
        })?;
    }
    Ok(())
}

fn zip_error(error: zip::result::ZipError) -> VoiceprintNpzError {
    VoiceprintNpzError::Archive(format!("voiceprint archive error: {error}"))
}

fn io_error(error: std::io::Error) -> VoiceprintNpzError {
    VoiceprintNpzError::Archive(format!("voiceprint archive I/O error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_embeddings_and_unicode_json_metadata() {
        let embeddings = (0..EMBEDDING_WIDTH * 2)
            .map(|index| index as f32 / 10.0)
            .collect::<Vec<_>>();
        let metadata = vec![
            r#"{"day":"20260101","source":"screen"}"#.to_owned(),
            r#"{"day":"20260102","label":"José"}"#.to_owned(),
        ];

        let bytes = write_voiceprints_npz(&embeddings, &metadata).unwrap();
        let actual = read_voiceprints_npz(&bytes).unwrap();

        assert_eq!(actual.embeddings, embeddings);
        assert_eq!(actual.rows, 2);
        assert_eq!(actual.metadata, metadata);
    }

    #[test]
    fn rejects_object_metadata_dtype() {
        let bytes = write_npy("|O", "(1,)", &[0; 8]);
        assert!(matches!(
            parse_metadata(&bytes),
            Err(VoiceprintNpzError::Invalid(message)) if message.contains("never pickle")
        ));
    }

    #[test]
    fn round_trips_empty_voiceprints() {
        let bytes = write_voiceprints_npz(&[], &[]).unwrap();
        assert_eq!(
            read_voiceprints_npz(&bytes).unwrap(),
            VoiceprintArchive {
                embeddings: Vec::new(),
                rows: 0,
                metadata: Vec::new(),
            }
        );
    }
}
