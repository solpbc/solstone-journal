// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::*;
use crate::store::voiceprints::{
    parse_unicode_npy, prove_open_zip, read_member, write_unicode_npy,
};
use solstone_core_npy::{parse_npy, write_npy};
use std::io::{Cursor, Write};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

pub(super) fn embeddings(bytes: &[u8]) -> Result<Vec<Vec<f32>>> {
    let npy = parse_npy(bytes).map_err(|e| e.to_string())?;
    if npy.descr != "<f4" || npy.fortran_order || npy.shape.len() != 2 || npy.shape[1] != WIDTH {
        return Err("face embeddings require C-order f32 (N,128)".into());
    }
    let out: Vec<Vec<f32>> = npy
        .payload
        .chunks_exact(WIDTH * 4)
        .map(|r| {
            r.chunks_exact(4)
                .map(|x| f32::from_le_bytes(x.try_into().expect("four bytes")))
                .collect()
        })
        .collect();
    if out.iter().any(|e| !valid_embedding(e)) {
        return Err("invalid face embedding".into());
    }
    Ok(out)
}
pub(super) fn read(bytes: &[u8]) -> Result<Archive> {
    let zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let (mut zip, names) = prove_open_zip(zip).map_err(|e| e.to_string())?;
    let mut sorted = names.clone();
    sorted.sort();
    if sorted != ["embeddings.npy", "envelope.npy", "metadata.npy"] {
        return Err("unrecognized face archive members".into());
    }
    let embs =
        embeddings(&read_member(&mut zip, &names, "embeddings.npy").map_err(|e| e.to_string())?)?;
    let meta = parse_unicode_npy(
        &read_member(&mut zip, &names, "metadata.npy").map_err(|e| e.to_string())?,
        "metadata.npy",
    )
    .map_err(|e| e.to_string())?;
    let env = parse_unicode_npy(
        &read_member(&mut zip, &names, "envelope.npy").map_err(|e| e.to_string())?,
        "envelope.npy",
    )
    .map_err(|e| e.to_string())?;
    if env.len() != 1 || embs.len() != meta.len() {
        return Err("face archive row count mismatch".into());
    }
    let envelope: Envelope = serde_json::from_str(&env[0]).map_err(|e| e.to_string())?;
    validate_envelope(&envelope)?;
    let mut keys = BTreeSet::new();
    let faces = embs
        .into_iter()
        .zip(meta)
        .map(|(embedding, m)| {
            let metadata: Metadata = serde_json::from_str(&m).map_err(|e| e.to_string())?;
            super::segment_relative(&metadata.key)?;
            if metadata.schema_version != 1
                || !metadata.quality.is_finite()
                || !keys.insert(metadata.key.clone())
            {
                return Err("invalid or duplicate face metadata".into());
            }
            Ok(Face {
                embedding,
                metadata,
            })
        })
        .collect::<Result<_>>()?;
    Ok(Archive { envelope, faces })
}
fn validate_envelope(e: &Envelope) -> Result<()> {
    if e.format != "solstone-faceprint-envelope" || e.version != 1 || e.encoder.width != WIDTH {
        return Err("unsupported faceprint envelope".into());
    }
    Ok(())
}
pub(super) fn write(archive: &Archive) -> Result<Vec<u8>> {
    validate_envelope(&archive.envelope)?;
    if archive.faces.iter().any(|f| !valid_embedding(&f.embedding)) {
        return Err("invalid face embedding".into());
    }
    let bytes: Vec<u8> = archive
        .faces
        .iter()
        .flat_map(|f| f.embedding.iter().flat_map(|x| x.to_le_bytes()))
        .collect();
    let metadata = archive
        .faces
        .iter()
        .map(|f| serde_json::to_string(&f.metadata).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>>>()?;
    let members = [
        (
            "embeddings.npy",
            write_npy("<f4", &format!("({}, 128)", archive.faces.len()), &bytes),
        ),
        ("metadata.npy", write_unicode_npy(&metadata)),
        (
            "envelope.npy",
            write_unicode_npy(&[
                serde_json::to_string(&archive.envelope).map_err(|e| e.to_string())?
            ]),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for (name, bytes) in members {
        zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    let bytes = zip.finish().map_err(|e| e.to_string())?.into_inner();
    if read(&bytes)? != *archive {
        return Err("face archive failed verification".into());
    }
    Ok(bytes)
}
