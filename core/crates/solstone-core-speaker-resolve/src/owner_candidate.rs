// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable owner-candidate snapshots under the journal awareness directory.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::Serialize;
use solstone_core_entity::normalize_embedding;
use solstone_core_journal_io::{
    AtomicWriteOptions, LockError, LockOptions, Removed, atomic_replace, contained_path, hold_lock,
    remove_file,
};
use solstone_core_npy::parse_npy;
use zip::ZipArchive;

use crate::owner_centroid::{
    OwnerCentroidWriteError, f32_scalar_npy, f32_vector_npy, i32_scalar_npy, unicode_scalar_npy,
    write_archive,
};

/// One owner-candidate snapshot written before user confirmation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnerCandidate {
    pub centroid: Vec<f32>,
    pub cluster_size: i32,
    pub threshold: f32,
    pub version: String,
    pub evidence_tier: String,
}

/// Failure while reading or writing an owner candidate.
#[derive(Debug)]
pub enum OwnerCandidateError {
    Path(solstone_core_journal_io::PathError),
    Lock(LockError),
    Write(solstone_core_journal_io::AtomicWriteError),
    Archive(String),
    Invalid(String),
}

impl fmt::Display for OwnerCandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Lock(LockError::Timeout(_)) => {
                formatter.write_str("voiceprint storage is busy; try again")
            }
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Archive(error) | Self::Invalid(error) => formatter.write_str(error),
        }
    }
}
impl Error for OwnerCandidateError {}

/// Load the candidate snapshot, returning `None` when it does not exist.
pub fn load_owner_candidate(
    journal_root: &Path,
) -> Result<Option<OwnerCandidate>, OwnerCandidateError> {
    let path = owner_candidate_path(journal_root)?;
    if !path.exists() {
        return Ok(None);
    }
    let file =
        File::open(&path).map_err(|error| OwnerCandidateError::Archive(error.to_string()))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| OwnerCandidateError::Archive(error.to_string()))?;
    let centroid = f32_vector(&member(&mut archive, "centroid.npy")?, "centroid.npy")?;
    let centroid = normalize_embedding(&centroid).ok_or_else(|| {
        OwnerCandidateError::Invalid("candidate centroid has zero norm".to_owned())
    })?;
    let cluster_size = i32_scalar(
        &member(&mut archive, "cluster_size.npy")?,
        "cluster_size.npy",
    )?;
    let threshold = f32_scalar(&member(&mut archive, "threshold.npy")?, "threshold.npy")?;
    let version = unicode_scalar(&member(&mut archive, "version.npy")?, "version.npy")?;
    let evidence_tier = unicode_scalar(
        &member(&mut archive, "evidence_tier.npy")?,
        "evidence_tier.npy",
    )?;
    Ok(Some(OwnerCandidate {
        centroid,
        cluster_size,
        threshold,
        version,
        evidence_tier,
    }))
}

/// Write and reload-verify the five-member candidate snapshot.
pub fn write_owner_candidate(
    journal_root: &Path,
    candidate: &OwnerCandidate,
) -> Result<(), OwnerCandidateError> {
    let centroid = normalize_embedding(&candidate.centroid).ok_or_else(|| {
        OwnerCandidateError::Invalid("candidate centroid has zero norm".to_owned())
    })?;
    let path = owner_candidate_path(journal_root)?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(OwnerCandidateError::Lock)?;
    let members = vec![
        ("centroid.npy", f32_vector_npy(&centroid)),
        ("cluster_size.npy", i32_scalar_npy(candidate.cluster_size)),
        ("threshold.npy", f32_scalar_npy(candidate.threshold)),
        ("version.npy", unicode_scalar_npy(&candidate.version)),
        (
            "evidence_tier.npy",
            unicode_scalar_npy(&candidate.evidence_tier),
        ),
    ];
    let bytes = write_archive(&members).map_err(convert_write_error)?;
    atomic_replace(&path, &bytes, AtomicWriteOptions::default())
        .map_err(OwnerCandidateError::Write)?;
    load_owner_candidate(journal_root)?.ok_or_else(|| {
        OwnerCandidateError::Invalid("owner candidate disappeared after write".to_owned())
    })?;
    Ok(())
}

