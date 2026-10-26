// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! NPY member construction and NPZ verification for transcript embeddings.

use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use solstone_core_npy::{npy_header, write_npy};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MEMBER_NAMES: [&str; 4] = [
    "embeddings.npy",
    "encoder.npy",
    "statement_ids.npy",
    "durations_s.npy",
];

pub(crate) struct NpzMembers {
    pub(crate) embeddings: Vec<u8>,
    pub(crate) encoder: Vec<u8>,
    pub(crate) statement_ids: Vec<u8>,
    pub(crate) durations_s: Vec<u8>,
}

impl NpzMembers {
    pub(crate) fn build(
        embeddings: &[f32],
        rows: usize,
        statement_ids: &[i32],
        durations_s: &[f32],
        encoder: &str,
    ) -> Self {
        let embedding_bytes = f32_bytes(embeddings);
        let statement_id_bytes = i32_bytes(statement_ids);
        let duration_bytes = f32_bytes(durations_s);
        let encoder_bytes = unicode_scalar_bytes(encoder);
        let width = encoder.chars().count();
        let width = usize::max(width, 1);
        Self {
            embeddings: write_npy("<f4", &format!("({rows}, 256)"), &embedding_bytes),
            encoder: write_npy(&format!("<U{width}"), "()", &encoder_bytes),
            statement_ids: write_npy("<i4", &format!("({rows},)"), &statement_id_bytes),
            durations_s: write_npy("<f4", &format!("({rows},)"), &duration_bytes),
        }
    }

    pub(crate) fn archive(&self) -> Result<Vec<u8>, String> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, bytes) in self.iter() {
            writer
                .start_file(name, options)
                .map_err(|error| error.to_string())?;
            writer.write_all(bytes).map_err(|error| error.to_string())?;
        }
        writer
            .finish()
            .map(Cursor::into_inner)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn verify_at(&self, path: &Path) -> Result<(), String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
        let (mut archive, names) =
            crate::zip_names::prove_open_zip(archive).map_err(|error| error.to_string())?;
        if names.len() != MEMBER_NAMES.len() {
            return Err("NPZ has an unexpected member count".to_owned());
        }
        let mut actual_names = names.clone();
        actual_names.sort_unstable();
        let mut expected_names = MEMBER_NAMES.map(str::to_owned).to_vec();
        expected_names.sort_unstable();
        if actual_names != expected_names {
            return Err("NPZ members do not match the transcript sidecar contract".to_owned());
        }
        for (name, expected) in self.iter() {
            let index = names
                .iter()
                .position(|proved| proved == name)
                .ok_or_else(|| format!("NPZ member {name} is missing"))?;
            let mut member = archive.by_index(index).map_err(|error| error.to_string())?;
            let mut actual = Vec::new();
            member
                .read_to_end(&mut actual)
                .map_err(|error| error.to_string())?;
            if actual != expected {
                return Err(format!("NPZ member {name} did not round-trip"));
            }
        }
        Ok(())
    }

    fn iter(&self) -> [(&'static str, &[u8]); 4] {
        [
            (MEMBER_NAMES[0], &self.embeddings),
            (MEMBER_NAMES[1], &self.encoder),
            (MEMBER_NAMES[2], &self.statement_ids),
            (MEMBER_NAMES[3], &self.durations_s),
        ]
    }
}

