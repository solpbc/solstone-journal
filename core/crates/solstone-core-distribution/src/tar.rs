// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use tar::{Builder, EntryType, Header};

use crate::stage::staged_files;

pub fn write_tar_gz(stage: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(dest)?;
    let encoder = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(file, Compression::default());
    let mut builder = Builder::new(encoder);
    for dest_path in staged_files(stage)? {
        let bytes = fs::read(stage.join(&dest_path))?;
        append_file(&mut builder, &dest_path, &bytes)?;
    }
    builder.finish()?;
    builder.into_inner()?.finish()?;
    Ok(())
}

fn append_file<W: io::Write>(builder: &mut Builder<W>, dest: &str, bytes: &[u8]) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_path(dest)?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder.append(&header, bytes)?;
    Ok(())
}

pub fn list_tar_gz(path: &Path) -> io::Result<Vec<String>> {
    list_tar_gz_bytes(&fs::read(path)?)
}

pub fn list_tar_gz_bytes(bytes: &[u8]) -> io::Result<Vec<String>> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut dests = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        dests.push(entry.path()?.to_string_lossy().replace('\\', "/"));
    }
    dests.sort();
    dests.dedup();
    Ok(dests)
}

pub fn gzip_bytes(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, bytes)?;
    encoder.finish()
}

pub fn gunzip_bytes(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}