/// Remove the owner candidate under the same sidecar lock used for writes.
///
/// An absent snapshot is a successful no-op, matching the Python clear paths.
pub fn clear_owner_candidate(journal_root: &Path) -> Result<Removed, OwnerCandidateError> {
    if !journal_root.join("awareness").is_dir() {
        return Ok(Removed::AlreadyAbsent);
    }
    let path = owner_candidate_path(journal_root)?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(OwnerCandidateError::Lock)?;
    remove_file(journal_root, "awareness/owner_candidate.npz").map_err(OwnerCandidateError::Path)
}

fn owner_candidate_path(journal_root: &Path) -> Result<std::path::PathBuf, OwnerCandidateError> {
    contained_path(journal_root, "awareness/owner_candidate.npz").map_err(OwnerCandidateError::Path)
}

fn convert_write_error(error: OwnerCentroidWriteError) -> OwnerCandidateError {
    OwnerCandidateError::Archive(error.to_string())
}

fn member(archive: &mut ZipArchive<File>, name: &str) -> Result<Vec<u8>, OwnerCandidateError> {
    let mut member = archive.by_name(name).map_err(|_| {
        OwnerCandidateError::Invalid(format!("owner candidate is missing required member {name}"))
    })?;
    let mut bytes = Vec::new();
    member
        .read_to_end(&mut bytes)
        .map_err(|error| OwnerCandidateError::Archive(error.to_string()))?;
    Ok(bytes)
}

fn f32_vector(bytes: &[u8], name: &str) -> Result<Vec<f32>, OwnerCandidateError> {
    let blob = parse_npy(bytes).map_err(|error| OwnerCandidateError::Invalid(error.to_string()))?;
    if blob.descr != "<f4" || blob.fortran_order || blob.shape.len() != 1 {
        return Err(OwnerCandidateError::Invalid(format!(
            "{name} must be a C-order one-dimensional <f4 array"
        )));
    }
    Ok(blob
        .payload
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect())
}

fn f32_scalar(bytes: &[u8], name: &str) -> Result<f32, OwnerCandidateError> {
    let blob = parse_npy(bytes).map_err(|error| OwnerCandidateError::Invalid(error.to_string()))?;
    if blob.descr != "<f4" || blob.fortran_order || !blob.shape.is_empty() {
        return Err(OwnerCandidateError::Invalid(format!(
            "{name} must be a C-order scalar <f4 array"
        )));
    }
    Ok(f32::from_le_bytes(
        blob.payload.try_into().expect("validated scalar"),
    ))
}

fn i32_scalar(bytes: &[u8], name: &str) -> Result<i32, OwnerCandidateError> {
    let blob = parse_npy(bytes).map_err(|error| OwnerCandidateError::Invalid(error.to_string()))?;
    if blob.descr != "<i4" || blob.fortran_order || !blob.shape.is_empty() {
        return Err(OwnerCandidateError::Invalid(format!(
            "{name} must be a C-order scalar <i4 array"
        )));
    }
    Ok(i32::from_le_bytes(
        blob.payload.try_into().expect("validated scalar"),
    ))
}

fn unicode_scalar(bytes: &[u8], name: &str) -> Result<String, OwnerCandidateError> {
    let blob = parse_npy(bytes).map_err(|error| OwnerCandidateError::Invalid(error.to_string()))?;
    if !blob.descr.starts_with("<U") || blob.fortran_order || !blob.shape.is_empty() {
        return Err(OwnerCandidateError::Invalid(format!(
            "{name} must be a C-order scalar unicode array"
        )));
    }
    blob.payload
        .chunks_exact(4)
        .take_while(|bytes| *bytes != [0, 0, 0, 0])
        .map(|bytes| {
            char::from_u32(u32::from_le_bytes(
                bytes.try_into().expect("four-byte chunk"),
            ))
            .ok_or_else(|| {
                OwnerCandidateError::Invalid(format!(
                    "{name} contains an invalid unicode code point"
                ))
            })
        })
        .collect()
}