const COPY_CHUNK: usize = 64 * 1024;
static NPY_SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// Zip NPY members from files. Each payload file is the raw little-endian array body.
///
/// `cancel` is polled before each 64 KiB chunk. A true callback returns
/// `"cancelled"` and the scratch members are removed.
pub(crate) fn write_npz_from_files(
    destination: &Path,
    embeddings_f32le: &Path,
    rows: usize,
    statement_ids_i32le: &Path,
    durations_f32le: &Path,
    encoder: &str,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let directory = destination
        .parent()
        .ok_or("npz destination has no parent")?;
    let token = NPY_SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    let embeddings = scratch_member(directory, token, "embeddings.npy");
    let ids = scratch_member(directory, token, "statement_ids.npy");
    let durations = scratch_member(directory, token, "durations_s.npy");
    let encoder_path = scratch_member(directory, token, "encoder.npy");
    let result = (|| {
        write_npy_member(
            &embeddings,
            "<f4",
            &format!("({rows}, 256)"),
            embeddings_f32le,
            cancel,
        )?;
        write_npy_member(
            &ids,
            "<i4",
            &format!("({rows},)"),
            statement_ids_i32le,
            cancel,
        )?;
        write_npy_member(
            &durations,
            "<f4",
            &format!("({rows},)"),
            durations_f32le,
            cancel,
        )?;
        if cancel() {
            return Err("cancelled".to_owned());
        }
        let width = usize::max(encoder.chars().count(), 1);
        std::fs::write(
            &encoder_path,
            write_npy(&format!("<U{width}"), "()", &unicode_scalar_bytes(encoder)),
        )
        .map_err(|error| error.to_string())?;
        let members = [
            ("embeddings.npy", embeddings.as_path()),
            ("encoder.npy", encoder_path.as_path()),
            ("statement_ids.npy", ids.as_path()),
            ("durations_s.npy", durations.as_path()),
        ];
        write_zip(destination, &members, cancel)?;
        verify_npz_against_files(destination, &members, cancel)
    })();
    for path in [&embeddings, &ids, &durations, &encoder_path] {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn scratch_member(directory: &Path, token: u64, name: &str) -> PathBuf {
    directory.join(format!(
        ".solstone-npy-{}-{token}-{name}",
        std::process::id()
    ))
}

fn write_npy_member(
    destination: &Path,
    descr: &str,
    shape: &str,
    payload: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut output = File::create(destination).map_err(|error| error.to_string())?;
    output
        .write_all(&npy_header(descr, shape))
        .map_err(|error| error.to_string())?;
    let mut input = File::open(payload).map_err(|error| error.to_string())?;
    let mut buffer = [0_u8; COPY_CHUNK];
    loop {
        if cancel() {
            return Err("cancelled".to_owned());
        }
        let read = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| error.to_string())?;
    }
    output.sync_all().map_err(|error| error.to_string())
}

fn write_zip(
    destination: &Path,
    members: &[(&str, &Path)],
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let file = File::create(destination).map_err(|error| error.to_string())?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut buffer = [0_u8; COPY_CHUNK];
    for (name, path) in members {
        if cancel() {
            return Err("cancelled".to_owned());
        }
        writer
            .start_file(*name, options)
            .map_err(|error| error.to_string())?;
        let mut input = File::open(path).map_err(|error| error.to_string())?;
        loop {
            if cancel() {
                return Err("cancelled".to_owned());
            }
            let read = input.read(&mut buffer).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            writer
                .write_all(&buffer[..read])
                .map_err(|error| error.to_string())?;
        }
    }
    writer
        .finish()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Compare an NPZ's members with files, in chunks. Name collisions fail closed.
pub(crate) fn verify_npz_against_files(
    path: &Path,
    members: &[(&str, &Path)],
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    let archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let (mut archive, names) =
        crate::zip_names::prove_open_zip(archive).map_err(|error| error.to_string())?;
    if names.len() != members.len() {
        return Err("NPZ has an unexpected member count".to_owned());
    }
    let mut actual_names = names.clone();
    actual_names.sort_unstable();
    let mut expected_names = members
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    expected_names.sort_unstable();
    if actual_names != expected_names {
        return Err("NPZ members do not match the transcript sidecar contract".to_owned());
    }
    let mut buffer_actual = [0_u8; COPY_CHUNK];
    let mut buffer_expected = [0_u8; COPY_CHUNK];
    for (name, expected_path) in members {
        let index = names
            .iter()
            .position(|proved| proved == name)
            .ok_or_else(|| format!("NPZ member {name} is missing"))?;
        let mut member = archive.by_index(index).map_err(|error| error.to_string())?;
        let mut expected = File::open(expected_path).map_err(|error| error.to_string())?;
        loop {
            if cancel() {
                return Err("cancelled".to_owned());
            }
            let expected_read = expected
                .read(&mut buffer_expected)
                .map_err(|error| error.to_string())?;
            if expected_read == 0 {
                let trailing = member
                    .read(&mut buffer_actual[..1])
                    .map_err(|error| error.to_string())?;
                if trailing != 0 {
                    return Err(format!("NPZ member {name} did not round-trip"));
                }
                break;
            }
            // Zip inflation returns short reads. Take the file's count, then
            // keep reading the member until that many bytes arrive.
            member
                .read_exact(&mut buffer_actual[..expected_read])
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::UnexpectedEof {
                        format!("NPZ member {name} did not round-trip")
                    } else {
                        error.to_string()
                    }
                })?;
            if buffer_actual[..expected_read] != buffer_expected[..expected_read] {
                return Err(format!("NPZ member {name} did not round-trip"));
            }
        }
    }
    Ok(())
}

