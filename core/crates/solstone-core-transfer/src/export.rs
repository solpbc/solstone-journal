// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic v1 tar+gzip export.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    DirEntryKind, PathOrDay, day_path, iter_segments, list_dir_entries,
};
use tar::{Builder, EntryType, Header};
use tempfile::NamedTempFile;

use crate::manifest::{
    MANIFEST_NAME, MANIFEST_VERSION, ManifestFile, SegmentManifest, TransferManifest, is_day,
};
use crate::{ExportReport, ExportRequest, TransferError};

#[derive(Debug)]
struct ExportFile {
    route: String,
    name: String,
    path: PathBuf,
    size: u64,
    sha256: String,
    mtime: u64,
}

/// Create a gzip-compressed v1 transfer archive for one journal day.
pub fn export(journal: &Path, request: ExportRequest) -> Result<ExportReport, TransferError> {
    if !is_day(&request.day) {
        return Err(TransferError::InvalidDay);
    }
    let day_directory = day_path(journal, Some(&request.day), false)?;
    if !day_directory.is_dir() {
        return Err(TransferError::MissingDay(request.day));
    }

    let mut segments = iter_segments(journal, PathOrDay::Directory(&day_directory))?;
    segments.sort_by(|left, right| {
        (left.stream.as_str(), left.key.as_str(), left.path.as_path()).cmp(&(
            right.stream.as_str(),
            right.key.as_str(),
            right.path.as_path(),
        ))
    });
    if segments.is_empty() {
        return Err(TransferError::NoSegments(request.day));
    }

    let mut manifest_segments = std::collections::BTreeMap::new();
    let mut files = Vec::new();
    for segment in segments {
        let route = format!("{}/{}", segment.stream, segment.key);
        let mut manifest_files = Vec::new();
        for entry in list_dir_entries(&segment.path)? {
            match entry.kind {
                DirEntryKind::Directory => continue,
                DirEntryKind::Other => return Err(TransferError::UnsupportedSource(entry.path)),
                DirEntryKind::File => {}
            }
            let name = entry
                .name
                .into_string()
                .map_err(|_| TransferError::UnsupportedSource(entry.path.clone()))?;
            let metadata = fs::metadata(&entry.path)?;
            let (sha256, size) = hash_file(&entry.path)?;
            let mtime = metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            manifest_files.push(ManifestFile {
                name: name.clone(),
                sha256: sha256.clone(),
                size,
            });
            files.push(ExportFile {
                route: route.clone(),
                name,
                path: entry.path,
                size,
                sha256,
                mtime,
            });
        }
        manifest_files.sort_by(|left, right| left.name.cmp(&right.name));
        if manifest_segments
            .insert(
                route.clone(),
                SegmentManifest {
                    files: manifest_files,
                },
            )
            .is_some()
        {
            return Err(TransferError::Manifest(format!(
                "duplicate segment route {route}"
            )));
        }
    }
    files.sort_by(|left, right| {
        (left.route.as_str(), left.name.as_str()).cmp(&(right.route.as_str(), right.name.as_str()))
    });

    let manifest = TransferManifest {
        version: MANIFEST_VERSION,
        day: request.day.clone(),
        created_at: Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ),
        host: Some(std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned())),
        segments: manifest_segments,
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| TransferError::Manifest(error.to_string()))?;

    let parent = request
        .output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(TransferError::MissingOutputParent(parent.to_path_buf()));
    }
    let mut temporary = NamedTempFile::new_in(parent)?;
    write_archive(temporary.as_file_mut(), &manifest_json, &files)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&request.output)
        .map_err(|error| error.error)?;

    Ok(ExportReport {
        day: request.day,
        segments: manifest.segments.len(),
        files: files.len(),
        output: request.output,
    })
}

fn write_archive(
    output: &mut File,
    manifest_json: &[u8],
    files: &[ExportFile],
) -> Result<(), TransferError> {
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    append_bytes(&mut archive, MANIFEST_NAME, manifest_json, now_seconds())?;
    for file in files {
        let mut source = File::open(&file.path)?;
        let (sha256, size) = hash_reader(&mut source)?;
        if sha256 != file.sha256 || size != file.size {
            return Err(TransferError::ContentMismatch(
                file.path.display().to_string(),
            ));
        }
        let mut source = File::open(&file.path)?;
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Regular);
        header.set_size(file.size);
        header.set_mode(0o644);
        header.set_mtime(file.mtime);
        header.set_cksum();
        archive.append_data(
            &mut header,
            format!("{}/{}", file.route, file.name),
            &mut source,
        )?;
    }
    let encoder = archive.into_inner()?;
    encoder.finish()?;
    Ok(())
}

fn append_bytes(
    archive: &mut Builder<GzEncoder<&mut File>>,
    name: &str,
    bytes: &[u8],
    mtime: u64,
) -> Result<(), TransferError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(mtime);
    header.set_cksum();
    archive.append_data(&mut header, name, bytes)?;
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> Result<(String, u64), TransferError> {
    hash_reader(&mut File::open(path)?)
}

pub(crate) fn hash_reader(reader: &mut impl Read) -> Result<(String, u64), TransferError> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 65_536];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
