// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use tar::Builder;

use crate::ar::{member, read_archive, write_archive};
use crate::record::FileRecord;
use crate::relocate::{from_system_path, to_system_path};
use crate::stage::staged_files;
use crate::tar::{append_directory, append_regular, gzip_bytes, tar_records};

pub struct DebMeta<'a> {
    pub version: &'a str,
    pub arch: &'a str,
}

pub fn write_deb(stage: &Path, dest: &Path, meta: DebMeta<'_>) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let control = gzip_bytes(&control_tar(meta)?)?;
    let data = gzip_bytes(&data_tar(stage)?)?;
    let mut out = fs::File::create(dest)?;
    write_archive(
        &mut out,
        &[
            ("debian-binary", b"2.0\n".as_slice()),
            ("control.tar.gz", control.as_slice()),
            ("data.tar.gz", data.as_slice()),
        ],
    )
}

fn control_tar(meta: DebMeta<'_>) -> io::Result<Vec<u8>> {
    let control = format!(
        "Package: solstone-journal\nVersion: {}\nArchitecture: {}\nMaintainer: sol pbc <support@solstone.app>\nDescription: solstone-journal\nDepends: libc6 (>= 2.27), libstdc++6, libgcc-s1\n",
        meta.version, meta.arch
    );
    let mut builder = Builder::new(Vec::new());
    append_regular(&mut builder, "control", control.as_bytes(), 0o644)?;
    builder.finish()?;
    builder.into_inner()
}

fn data_tar(stage: &Path) -> io::Result<Vec<u8>> {
    let mut builder = Builder::new(Vec::new());
    let files = staged_files(stage)?;
    let mut directories = BTreeSet::new();
    for dest in &files {
        let archive = to_system_path(dest).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unstaged dest {dest} has no system prefix"),
            )
        })?;
        let mut parent = Path::new(&archive).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            directories.insert(path.to_string_lossy().replace('\\', "/"));
            parent = path.parent();
        }
    }
    for directory in directories {
        append_directory(&mut builder, &directory, 0o755)?;
    }
    for dest in files {
        let archive = to_system_path(&dest).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unstaged dest {dest} has no system prefix"),
            )
        })?;
        let path = stage.join(&dest);
        let bytes = fs::read(&path)?;
        let mode = crate::stage::file_mode(&fs::metadata(&path)?);
        append_regular(&mut builder, &archive, &bytes, mode)?;
    }
    builder.finish()?;
    builder.into_inner()
}

pub fn deb_records(path: &Path) -> io::Result<Vec<FileRecord>> {
    let bytes = fs::read(path)?;
    let members = read_archive(&bytes)?;
    let data = member(&members, "data.tar.gz")?;
    let mut records = Vec::new();
    for record in tar_records(data)? {
        let dest = from_system_path(&record.dest).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("deb member {} is outside the system prefix", record.dest),
            )
        })?;
        records.push(FileRecord::file(dest, record.mode, record.digest));
    }
    records.sort();
    Ok(records)
}

pub fn deb_control_text(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    let members = read_archive(&bytes)?;
    let control = member(&members, "control.tar.gz")?;
    let decoder = flate2::read::GzDecoder::new(control);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_string_lossy() == "control" {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut entry, &mut text)?;
            return Ok(text);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "deb control file missing",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_tar_carries_deterministic_parent_directories() {
        let stage = tempfile::Builder::new()
            .prefix("solstone-deb-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        crate::stage::write_staged_file_mode(
            stage.path(),
            "lib/solstone-core-speakers-analyze/libonnxruntime.so.1",
            b"runtime",
            0o755,
        )
        .unwrap();
        let bytes = data_tar(stage.path()).unwrap();
        let mut archive = tar::Archive::new(bytes.as_slice());
        let entries = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.path().unwrap().to_string_lossy().into_owned(),
                    entry.header().entry_type(),
                )
            })
            .collect::<Vec<_>>();
        assert!(entries.iter().any(|(path, kind)| {
            path == "usr/lib/solstone-core-speakers-analyze" && kind.is_dir()
        }));
        assert!(entries.iter().any(|(path, kind)| {
            path == "usr/lib/solstone-core-speakers-analyze/libonnxruntime.so.1" && kind.is_file()
        }));
    }
}