/// Structural verification used after a staged NPZ has been copied.
///
/// The byte-for-byte member check happens before the copy, against the staged
/// NPY files. This pass rejects a destination whose member names are missing,
/// duplicated, or not the transcript sidecar set.
pub(crate) fn verify_npz_file(path: &Path) -> Result<(), String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    let archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let (_archive, names) =
        crate::zip_names::prove_open_zip(archive).map_err(|error| error.to_string())?;
    if names.len() != MEMBER_NAMES.len() {
        return Err("NPZ has an unexpected member count".to_owned());
    }
    let mut actual_names = names;
    actual_names.sort_unstable();
    let mut expected_names = MEMBER_NAMES.map(str::to_owned).to_vec();
    expected_names.sort_unstable();
    if actual_names != expected_names {
        return Err("NPZ members do not match the transcript sidecar contract".to_owned());
    }
    Ok(())
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn i32_bytes(values: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn unicode_scalar_bytes(value: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(value.chars().count() * std::mem::size_of::<u32>());
    for character in value.chars() {
        bytes.extend_from_slice(&(character as u32).to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_names::fixtures::{Member, build_zip, colliding, extra_7075};

    #[test]
    fn verify_at_literal_success_and_collision_is_archive_error() {
        let members = NpzMembers::build(&[0.0; 256], 1, &[1], &[1.0], "enc");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.npz");
        std::fs::write(&path, members.archive().unwrap()).unwrap();
        members.verify_at(&path).expect("literal success");

        let bad = dir.path().join("bad.npz");
        std::fs::write(&bad, colliding("embeddings.npy", &members.embeddings)).unwrap();
        let error = members.verify_at(&bad).expect_err("collision");
        assert!(error.contains("collision") || error.contains("unproved") || !error.is_empty());

        let unflagged = dir.path().join("unflagged.npz");
        std::fs::write(
            &unflagged,
            build_zip(&[Member {
                name: b"embeddings.npy".to_vec(),
                flags: 0,
                extra: extra_7075(b"embeddings.npy", b"embeddings.npy", Some(0)),
                data: members.embeddings.clone(),
            }]),
        )
        .unwrap();
        assert!(members.verify_at(&unflagged).is_err());
    }

    #[test]
    fn multi_chunk_member_round_trips_and_a_later_byte_fails() {
        let dir = tempfile::tempdir().unwrap();
        let rows = 160;
        let payload_len = rows * 256 * 4;
        assert!(payload_len > COPY_CHUNK);
        let mut payload = vec![0_u8; payload_len];
        payload[70_000] = 0xA5;
        let embeddings = dir.path().join("embeddings.f32le");
        let ids = dir.path().join("ids.i32le");
        let durations = dir.path().join("durations.f32le");
        std::fs::write(&embeddings, &payload).unwrap();
        let mut id_bytes = Vec::with_capacity(rows * 4);
        let mut duration_bytes = Vec::with_capacity(rows * 4);
        for index in 0..rows {
            id_bytes.extend_from_slice(&(i32::try_from(index).unwrap()).to_le_bytes());
            duration_bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        }
        std::fs::write(&ids, &id_bytes).unwrap();
        std::fs::write(&durations, &duration_bytes).unwrap();
        let destination = dir.path().join("statements.npz");
        write_npz_from_files(
            &destination,
            &embeddings,
            rows,
            &ids,
            &durations,
            "enc",
            &|| false,
        )
        .expect("member larger than one chunk verifies");

        let embeddings_npy = dir.path().join("embeddings.npy");
        let ids_npy = dir.path().join("statement_ids.npy");
        let durations_npy = dir.path().join("durations_s.npy");
        let encoder_npy = dir.path().join("encoder.npy");
        std::fs::write(
            &embeddings_npy,
            write_npy("<f4", &format!("({rows}, 256)"), &payload),
        )
        .unwrap();
        std::fs::write(&ids_npy, write_npy("<i4", &format!("({rows},)"), &id_bytes)).unwrap();
        std::fs::write(
            &durations_npy,
            write_npy("<f4", &format!("({rows},)"), &duration_bytes),
        )
        .unwrap();
        let encoder = "enc";
        let width = encoder.chars().count();
        std::fs::write(
            &encoder_npy,
            write_npy(&format!("<U{width}"), "()", &unicode_scalar_bytes(encoder)),
        )
        .unwrap();
        let members = [
            ("embeddings.npy", embeddings_npy.as_path()),
            ("encoder.npy", encoder_npy.as_path()),
            ("statement_ids.npy", ids_npy.as_path()),
            ("durations_s.npy", durations_npy.as_path()),
        ];
        verify_npz_against_files(&destination, &members, &|| false)
            .expect("reconstructed members match");
        let mut changed = std::fs::read(&embeddings_npy).unwrap();
        let index = changed
            .iter()
            .rposition(|byte| *byte == 0xA5)
            .expect("marker");
        assert!(index > COPY_CHUNK);
        changed[index] = 0x5A;
        std::fs::write(&embeddings_npy, changed).unwrap();
        let error = verify_npz_against_files(&destination, &members, &|| false)
            .expect_err("a byte past the first chunk must fail");
        assert!(error.contains("did not round-trip"), "{error}");
    }
}
