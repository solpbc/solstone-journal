// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::digest::sha256_hex;
use crate::record::FileRecord;

pub const NON_UNIX_STAGE_MODE: u32 = 0;

#[must_use]
pub fn recorded_mode(requested: u32) -> u32 {
    #[cfg(unix)]
    {
        requested
    }
    #[cfg(not(unix))]
    {
        let _ = requested;
        NON_UNIX_STAGE_MODE
    }
}

#[must_use]
pub fn file_mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        NON_UNIX_STAGE_MODE
    }
}

pub fn write_staged_file(root: &Path, dest: &str, contents: &[u8]) -> io::Result<PathBuf> {
    write_staged_file_mode(root, dest, contents, 0o644)
}

pub fn write_staged_file_mode(
    root: &Path,
    dest: &str,
    contents: &[u8],
    mode: u32,
) -> io::Result<PathBuf> {
    crate::archive::refuse_escape(dest)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.as_str()))?;
    let path = root.join(dest);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, contents)?;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(mode);
        fs::set_permissions(&path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(path)
}

pub fn staged_files(root: &Path) -> io::Result<Vec<String>> {
    Ok(staged_records(root)?
        .into_iter()
        .map(|record| record.dest)
        .collect())
}

pub fn staged_records(root: &Path) -> io::Result<Vec<FileRecord>> {
    let mut dests = Vec::new();
    collect(root, root, &mut dests)?;
    dests.sort();
    dests.dedup();
    let mut records = Vec::new();
    for dest in dests {
        let path = root.join(&dest);
        let bytes = fs::read(&path)?;
        let mode = file_mode(&fs::metadata(&path)?);
        records.push(FileRecord::file(dest, mode, sha256_hex(&bytes)));
    }
    Ok(records)
}

fn collect(root: &Path, dir: &Path, dests: &mut Vec<String>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, dests)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| io::Error::other(error.to_string()))?;
        dests.push(relative.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}
