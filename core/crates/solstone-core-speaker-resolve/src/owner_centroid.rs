// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read confirmed owner-centroid records written by the speakers owner flow.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use solstone_core_entity::entity_memory_path;
use zip::ZipArchive;

use solstone_core_npy::{NpyBlob, parse_npy};

/// One normalized owner centroid and the calibration stored beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerCentroid {
    pub centroid: Vec<f32>,
    pub threshold: f32,
    pub margin: Option<f32>,
    pub cluster_size: i32,
    pub last_refreshed_at: Option<String>,
}

/// Failure while reading an owner-centroid NPZ record.
#[derive(Debug)]
pub enum OwnerCentroidError {
    EntityPath(solstone_core_entity::EntityLifecycleError),
    Io { path: PathBuf, detail: String },
    Archive(String),
    Invalid(String),
}

impl fmt::Display for OwnerCentroidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityPath(error) => error.fmt(formatter),
            Self::Io { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::Archive(detail) | Self::Invalid(detail) => formatter.write_str(detail),
        }
    }
}

impl Error for OwnerCentroidError {}

/// Load and normalize an owner's persisted centroid, if present.
pub fn load_owner_centroid(
    journal_root: &Path,
    principal_entity_id: &str,
) -> Result<Option<OwnerCentroid>, OwnerCentroidError> {
    let directory = entity_memory_path(journal_root, principal_entity_id, false)
        .map_err(OwnerCentroidError::EntityPath)?;
    let path = directory.join("owner_centroid.npz");
    if !path.exists() {
        return Ok(None);
    }
    let file = File::open(&path).map_err(|error| OwnerCentroidError::Io {
        path: path.clone(),
        detail: error.to_string(),
    })?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| OwnerCentroidError::Archive(error.to_string()))?;
    let centroid_bytes = read_member_bytes(&mut archive, "centroid.npy")?;
    let threshold_bytes = read_member_bytes(&mut archive, "threshold.npy")?;
    let cluster_size_bytes = read_member_bytes(&mut archive, "cluster_size.npy")?;
    let margin = optional_member(&mut archive, "margin.npy")?;
    let last_refreshed_at = optional_member(&mut archive, "last_refreshed_at.npy")?;

    let centroid_blob = parse_blob(&centroid_bytes)?;
    let threshold_blob = parse_blob(&threshold_bytes)?;
    let cluster_size_blob = parse_blob(&cluster_size_bytes)?;
    let centroid = f32_vector(&centroid_blob, "centroid.npy")?;
    let norm = centroid
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm == 0.0 {
        return Ok(None);
    }
    let centroid = centroid.into_iter().map(|value| value / norm).collect();
    let threshold = f32_scalar(&threshold_blob, "threshold.npy")?;
    let cluster_size = i32_scalar(&cluster_size_blob, "cluster_size.npy")?;
    let margin = margin
        .as_deref()
        .map(parse_blob)
        .transpose()?
        .map(|blob| f32_scalar(&blob, "margin.npy"))
        .transpose()?;
    let last_refreshed_at = last_refreshed_at
        .as_deref()
        .map(parse_blob)
        .transpose()?
        .map(|blob| unicode_scalar(&blob, "last_refreshed_at.npy"))
        .transpose()?
        .filter(|value| !value.is_empty());

    Ok(Some(OwnerCentroid {
        centroid,
        threshold,
        margin,
        cluster_size,
        last_refreshed_at,
    }))
}

fn parse_blob(bytes: &[u8]) -> Result<NpyBlob<'_>, OwnerCentroidError> {
    parse_npy(bytes).map_err(|error| OwnerCentroidError::Invalid(error.to_string()))
}

fn optional_member(
    archive: &mut ZipArchive<File>,
    name: &str,
) -> Result<Option<Vec<u8>>, OwnerCentroidError> {
    match archive.by_name(name) {
        Ok(mut member) => {
            let mut bytes = Vec::new();
            member
                .read_to_end(&mut bytes)
                .map_err(|error| OwnerCentroidError::Archive(error.to_string()))?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(error) => Err(OwnerCentroidError::Archive(error.to_string())),
    }
}

fn read_member_bytes(
    archive: &mut ZipArchive<File>,
    name: &str,
) -> Result<Vec<u8>, OwnerCentroidError> {
    optional_member(archive, name)?.ok_or_else(|| {
        OwnerCentroidError::Invalid(format!("owner centroid is missing required member {name}"))
    })
}

fn f32_vector(blob: &NpyBlob<'_>, name: &str) -> Result<Vec<f32>, OwnerCentroidError> {
    if blob.descr != "<f4" || blob.fortran_order || blob.shape.len() != 1 {
        return Err(OwnerCentroidError::Invalid(format!(
            "{name} must be a C-order one-dimensional <f4 array"
        )));
    }
    Ok(blob
        .payload
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect())
}

fn f32_scalar(blob: &NpyBlob<'_>, name: &str) -> Result<f32, OwnerCentroidError> {
    if blob.descr != "<f4" || blob.fortran_order || !blob.shape.is_empty() {
        return Err(OwnerCentroidError::Invalid(format!(
            "{name} must be a C-order scalar <f4 array"
        )));
    }
    Ok(f32::from_le_bytes(
        blob.payload.try_into().expect("validated scalar payload"),
    ))
}

fn i32_scalar(blob: &NpyBlob<'_>, name: &str) -> Result<i32, OwnerCentroidError> {
    if blob.descr != "<i4" || blob.fortran_order || !blob.shape.is_empty() {
        return Err(OwnerCentroidError::Invalid(format!(
            "{name} must be a C-order scalar <i4 array"
        )));
    }
    Ok(i32::from_le_bytes(
        blob.payload.try_into().expect("validated scalar payload"),
    ))
}

fn unicode_scalar(blob: &NpyBlob<'_>, name: &str) -> Result<String, OwnerCentroidError> {
    if !blob.descr.starts_with("<U") || blob.fortran_order || !blob.shape.is_empty() {
        return Err(OwnerCentroidError::Invalid(format!(
            "{name} must be a C-order scalar unicode array"
        )));
    }
    let mut value = String::new();
    for bytes in blob.payload.chunks_exact(4) {
        let codepoint = u32::from_le_bytes(bytes.try_into().expect("four-byte chunk"));
        if codepoint == 0 {
            continue;
        }
        let character = char::from_u32(codepoint).ok_or_else(|| {
            OwnerCentroidError::Invalid(format!("{name} contains an invalid unicode code point"))
        })?;
        value.push(character);
    }
    Ok(value)
}
