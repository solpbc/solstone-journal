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
        let Some(bytes) = crate::container_seam::apply(&dest_path, bytes) else {
            continue;
        };
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
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("")?;
    header.set_groupname("")?;
    header.set_mtime(0);
    // `append_data` writes a GNU long-name entry for a path the 100-byte
    // header field cannot hold, such as a deep licence-tree path.
    builder.append_data(&mut header, dest, bytes)?;
    Ok(())
}

pub(crate) fn append_directory<W: io::Write>(
    builder: &mut Builder<W>,
    dest: &str,
    mode: u32,
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Directory);
    header.set_size(0);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("")?;
    header.set_groupname("")?;
    header.set_mtime(0);
    builder.append_data(&mut header, dest, io::empty())
}

#[derive(Debug)]
pub struct MemberBytes {
    pub path: String,
    pub bytes: Vec<u8>,
}

struct TarMember {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

fn read_tar_members(bytes: &[u8]) -> io::Result<Vec<TarMember>> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut members = Vec::new();
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
        members.push(TarMember {
            path: dest,
            mode,
            bytes,
        });
    }
    Ok(members)
}

pub fn tar_records(bytes: &[u8]) -> io::Result<Vec<FileRecord>> {
    let mut records = Vec::new();
    for member in read_tar_members(bytes)? {
        records.push(FileRecord::file(
            member.path,
            member.mode,
            sha256_hex(&member.bytes),
        ));
    }
    records.sort();
    Ok(records)
}

pub fn tar_members(bytes: &[u8]) -> io::Result<Vec<MemberBytes>> {
    Ok(read_tar_members(bytes)?
        .into_iter()
        .map(|member| MemberBytes {
            path: member.path,
            bytes: member.bytes,
        })
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Licence trees nest Go module paths well past the 100-byte name field of
    /// a tar header. The archive must still carry each one whole.
    #[test]
    fn tar_gz_and_directory_entries_keep_paths_longer_than_a_header_name() {
        let long_dir = "share/solstone-journal/licenses/restic/deps/github.com__GoogleCloudPlatform__opentelemetry-operations-go__internal__resourcemapping@v0.55.0";
        let long_file = format!("{long_dir}/LICENSE");
        assert!(long_file.len() > 100);
        let root = tempfile::tempdir().unwrap();
        let stage = root.path().join("stage");
        fs::create_dir_all(stage.join(long_dir)).unwrap();
        fs::write(stage.join(&long_file), b"licence text").unwrap();
        let out = root.path().join("out.tar.gz");
        write_tar_gz(&stage, &out).unwrap();
        let members = tar_members(&fs::read(&out).unwrap()).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].path, long_file);
        assert_eq!(members[0].bytes, b"licence text");

        let mut builder = Builder::new(Vec::new());
        append_directory(&mut builder, &format!("./usr/{long_dir}/"), 0o755).unwrap();
        append_regular(&mut builder, &format!("./usr/{long_file}"), b"x", 0o644).unwrap();
        let bytes = builder.into_inner().unwrap();
        let mut archive = tar::Archive::new(bytes.as_slice());
        let paths: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|entry| {
                entry
                    .unwrap()
                    .path()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            paths,
            vec![format!("./usr/{long_dir}/"), format!("./usr/{long_file}")]
        );
    }
}
