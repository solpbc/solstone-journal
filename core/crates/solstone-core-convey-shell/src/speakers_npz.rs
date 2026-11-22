// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Small typed reads for the NPZ records shared by the speakers read surface.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::zip2_name::{Zip2NameFailure, prove_zip2_directory};
use solstone_core_npy::parse_npy;
use zip::ZipArchive;

pub(crate) struct Voiceprints {
    pub(crate) embeddings: Vec<Vec<f32>>,
    pub(crate) metadata: Vec<Value>,
}

/// Segment embedding metadata used to mark transcript rows available for review.
pub(crate) struct SegmentEmbeddings {
    pub(crate) statement_ids: Vec<i32>,
    pub(crate) durations_s: Option<Vec<f32>>,
}

/// Load the audio-embedding members required by the review surface.
///
/// This intentionally treats any unreadable or incomplete archive as absent,
/// matching Python's broad failure handling around segment NPZ reads.
pub(crate) fn load_segment_embeddings(
    path: &Path,
) -> Result<Option<SegmentEmbeddings>, Zip2NameFailure> {
    let Some((mut archive, names)) = open_archive(path)? else {
        return Ok(None);
    };
    let Some(embeddings) = member(&mut archive, &names, "embeddings.npy")? else {
        return Ok(None);
    };
    let Some(_) = f32_matrix(&embeddings, 256) else {
        return Ok(None);
    };
    let Some(statement_bytes) = member(&mut archive, &names, "statement_ids.npy")? else {
        return Ok(None);
    };
    let Some(statement_ids) = i32_vector(&statement_bytes) else {
        return Ok(None);
    };
    let durations_s = match member(&mut archive, &names, "durations_s.npy")? {
        Some(bytes) => Some(match f32_vector(&bytes) {
            Some(values) => values,
            None => return Ok(None),
        }),
        None => None,
    };
    Ok(Some(SegmentEmbeddings {
        statement_ids,
        durations_s,
    }))
}

pub(crate) fn load_voiceprints(path: &Path) -> Result<Option<Voiceprints>, Zip2NameFailure> {
    let Some((mut archive, names)) = open_archive(path)? else {
        return Ok(None);
    };
    let Some(embedding_bytes) = member(&mut archive, &names, "embeddings.npy")? else {
        return Ok(None);
    };
    let Some(embeddings) = f32_matrix(&embedding_bytes, 256) else {
        return Ok(None);
    };
    let Some(metadata_bytes) = member(&mut archive, &names, "metadata.npy")? else {
        return Ok(None);
    };
    let Some(metadata) = unicode_array(&metadata_bytes).and_then(|rows| {
        rows.into_iter()
            .map(|row| serde_json::from_str(&row).ok())
            .collect::<Option<Vec<_>>>()
    }) else {
        return Ok(None);
    };
    Ok((embeddings.len() == metadata.len()).then_some(Voiceprints {
        embeddings,
        metadata,
    }))
}

pub(crate) fn owner_centroid_summary(
    path: &Path,
) -> Result<Option<OwnerCentroidSummary>, Zip2NameFailure> {
    let Some((mut archive, names)) = open_archive(path)? else {
        return Ok(None);
    };
    let Some(centroid_bytes) = member(&mut archive, &names, "centroid.npy")? else {
        return Ok(None);
    };
    let Some(centroid) = f32_vector(&centroid_bytes) else {
        return Ok(None);
    };
    let norm = centroid
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm == 0.0 {
        return Ok(None);
    }
    let Some(threshold_bytes) = member(&mut archive, &names, "threshold.npy")? else {
        return Ok(None);
    };
    let Some(threshold) = f32_scalar(&threshold_bytes) else {
        return Ok(None);
    };
    let Some(cluster_size_bytes) = member(&mut archive, &names, "cluster_size.npy")? else {
        return Ok(None);
    };
    let Some(cluster_size) = i32_scalar(&cluster_size_bytes) else {
        return Ok(None);
    };
    let margin = member(&mut archive, &names, "margin.npy")?.and_then(|bytes| f32_scalar(&bytes));
    let last_refreshed_at = member(&mut archive, &names, "last_refreshed_at.npy")?
        .and_then(|bytes| unicode_scalar(&bytes))
        .unwrap_or_default();
    let created_at = member(&mut archive, &names, "created_at.npy")?
        .and_then(|bytes| unicode_scalar(&bytes))
        .filter(|value| !value.is_empty());
    let evidence_tier = member(&mut archive, &names, "evidence_tier.npy")?
        .and_then(|bytes| unicode_scalar(&bytes))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "standard".to_owned());
    let evidence_hash = member(&mut archive, &names, "evidence_hash.npy")?
        .and_then(|bytes| unicode_scalar(&bytes))
        .filter(|value| !value.is_empty());
    let evidence_intra_cosine_p25 = member(&mut archive, &names, "evidence_intra_cosine_p25.npy")?
        .and_then(|bytes| f32_scalar(&bytes));
    Ok(Some(OwnerCentroidSummary {
        cluster_size,
        threshold,
        margin,
        last_refreshed_at,
        created_at,
        evidence_tier,
        evidence_hash,
        evidence_intra_cosine_p25,
    }))
}

pub(crate) struct OwnerCentroidSummary {
    pub(crate) cluster_size: i32,
    pub(crate) threshold: f32,
    pub(crate) margin: Option<f32>,
    pub(crate) last_refreshed_at: String,
    pub(crate) created_at: Option<String>,
    pub(crate) evidence_tier: String,
    pub(crate) evidence_hash: Option<String>,
    pub(crate) evidence_intra_cosine_p25: Option<f32>,
}

