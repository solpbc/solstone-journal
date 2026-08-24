// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read confirmed owner-centroid records written by the speakers owner flow.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use solstone_core_entity::{entity_memory_path, normalize_embedding};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, atomic_replace, hold_lock,
};
use zip::ZipArchive;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use solstone_core_npy::{NpyBlob, parse_npy, write_npy};
use solstone_core_speaker_id::calibration::{
    OWNER_MARGIN_MIN, OWNER_REBUILD_MAX_COHESION_DROP, OWNER_REBUILD_MIN_CENTROID_AGREEMENT,
    OWNER_REBUILD_MIN_CLUSTER_SIZE_RATIO, OWNER_THRESHOLD,
};

use crate::owner_admission::{OwnerAdmissionFailure, require_admitted_owner_target};

/// One normalized owner centroid and the calibration stored beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerCentroid {
    pub centroid: Vec<f32>,
    pub threshold: f32,
    pub margin: Option<f32>,
    pub cluster_size: i32,
    pub last_refreshed_at: Option<String>,
    pub created_at: Option<String>,
    pub evidence_tier: Option<String>,
    pub evidence_hash: Option<String>,
    pub evidence_intra_cosine_p25: Option<f32>,
}

/// Failure while reading an owner-centroid NPZ record.
#[derive(Debug)]
pub enum OwnerCentroidError {
    IdentityInvalid,
    TargetMismatch {
        requested_id: String,
        admitted_id: String,
    },
    EntityPath(solstone_core_entity::EntityLifecycleError),
    Io {
        path: PathBuf,
        detail: String,
    },
    Archive(String),
    MissingRequiredMember(String),
    Invalid(String),
}

impl fmt::Display for OwnerCentroidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdentityInvalid => {
                formatter.write_str("configured owner identity is not admitted")
            }
            Self::TargetMismatch {
                requested_id,
                admitted_id,
            } => write!(
                formatter,
                "requested owner target {requested_id:?} does not match the journal's admitted owner {admitted_id:?}"
            ),
            Self::EntityPath(error) => error.fmt(formatter),
            Self::Io { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::Archive(detail) | Self::MissingRequiredMember(detail) | Self::Invalid(detail) => {
                formatter.write_str(detail)
            }
        }
    }
}

impl Error for OwnerCentroidError {}

/// Input for the common build/confirm owner-centroid writer.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerCentroidWriteInput {
    pub centroid: Vec<f32>,
    pub cluster_size: i32,
    pub timestamp: String,
    pub evidence_tier: String,
}

/// Input for a guarded owner-centroid rebuild.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerCentroidRebuildInput {
    pub centroid: Vec<f32>,
    pub embeddings_count: i32,
    pub timestamp: String,
    pub evidence_hash: String,
    pub evidence_intra_cosine_p25: f32,
    pub evidence_tier: String,
    pub override_regression: bool,
}

/// Result of a guarded owner-centroid rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum OwnerCentroidRebuildOutcome {
    Rebuilt { override_applied: bool },
    Unchanged,
    Refused { reason: String },
}

/// Failure while writing an owner-centroid record.
#[derive(Debug)]
pub enum OwnerCentroidWriteError {
    IdentityInvalid,
    TargetMismatch {
        requested_id: String,
        admitted_id: String,
    },
    EntityPath(solstone_core_entity::EntityLifecycleError),
    Lock(LockError),
    Write(AtomicWriteError),
    Read(OwnerCentroidError),
    Invalid(String),
    Archive(String),
}

impl fmt::Display for OwnerCentroidWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdentityInvalid => {
                formatter.write_str("configured owner identity is not admitted")
            }
            Self::TargetMismatch {
                requested_id,
                admitted_id,
            } => write!(
                formatter,
                "requested owner target {requested_id:?} does not match the journal's admitted owner {admitted_id:?}"
            ),
            Self::EntityPath(error) => error.fmt(formatter),
            Self::Lock(LockError::Timeout(_)) => {
                formatter.write_str("voiceprint storage is busy; try again")
            }
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Invalid(error) | Self::Archive(error) => formatter.write_str(error),
        }
    }
}

impl Error for OwnerCentroidWriteError {}

impl From<solstone_core_entity::EntityLifecycleError> for OwnerCentroidWriteError {
    fn from(error: solstone_core_entity::EntityLifecycleError) -> Self {
        Self::EntityPath(error)
    }
}
impl From<LockError> for OwnerCentroidWriteError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}
impl From<AtomicWriteError> for OwnerCentroidWriteError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Write(error)
    }
}
impl From<OwnerCentroidError> for OwnerCentroidWriteError {
    fn from(error: OwnerCentroidError) -> Self {
        Self::Read(error)
    }
}

fn load_admission_failure(error: OwnerAdmissionFailure) -> OwnerCentroidError {
    match error {
        OwnerAdmissionFailure::IdentityInvalid => OwnerCentroidError::IdentityInvalid,
        OwnerAdmissionFailure::TargetMismatch {
            requested_id,
            admitted_id,
        } => OwnerCentroidError::TargetMismatch {
            requested_id,
            admitted_id,
        },
    }
}

