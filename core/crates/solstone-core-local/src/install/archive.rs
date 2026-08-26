// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;

pub use solstone_core_artifact_download::{
    ArchiveError, DownloadHostPolicy, PRODUCTION_DOWNLOAD_POLICY, clear_macos_quarantine,
    download_verified, download_verified_origin, ensure_verified_url, make_executable, origin_url,
    verify_sha256,
};

pub fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<(), ArchiveError> {
    let mut tar = tar::Archive::new(GzDecoder::new(std::fs::File::open(archive)?));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if escapes_destination(&path) {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        let output = destination.join(&path);
        if !output.starts_with(destination) {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        if (entry.header().entry_type().is_symlink() || entry.header().entry_type().is_hard_link())
            && let Some(link) = entry.link_name()?
            && !link_stays_within(
                destination,
                if entry.header().entry_type().is_symlink() {
                    output.parent().unwrap_or(destination)
                } else {
                    destination
                },
                &link,
            )
        {
            return Err(ArchiveError::PathEscape(format!(
                "{} -> {}",
                path.display(),
                link.display()
            )));
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(output)?;
    }
    Ok(())
}

fn escapes_destination(path: &Path) -> bool {
    path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn link_stays_within(root: &Path, base: &Path, link: &Path) -> bool {
    if link.is_absolute() {
        return false;
    }
    let Ok(relative_base) = base.strip_prefix(root) else {
        return false;
    };
    let mut components: Vec<_> = relative_base
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect();
    for component in link.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => components.push(value),
            Component::ParentDir => {
                if components.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
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