pub(crate) fn npz_row_count(path: &Path, name: &str) -> Result<Option<usize>, Zip2NameFailure> {
    let Some((mut archive, names)) = open_archive(path)? else {
        return Ok(None);
    };
    let Some(member) = member(&mut archive, &names, &format!("{name}.npy"))? else {
        return Ok(None);
    };
    let blob = parse_npy(&member).ok();
    Ok(blob.and_then(|blob| (!blob.shape.is_empty()).then_some(blob.shape[0])))
}

type ProvedArchive = (ZipArchive<File>, Vec<String>);

/// Preserve legacy optional entity reads while refusing unproved member names.
pub(crate) fn entity_voiceprints_for_read(
    root: &Path,
    entity_id: &str,
) -> Result<Option<solstone_core_entity::VoiceprintArchive>, Zip2NameFailure> {
    match solstone_core_entity::try_load_entity_voiceprints_file(root, entity_id) {
        Ok(archive) => Ok(archive),
        Err(solstone_core_entity::VoiceprintOperationError::Npz(
            solstone_core_entity::VoiceprintNpzError::NameProof(failure),
        )) => Err(failure),
        Err(_) => Ok(None),
    }
}

fn open_archive(path: &Path) -> Result<Option<ProvedArchive>, Zip2NameFailure> {
    let Ok(file) = File::open(path) else {
        return Ok(None);
    };
    let archive = match ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) => return Ok(None),
    };
    Ok(Some(prove_open_zip(archive)?))
}

fn prove_open_zip<R: Read + Seek>(
    mut archive: ZipArchive<R>,
) -> Result<(ZipArchive<R>, Vec<String>), Zip2NameFailure> {
    let mut coords = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| Zip2NameFailure::Unreadable {
                detail: error.to_string().into(),
            })?;
        coords.push((entry.header_start(), entry.central_header_start()));
    }
    let central_start = archive.central_directory_start();
    let mut reader = archive.into_inner();
    let names = prove_zip2_directory(&mut reader, central_start, &coords)?;
    let archive = ZipArchive::new(reader).map_err(|error| Zip2NameFailure::Unreadable {
        detail: error.to_string().into(),
    })?;
    Ok((archive, names))
}

fn member(
    archive: &mut ZipArchive<File>,
    names: &[String],
    name: &str,
) -> Result<Option<Vec<u8>>, Zip2NameFailure> {
    let Some(index) = names.iter().position(|proved| proved == name) else {
        return Ok(None);
    };
    let mut member = archive
        .by_index(index)
        .map_err(|error| Zip2NameFailure::Unreadable {
            detail: error.to_string().into(),
        })?;
    let mut bytes = Vec::new();
    member
        .read_to_end(&mut bytes)
        .map_err(|error| Zip2NameFailure::Unreadable {
            detail: error.to_string().into(),
        })?;
    Ok(Some(bytes))
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

fn i32_vector(bytes: &[u8]) -> Option<Vec<i32>> {
    let blob = parse_npy(bytes).ok()?;
    if blob.descr != "<i4" || blob.fortran_order || blob.shape.len() != 1 {
        return None;
    }
    blob.payload
        .chunks_exact(4)
        .map(|bytes| Some(i32::from_le_bytes(bytes.try_into().ok()?)))
        .collect()
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use solstone_core_journal_io::zip2_name::crc32_ieee;

    pub(crate) fn colliding(name: &str, data: &[u8]) -> Vec<u8> {
        const UTF8_FLAG: u16 = 1 << 11;
        let flagged = "é".as_bytes();
        stored_zip(&[
            (name.as_bytes(), UTF8_FLAG, &[], data),
            (&[0x82], 0, &[], b"a"),
            (flagged, UTF8_FLAG, &[], b"b"),
        ])
    }

    type ZipMember<'a> = (&'a [u8], u16, &'a [u8], &'a [u8]);

    fn stored_zip(members: &[ZipMember<'_>]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut offsets = Vec::new();
        for (name, flags, _extra, data) in members {
            offsets.push(local.len() as u32);
            let crc = crc32_ieee(data);
            let size = data.len() as u32;
            local.extend_from_slice(b"PK\x03\x04");
            local.extend_from_slice(&20_u16.to_le_bytes());
            local.extend_from_slice(&flags.to_le_bytes());
            local.extend_from_slice(&[0; 6]);
            local.extend_from_slice(&crc.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0_u16.to_le_bytes());
            local.extend_from_slice(name);
            local.extend_from_slice(data);
        }
        let cd_start = local.len() as u32;
        let mut central = Vec::new();
        for (index, (name, flags, extra, data)) in members.iter().enumerate() {
            let crc = crc32_ieee(data);
            let size = data.len() as u32;
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&flags.to_le_bytes());
            central.extend_from_slice(&[0; 6]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&offsets[index].to_le_bytes());
            central.extend_from_slice(name);
            central.extend_from_slice(extra);
        }
        let mut bytes = local;
        bytes.extend_from_slice(&central);
        let count = members.len() as u16;
        bytes.extend_from_slice(b"PK\x05\x06");
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&cd_start.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes
    }

    #[test]
    fn load_segment_embeddings_refuses_collision() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.npz");
        assert!(load_segment_embeddings(&missing).unwrap().is_none());

        let collision = dir.path().join("collision.npz");
        std::fs::write(&collision, colliding("embeddings.npy", b"x")).unwrap();
        assert!(load_segment_embeddings(&collision).is_err());
    }
}