fn write_admission_failure(error: OwnerAdmissionFailure) -> OwnerCentroidWriteError {
    match error {
        OwnerAdmissionFailure::IdentityInvalid => OwnerCentroidWriteError::IdentityInvalid,
        OwnerAdmissionFailure::TargetMismatch {
            requested_id,
            admitted_id,
        } => OwnerCentroidWriteError::TargetMismatch {
            requested_id,
            admitted_id,
        },
    }
}

/// Load and normalize an owner's persisted centroid, if present.
pub fn load_owner_centroid(
    journal_root: &Path,
    principal_entity_id: &str,
) -> Result<Option<OwnerCentroid>, OwnerCentroidError> {
    require_admitted_owner_target(journal_root, principal_entity_id)
        .map_err(load_admission_failure)?;
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
    let created_at = optional_member(&mut archive, "created_at.npy")?;
    let evidence_tier = optional_member(&mut archive, "evidence_tier.npy")?;
    let evidence_hash = optional_member(&mut archive, "evidence_hash.npy")?;
    let evidence_intra_cosine_p25 = optional_member(&mut archive, "evidence_intra_cosine_p25.npy")?;

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
    let created_at = optional_unicode_scalar(created_at, "created_at.npy")?;
    let evidence_tier = optional_unicode_scalar(evidence_tier, "evidence_tier.npy")?;
    let evidence_hash = optional_unicode_scalar(evidence_hash, "evidence_hash.npy")?;
    let evidence_intra_cosine_p25 = evidence_intra_cosine_p25
        .as_deref()
        .map(parse_blob)
        .transpose()?
        .map(|blob| f32_scalar(&blob, "evidence_intra_cosine_p25.npy"))
        .transpose()?;

    Ok(Some(OwnerCentroid {
        centroid,
        threshold,
        margin,
        cluster_size,
        last_refreshed_at,
        created_at,
        evidence_tier,
        evidence_hash,
        evidence_intra_cosine_p25,
    }))
}

/// Write the seven-member owner-centroid NPZ used by owner build and confirm.
pub fn write_owner_centroid(
    journal_root: &Path,
    principal_entity_id: &str,
    input: &OwnerCentroidWriteInput,
) -> Result<(), OwnerCentroidWriteError> {
    require_admitted_owner_target(journal_root, principal_entity_id)
        .map_err(write_admission_failure)?;
    let centroid = normalize_embedding(&input.centroid).ok_or_else(|| {
        OwnerCentroidWriteError::Invalid("owner centroid must have nonzero norm".to_owned())
    })?;
    let path = owner_centroid_path(journal_root, principal_entity_id, true)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let members = base_members(
        &centroid,
        input.cluster_size,
        &input.timestamp,
        &input.evidence_tier,
    );
    write_and_verify(journal_root, principal_entity_id, &path, &members)
}

/// Rebuild an owner centroid under the Python-compatible incumbent guards.
pub fn rebuild_owner_centroid(
    journal_root: &Path,
    principal_entity_id: &str,
    input: &OwnerCentroidRebuildInput,
) -> Result<OwnerCentroidRebuildOutcome, OwnerCentroidWriteError> {
    require_admitted_owner_target(journal_root, principal_entity_id)
        .map_err(write_admission_failure)?;
    let candidate = normalize_embedding(&input.centroid).ok_or_else(|| {
        OwnerCentroidWriteError::Invalid("owner centroid must have nonzero norm".to_owned())
    })?;
    let path = owner_centroid_path(journal_root, principal_entity_id, false)?;
    if !path.exists() {
        return Ok(OwnerCentroidRebuildOutcome::Refused {
            reason: "no_owner_centroid".to_owned(),
        });
    }
    let _lock = hold_lock(&path, LockOptions::default())?;
    let Some(incumbent) = load_owner_centroid(journal_root, principal_entity_id)? else {
        return Ok(OwnerCentroidRebuildOutcome::Refused {
            reason: "no_owner_centroid".to_owned(),
        });
    };
    if incumbent.last_refreshed_at.is_none() {
        return Ok(OwnerCentroidRebuildOutcome::Refused {
            reason: "no_owner_centroid".to_owned(),
        });
    }
    if incumbent.evidence_hash.as_deref() == Some(input.evidence_hash.as_str()) {
        return Ok(OwnerCentroidRebuildOutcome::Unchanged);
    }
    let agreement = dot(&incumbent.centroid, &candidate);
    let regression = if agreement < OWNER_REBUILD_MIN_CENTROID_AGREEMENT {
        Some("centroid_agreement_too_low")
    } else if incumbent.evidence_hash.is_some()
        && (input.embeddings_count as f32)
            < (incumbent.cluster_size as f32 * OWNER_REBUILD_MIN_CLUSTER_SIZE_RATIO)
    {
        Some("cluster_size_regression")
    } else if incumbent.evidence_hash.is_some()
        && incumbent.evidence_intra_cosine_p25.is_some()
        && incumbent.evidence_tier.as_deref() == Some(input.evidence_tier.as_str())
        && input.evidence_intra_cosine_p25
            < incumbent.evidence_intra_cosine_p25.expect("checked above")
                - OWNER_REBUILD_MAX_COHESION_DROP
    {
        Some("cohesion_regression")
    } else {
        None
    };
    if let Some(reason) = regression
        && !input.override_regression
    {
        return Ok(OwnerCentroidRebuildOutcome::Refused {
            reason: reason.to_owned(),
        });
    }
    let created_at = incumbent
        .created_at
        .as_deref()
        .or(incumbent.last_refreshed_at.as_deref())
        .unwrap_or(&input.timestamp);
    let mut members = base_members(
        &candidate,
        input.embeddings_count,
        &input.timestamp,
        &input.evidence_tier,
    );
    replace_member(
        &mut members,
        "created_at.npy",
        unicode_scalar_npy(created_at),
    );
    members.push((
        "evidence_hash.npy",
        unicode_scalar_npy(&input.evidence_hash),
    ));
    members.push((
        "evidence_intra_cosine_p25.npy",
        f32_scalar_npy(input.evidence_intra_cosine_p25),
    ));
    write_and_verify(journal_root, principal_entity_id, &path, &members)?;
    Ok(OwnerCentroidRebuildOutcome::Rebuilt {
        override_applied: regression.is_some(),
    })
}

