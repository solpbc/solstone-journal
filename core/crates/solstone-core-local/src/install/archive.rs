// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("archive member escapes destination: {0}")]
    PathEscape(String),
    #[error("sha256 mismatch")]
    DigestMismatch,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("download failed: {0}")]
    Download(String),
}

pub fn verify_sha256(path: &Path, expected: &str) -> Result<String, ArchiveError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut chunk = [0_u8; 1024 * 1024];
    loop {
        let size = file.read(&mut chunk)?;
        if size == 0 {
            break;
        }
        digest.update(&chunk[..size]);
    }
    let actual: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(ArchiveError::DigestMismatch);
    }
    Ok(actual)
}

pub fn download(
    url: &str,
    destination: &Path,
    expected_sha256: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), ArchiveError> {
    let response = ureq::get(url)
        .call()
        .map_err(|error| ArchiveError::Download(error.to_string()))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let parent = destination
        .parent()
        .ok_or_else(|| ArchiveError::Download("destination has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.part",
        destination.file_name().unwrap().to_string_lossy()
    ));
    let mut out = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    let mut body = response.into_body().into_reader();
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let size = body.read(&mut chunk)?;
        if size == 0 {
            break;
        }
        out.write_all(&chunk[..size])?;
        digest.update(&chunk[..size]);
        received += size as u64;
        progress(received, total);
    }
    out.sync_all()?;
    let actual: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected_sha256 {
        let _ = fs::remove_file(&temporary);
        return Err(ArchiveError::DigestMismatch);
    }
    fs::rename(temporary, destination)?;
    Ok(())
}

pub fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<(), ArchiveError> {
    let mut tar = tar::Archive::new(GzDecoder::new(File::open(archive)?));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        let output = destination.join(&path);
        if !output.starts_with(destination) {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        entry.unpack(output)?;
    }
    Ok(())
}

pub fn make_executable(path: &Path) -> Result<(), ArchiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

pub fn clear_macos_quarantine(path: &Path) -> Result<(), ArchiveError> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(path)
            .status()?;
        if !status.success() {
            return Err(ArchiveError::Download("xattr failed".to_owned()));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn snapshot_tree(parent: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
    fn visit(root: &Path, here: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(here)? {
            let entry = entry?;
            let path = entry.path();
            output.push(path.strip_prefix(root).expect("under root").to_path_buf());
            if entry.file_type()?.is_dir() {
                visit(root, &path, output)?;
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    if parent.exists() {
        visit(parent, parent, &mut output)?;
    }
    output.sort();
    Ok(output)
}
