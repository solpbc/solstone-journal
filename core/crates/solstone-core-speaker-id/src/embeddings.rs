// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reader for transcript embedding sidecars written by this crate.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::npy_read::{NpyBlob, parse_npy};

/// Embeddings paired with their statement IDs in on-disk order.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingsFile {
    pub statements: Vec<(i64, Vec<f32>)>,
}

/// Failure while reading a present embeddings sidecar.
#[derive(Debug)]
pub enum EmbeddingsError {
    Io { path: PathBuf, detail: String },
    Archive(String),
    Invalid(String),
}

impl fmt::Display for EmbeddingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::Archive(detail) | Self::Invalid(detail) => formatter.write_str(detail),
        }
    }
}

impl Error for EmbeddingsError {}

/// Load one embeddings sidecar, treating an absent file or required member as absent data.
pub fn load_embeddings_file(path: &Path) -> Result<Option<EmbeddingsFile>, EmbeddingsError> {
    if !path.exists() {
        return Ok(None);
    }
    let file = File::open(path).map_err(|error| EmbeddingsError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| EmbeddingsError::Archive(error.to_string()))?;
    let Some(embedding_bytes) = optional_member(&mut archive, "embeddings.npy")? else {
        return Ok(None);
    };
    let Some(statement_id_bytes) = optional_member(&mut archive, "statement_ids.npy")? else {
        return Ok(None);
    };
    let embeddings =
        parse_npy(&embedding_bytes).map_err(|error| EmbeddingsError::Invalid(error.to_string()))?;
    let statement_ids = parse_npy(&statement_id_bytes)
        .map_err(|error| EmbeddingsError::Invalid(error.to_string()))?;
    let rows = f32_rows(&embeddings)?;
    let ids = i32_vector(&statement_ids)?;
    if rows.len() != ids.len() {
        return Err(EmbeddingsError::Invalid(
            "embeddings and statement_ids row counts differ".to_owned(),
        ));
    }
    Ok(Some(EmbeddingsFile {
        statements: ids
            .into_iter()
            .zip(rows)
            .map(|(id, embedding)| (i64::from(id), embedding))
            .collect(),
    }))
}

fn optional_member(
    archive: &mut ZipArchive<File>,
    name: &str,
) -> Result<Option<Vec<u8>>, EmbeddingsError> {
    match archive.by_name(name) {
        Ok(mut member) => {
            let mut bytes = Vec::new();
            member
                .read_to_end(&mut bytes)
                .map_err(|error| EmbeddingsError::Archive(error.to_string()))?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(error) => Err(EmbeddingsError::Archive(error.to_string())),
    }
}

fn f32_rows(blob: &NpyBlob<'_>) -> Result<Vec<Vec<f32>>, EmbeddingsError> {
    if blob.descr != "<f4" || blob.fortran_order || blob.shape.len() != 2 || blob.shape[1] != 256 {
        return Err(EmbeddingsError::Invalid(
            "embeddings.npy must be a C-order (rows, 256) <f4 array".to_owned(),
        ));
    }
    Ok(blob
        .payload
        .chunks_exact(4 * 256)
        .map(|row| {
            row.chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
                .collect()
        })
        .collect())
}

fn i32_vector(blob: &NpyBlob<'_>) -> Result<Vec<i32>, EmbeddingsError> {
    if blob.descr != "<i4" || blob.fortran_order || blob.shape.len() != 1 {
        return Err(EmbeddingsError::Invalid(
            "statement_ids.npy must be a C-order one-dimensional <i4 array".to_owned(),
        ));
    }
    Ok(blob
        .payload
        .chunks_exact(4)
        .map(|bytes| i32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect())
}