fn owner_centroid_path(
    journal_root: &Path,
    principal_entity_id: &str,
    create: bool,
) -> Result<PathBuf, OwnerCentroidWriteError> {
    Ok(entity_memory_path(journal_root, principal_entity_id, create)?.join("owner_centroid.npz"))
}

fn base_members(
    centroid: &[f32],
    cluster_size: i32,
    timestamp: &str,
    evidence_tier: &str,
) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("centroid.npy", f32_vector_npy(centroid)),
        ("cluster_size.npy", i32_scalar_npy(cluster_size)),
        ("threshold.npy", f32_scalar_npy(OWNER_THRESHOLD)),
        ("margin.npy", f32_scalar_npy(OWNER_MARGIN_MIN)),
        ("last_refreshed_at.npy", unicode_scalar_npy(timestamp)),
        ("created_at.npy", unicode_scalar_npy(timestamp)),
        ("evidence_tier.npy", unicode_scalar_npy(evidence_tier)),
    ]
}

fn replace_member(members: &mut [(&'static str, Vec<u8>)], name: &str, bytes: Vec<u8>) {
    if let Some((_, member)) = members.iter_mut().find(|(current, _)| *current == name) {
        *member = bytes;
    }
}

fn write_and_verify(
    journal_root: &Path,
    principal_entity_id: &str,
    path: &Path,
    members: &[(&str, Vec<u8>)],
) -> Result<(), OwnerCentroidWriteError> {
    let bytes = write_archive(members)?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default())?;
    load_owner_centroid(journal_root, principal_entity_id)?.ok_or_else(|| {
        OwnerCentroidWriteError::Invalid("owner centroid disappeared after write".to_owned())
    })?;
    Ok(())
}

pub(crate) fn write_archive(
    members: &[(&str, Vec<u8>)],
) -> Result<Vec<u8>, OwnerCentroidWriteError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer
            .start_file(*name, options)
            .map_err(|error| OwnerCentroidWriteError::Archive(error.to_string()))?;
        writer
            .write_all(bytes)
            .map_err(|error| OwnerCentroidWriteError::Archive(error.to_string()))?;
    }
    writer
        .finish()
        .map_err(|error| OwnerCentroidWriteError::Archive(error.to_string()))
        .map(|cursor| cursor.into_inner())
}

pub(crate) fn f32_vector_npy(values: &[f32]) -> Vec<u8> {
    let payload = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect::<Vec<_>>();
    write_npy("<f4", &format!("({},)", values.len()), &payload)
}

pub(crate) fn f32_scalar_npy(value: f32) -> Vec<u8> {
    write_npy("<f4", "()", &value.to_le_bytes())
}

pub(crate) fn i32_scalar_npy(value: i32) -> Vec<u8> {
    write_npy("<i4", "()", &value.to_le_bytes())
}

pub(crate) fn unicode_scalar_npy(value: &str) -> Vec<u8> {
    let payload = value
        .chars()
        .flat_map(|character| (character as u32).to_le_bytes())
        .collect::<Vec<_>>();
    write_npy(&format!("<U{}", value.chars().count()), "()", &payload)
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

fn optional_unicode_scalar(
    member: Option<Vec<u8>>,
    name: &str,
) -> Result<Option<String>, OwnerCentroidError> {
    let value = member
        .as_deref()
        .map(parse_blob)
        .transpose()?
        .map(|blob| unicode_scalar(&blob, name))
        .transpose()?
        .filter(|value| !value.is_empty());
    Ok(value)
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
        OwnerCentroidError::MissingRequiredMember(format!(
            "owner centroid is missing required member {name}"
        ))
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
