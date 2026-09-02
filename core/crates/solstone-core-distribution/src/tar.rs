// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use tar::{Builder, EntryType, Header};

use crate::digest::sha256_hex;
use crate::record::FileRecord;
use crate::stage::staged_files;

pub fn write_tar_gz(stage: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(dest)?;
    let encoder = deterministic_gzip(file);
    let mut builder = Builder::new(encoder);
    for dest_path in staged_files(stage)? {
        crate::archive::refuse_escape(&dest_path)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.as_str()))?;
        let path = stage.join(&dest_path);
        let bytes = fs::read(&path)?;
        let mode = crate::stage::file_mode(&fs::metadata(&path)?);
        append_file(&mut builder, &dest_path, &bytes, mode)?;
    }
    builder.finish()?;
    builder.into_inner()?.finish()?;
    Ok(())
}

pub fn deterministic_gzip<W: io::Write>(inner: W) -> GzEncoder<W> {
    GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(inner, Compression::default())
}

fn append_file<W: io::Write>(
    builder: &mut Builder<W>,
    dest: &str,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_path(dest)?;
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("")?;
    header.set_groupname("")?;
    header.set_mtime(0);
    header.set_cksum();
    builder.append(&header, bytes)?;
    Ok(())
}

pub(crate) fn append_directory<W: io::Write>(
    builder: &mut Builder<W>,
    dest: &str,
    mode: u32,
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Directory);
    header.set_path(dest)?;
    header.set_size(0);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("")?;
    header.set_groupname("")?;
    header.set_mtime(0);
    header.set_cksum();
    builder.append(&header, io::empty())
}

pub fn tar_records(bytes: &[u8]) -> io::Result<Vec<FileRecord>> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut records = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                crate::archive::ArchiveEscape::SymlinkEscape.as_str(),
            ));
        }
        let dest = entry.path()?.to_string_lossy().replace('\\', "/");
        crate::archive::refuse_escape(&dest)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.as_str()))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let mode = entry.header().mode()?;
        records.push(FileRecord::file(dest, mode, sha256_hex(&bytes)));
    }
    records.sort();
    Ok(records)
}

pub fn gzip_bytes(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = deterministic_gzip(Vec::new());
    std::io::Write::write_all(&mut encoder, bytes)?;
    encoder.finish()
}

pub fn gunzip_bytes(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

pub(crate) fn append_regular<W: io::Write>(
    builder: &mut Builder<W>,
    dest: &str,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    append_file(builder, dest, bytes, mode)
}
