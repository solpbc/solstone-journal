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

pub use crate::owner_candidate_hydrate::hydrate_owner_candidate_samples;

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
    let archive =
        ZipArchive::new(file).map_err(|error| OwnerCandidateError::Archive(error.to_string()))?;
    let (mut archive, names) = crate::zip_names::prove_open_zip(archive)
        .map_err(|failure| OwnerCandidateError::Archive(failure.to_string()))?;
    let centroid = f32_vector(
        &member(&mut archive, &names, "centroid.npy")?,
        "centroid.npy",
    )?;
    let centroid = normalize_embedding(&centroid).ok_or_else(|| {
        OwnerCandidateError::Invalid("candidate centroid has zero norm".to_owned())
    })?;
    let cluster_size = i32_scalar(
        &member(&mut archive, &names, "cluster_size.npy")?,
        "cluster_size.npy",
    )?;
    let threshold = f32_scalar(
        &member(&mut archive, &names, "threshold.npy")?,
        "threshold.npy",
    )?;
    let version = unicode_scalar(&member(&mut archive, &names, "version.npy")?, "version.npy")?;
    let evidence_tier = unicode_scalar(
        &member(&mut archive, &names, "evidence_tier.npy")?,
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

/// Acquire the sidecar lock for the owner candidate snapshot.
pub fn hold_owner_candidate_lock(
    journal_root: &Path,
) -> Result<solstone_core_journal_io::FileLock, OwnerCandidateError> {
    let path = owner_candidate_path(journal_root)?;
    hold_lock(&path, LockOptions::default()).map_err(OwnerCandidateError::Lock)
}

/// Write and reload-verify the five-member candidate snapshot while holding the candidate lock.
pub fn write_owner_candidate_in_lock(
    journal_root: &Path,
    candidate: &OwnerCandidate,
) -> Result<(), OwnerCandidateError> {
    write_owner_candidate_in_lock_checked(journal_root, candidate, &|| Ok(()))
}

pub fn write_owner_candidate_in_lock_checked(
    journal_root: &Path,
    candidate: &OwnerCandidate,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), OwnerCandidateError> {
    let centroid = normalize_embedding(&candidate.centroid).ok_or_else(|| {
        OwnerCandidateError::Invalid("candidate centroid has zero norm".to_owned())
    })?;
    let path = owner_candidate_path(journal_root)?;
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
    checkpoint().map_err(OwnerCandidateError::Invalid)?;
    atomic_replace(&path, &bytes, AtomicWriteOptions::default())
        .map_err(OwnerCandidateError::Write)?;
    load_owner_candidate(journal_root)?.ok_or_else(|| {
        OwnerCandidateError::Invalid("owner candidate disappeared after write".to_owned())
    })?;
    Ok(())
}

/// Remove the owner candidate while holding the candidate lock.
///
/// An absent snapshot is a successful no-op, matching the Python clear paths.
pub fn clear_owner_candidate_in_lock(journal_root: &Path) -> Result<Removed, OwnerCandidateError> {
    clear_owner_candidate_in_lock_checked(journal_root, &|| Ok(()))
}

#[expect(
    clippy::disallowed_methods,
    reason = "not moved in stage 1: clearing the owner-voice candidate on confirm, reject or undo is a speaker correction (board row 12)"
)]
pub fn clear_owner_candidate_in_lock_checked(
    journal_root: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Removed, OwnerCandidateError> {
    if !journal_root.join("awareness").is_dir() {
        return Ok(Removed::AlreadyAbsent);
    }
    checkpoint().map_err(OwnerCandidateError::Invalid)?;
    remove_file(journal_root, "awareness/owner_candidate.npz").map_err(OwnerCandidateError::Path)
}

/// Write and reload-verify the five-member candidate snapshot.
pub fn write_owner_candidate(
    journal_root: &Path,
    candidate: &OwnerCandidate,
) -> Result<(), OwnerCandidateError> {
    let _lock = hold_owner_candidate_lock(journal_root)?;
    write_owner_candidate_in_lock(journal_root, candidate)
}

/// Remove the owner candidate under the sidecar lock.
///
/// An absent snapshot is a successful no-op, matching the Python clear paths.
pub fn clear_owner_candidate(journal_root: &Path) -> Result<Removed, OwnerCandidateError> {
    let _lock = hold_owner_candidate_lock(journal_root)?;
    clear_owner_candidate_in_lock(journal_root)
}

fn owner_candidate_path(journal_root: &Path) -> Result<std::path::PathBuf, OwnerCandidateError> {
    contained_path(journal_root, "awareness/owner_candidate.npz").map_err(OwnerCandidateError::Path)
}

fn convert_write_error(error: OwnerCentroidWriteError) -> OwnerCandidateError {
    OwnerCandidateError::Archive(error.to_string())
}

fn member(
    archive: &mut ZipArchive<File>,
    names: &[String],
    name: &str,
) -> Result<Vec<u8>, OwnerCandidateError> {
    let Some(index) = names.iter().position(|proved| proved == name) else {
        return Err(OwnerCandidateError::Invalid(format!(
            "owner candidate is missing required member {name}"
        )));
    };
    let mut member = archive
        .by_index(index)
        .map_err(|error| OwnerCandidateError::Archive(error.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_candidate() -> OwnerCandidate {
        let mut centroid = vec![0.0f32; 256];
        centroid[0] = 1.0;
        OwnerCandidate {
            centroid,
            cluster_size: 3,
            threshold: 0.43,
            version: "2026-09-18T00:00:00Z".to_owned(),
            evidence_tier: "direct".to_owned(),
        }
    }

    #[test]
    fn in_lock_operations_succeed_under_held_lock() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let candidate = sample_candidate();

        let lock = hold_owner_candidate_lock(root).expect("acquire lock");
        write_owner_candidate_in_lock(root, &candidate).expect("write in lock");

        let loaded = load_owner_candidate(root).expect("load candidate");
        assert_eq!(loaded, Some(candidate));

        clear_owner_candidate_in_lock(root).expect("clear in lock");
        let cleared = load_owner_candidate(root).expect("load after clear");
        assert_eq!(cleared, None);

        drop(lock);
    }

    #[test]
    fn load_owner_candidate_refuses_collision_instead_of_missing_member() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("awareness")).unwrap();
        std::fs::write(
            root.join("awareness/owner_candidate.npz"),
            crate::zip_names::fixtures::colliding("centroid.npy", b"x"),
        )
        .unwrap();
        let error = load_owner_candidate(root).expect_err("collision");
        match error {
            OwnerCandidateError::Archive(detail) => {
                assert!(!detail.contains("missing required member"), "{detail}");
            }
            OwnerCandidateError::Invalid(detail) => {
                panic!("collision must not look like absence: {detail}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
