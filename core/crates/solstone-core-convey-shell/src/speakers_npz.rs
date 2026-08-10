// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Small typed reads for the NPZ records shared by the speakers read surface.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::Value;
use solstone_core_npy::parse_npy;
use zip::ZipArchive;

pub(crate) struct Voiceprints {
    pub(crate) embeddings: Vec<Vec<f32>>,
    pub(crate) metadata: Vec<Value>,
}

pub(crate) fn load_voiceprints(path: &Path) -> Option<Voiceprints> {
    let mut archive = open_archive(path)?;
    let embeddings = f32_matrix(&member(&mut archive, "embeddings.npy")?, 256)?;
    let metadata = unicode_array(&member(&mut archive, "metadata.npy")?)?
        .into_iter()
        .map(|row| serde_json::from_str(&row).ok())
        .collect::<Option<Vec<_>>>()?;
    (embeddings.len() == metadata.len()).then_some(Voiceprints {
        embeddings,
        metadata,
    })
}

pub(crate) fn owner_centroid_summary(path: &Path) -> Option<OwnerCentroidSummary> {
    let mut archive = open_archive(path)?;
    let centroid = f32_vector(&member(&mut archive, "centroid.npy")?)?;
    let norm = centroid
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm == 0.0 {
        return None;
    }
    let _threshold = f32_scalar(&member(&mut archive, "threshold.npy")?)?;
    let cluster_size = i32_scalar(&member(&mut archive, "cluster_size.npy")?)?;
    let last_refreshed_at = optional_member(&mut archive, "last_refreshed_at.npy")
        .and_then(|bytes| unicode_scalar(&bytes))
        .unwrap_or_default();
    let created_at = optional_member(&mut archive, "created_at.npy")
        .and_then(|bytes| unicode_scalar(&bytes))
        .filter(|value| !value.is_empty());
    let evidence_tier = optional_member(&mut archive, "evidence_tier.npy")
        .and_then(|bytes| unicode_scalar(&bytes))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "standard".to_owned());
    Some(OwnerCentroidSummary {
        cluster_size,
        last_refreshed_at,
        created_at,
        evidence_tier,
    })
}

pub(crate) struct OwnerCentroidSummary {
    pub(crate) cluster_size: i32,
    pub(crate) last_refreshed_at: String,
    pub(crate) created_at: Option<String>,
    pub(crate) evidence_tier: String,
}

fn open_archive(path: &Path) -> Option<ZipArchive<File>> {
    ZipArchive::new(File::open(path).ok()?).ok()
}

fn member(archive: &mut ZipArchive<File>, name: &str) -> Option<Vec<u8>> {
    let mut member = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn optional_member(archive: &mut ZipArchive<File>, name: &str) -> Option<Vec<u8>> {
    member(archive, name)
}

fn f32_matrix(bytes: &[u8], columns: usize) -> Option<Vec<Vec<f32>>> {
    let blob = parse_npy(bytes).ok()?;
    if blob.descr != "<f4"
        || blob.fortran_order
        || blob.shape.len() != 2
        || blob.shape[1] != columns
    {
        return None;
    }
    blob.payload
        .chunks_exact(4)
        .map(|bytes| Some(f32::from_le_bytes(bytes.try_into().ok()?)))
        .collect::<Option<Vec<_>>>()
        .map(|values| {
            values
                .chunks_exact(columns)
                .map(ToOwned::to_owned)
                .collect()
        })
}

fn f32_vector(bytes: &[u8]) -> Option<Vec<f32>> {
    let blob = parse_npy(bytes).ok()?;
    if blob.descr != "<f4" || blob.fortran_order || blob.shape.len() != 1 {
        return None;
    }
    blob.payload
        .chunks_exact(4)
        .map(|bytes| Some(f32::from_le_bytes(bytes.try_into().ok()?)))
        .collect::<Option<Vec<_>>>()
}

fn f32_scalar(bytes: &[u8]) -> Option<f32> {
    let blob = parse_npy(bytes).ok()?;
    if blob.descr != "<f4" || blob.fortran_order || !blob.shape.is_empty() {
        return None;
    }
    Some(f32::from_le_bytes(blob.payload.try_into().ok()?))
}

fn i32_scalar(bytes: &[u8]) -> Option<i32> {
    let blob = parse_npy(bytes).ok()?;
    if blob.descr != "<i4" || blob.fortran_order || !blob.shape.is_empty() {
        return None;
    }
    Some(i32::from_le_bytes(blob.payload.try_into().ok()?))
}

fn unicode_array(bytes: &[u8]) -> Option<Vec<String>> {
    let blob = parse_npy(bytes).ok()?;
    if !blob.descr.starts_with("<U") || blob.fortran_order || blob.shape.len() != 1 {
        return None;
    }
    let width = blob.descr.strip_prefix("<U")?.parse::<usize>().ok()?;
    if width == 0 {
        return (blob.shape[0] == 0).then_some(Vec::new());
    }
    blob.payload
        .chunks_exact(width.checked_mul(4)?)
        .map(unicode_from_utf32)
        .collect()
}

fn unicode_scalar(bytes: &[u8]) -> Option<String> {
    let blob = parse_npy(bytes).ok()?;
    if !blob.descr.starts_with("<U") || blob.fortran_order || !blob.shape.is_empty() {
        return None;
    }
    unicode_from_utf32(blob.payload)
}

fn unicode_from_utf32(bytes: &[u8]) -> Option<String> {
    let mut value = String::new();
    for bytes in bytes.chunks_exact(4) {
        let codepoint = u32::from_le_bytes(bytes.try_into().ok()?);
        if codepoint != 0 {
            value.push(char::from_u32(codepoint)?);
        }
    }
    Some(value)
}
