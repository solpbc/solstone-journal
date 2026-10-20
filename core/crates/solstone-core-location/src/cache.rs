// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::accuracy::{Installed, asset_root, read_installed_strict};
use sha2::{Digest, Sha256};
use solstone_core_artifact_download::generation::{Generation, Member, hold_write};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use std::{fs, io, path::Path};

pub(crate) fn inventory(installed: &Installed) -> io::Result<Generation> {
    Generation::new(
        crate::install::UNIT,
        installed
            .files
            .iter()
            .map(|f| Member {
                file: f.file.clone(),
                sha256: f.sha256.clone(),
                size_bytes: f.size_bytes,
            })
            .collect(),
    )
}

pub(crate) fn plain(path: &Path, directory: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return false;
            }
        }
        if directory {
            m.file_type().is_dir()
        } else {
            m.file_type().is_file()
        }
    })
}

fn exact(installed: &Installed, directory: &Path) -> bool {
    if !plain(directory, true) {
        return false;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    let mut count = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        if !plain(&entry.path(), false)
            || !installed
                .files
                .iter()
                .any(|f| entry.file_name() == f.file.as_str())
        {
            return false;
        }
        count += 1;
    }
    count == installed.files.len() && inventory(installed).is_ok_and(|g| g.verify(directory))
}

fn use_directory(journal: &Path, installed: &Installed) -> std::path::PathBuf {
    if installed.cache_generation.is_some() {
        installed.directory(journal)
    } else {
        asset_root(journal)
            .join("legacy-use")
            .join(if installed.folder.is_empty() {
                &installed.version
            } else {
                &installed.folder
            })
    }
}

pub(crate) fn hold_use(
    journal: &Path,
    installed: &Installed,
) -> io::Result<solstone_core_journal_io::FileLock> {
    solstone_core_artifact_download::generation::hold_use(&use_directory(journal, installed))
}

pub(crate) fn remove_legacy(journal: &Path, installed: &Installed) -> io::Result<()> {
    if installed.cache_generation.is_some() {
        return Ok(());
    }
    let _hold = match hold_write(&use_directory(journal, installed)) {
        Ok(hold) => hold,
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(e) => return Err(e),
    };
    let directory = installed.directory(journal);
    if !plain(&directory, true) {
        return Ok(());
    }
    // Frozen releases never created this control file. Preserve an owner entry
    // even while resuming a retirement we admitted earlier.
    if fs::symlink_metadata(directory.join(".generation.json")).is_ok() {
        return Ok(());
    }
    let record = inventory(installed)?;
    let retiring = fs::read(directory.join(".retiring.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Generation>(&b).ok())
        .as_ref()
        == Some(&record);
    if !retiring && !exact(installed, &directory) {
        return Ok(());
    }
    record.retire(&directory)
}

/// Record verified legacy provenance outside the tree frozen releases inventory.
pub(crate) fn remember_legacy(journal: &Path, installed: &Installed) -> io::Result<()> {
    if installed.cache_generation.is_some() || !installed.valid_paths() {
        return Ok(());
    }
    let directory = installed.directory(journal);
    if !exact(installed, &directory) {
        return Ok(());
    }
    let name = directory
        .file_name()
        .ok_or_else(|| io::Error::other("legacy pack folder missing"))?
        .to_string_lossy();
    let path = asset_root(journal)
        .join("legacy")
        .join(format!("{:x}.json", Sha256::digest(name.as_bytes())));
    atomic_replace(
        path,
        &serde_json::to_vec(installed)?,
        AtomicWriteOptions::default(),
    )
    .map_err(io::Error::other)
}

/// Called under daily admission and publication exclusion. Unreadable control
/// truth never authorizes collection. Genuine absence revokes all generations.
pub(crate) fn sweep(journal: &Path, offered: Option<&crate::install::Offered>) {
    let installed = match read_installed_strict(journal) {
        Ok(i) => i,
        Err(_) => return,
    };
    let preserve_previous = installed.is_some();
    let current = match installed.as_ref().map(inventory).transpose() {
        Ok(i) => i,
        Err(_) => return,
    };
    let mut protected: Vec<_> = current.into_iter().collect();
    if installed.is_some()
        && let Some(offered) = offered
    {
        let Ok(current_offer) = Generation::new(
            crate::install::UNIT,
            offered
                .files
                .iter()
                .map(|f| Member {
                    file: f.file.clone(),
                    sha256: f.sha256.clone(),
                    size_bytes: f.size_bytes,
                })
                .collect(),
        ) else {
            return;
        };
        protected.push(current_offer);
    }
    let predecessor = if preserve_previous {
        match solstone_core_assets::windows_predecessor::retention_predecessor() {
            Some(Ok(p)) => Some(p),
            Some(Err(reason)) => {
                log::warn!("{reason}");
                return;
            }
            None => None,
        }
    } else {
        None
    };
    let root = asset_root(journal);
    let _ = solstone_core_artifact_download::generation::collect(
        &root.join("generations"),
        crate::install::UNIT,
        &protected,
        preserve_previous,
    );
    let ledger = root.join("legacy");
    if !plain(&ledger, true) {
        return;
    }
    let Ok(entries) = fs::read_dir(&ledger) else {
        return;
    };
    for entry in entries.flatten() {
        if !plain(&entry.path(), false) || entry.metadata().is_ok_and(|m| m.len() > 65536) {
            continue;
        }
        let Some(legacy) = fs::read(entry.path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Installed>(&b).ok())
        else {
            continue;
        };
        if legacy.cache_generation.is_some() || !legacy.valid_paths() {
            continue;
        }
        let directory = legacy.directory(journal);
        if installed
            .as_ref()
            .is_some_and(|i| i.directory(journal) == directory)
        {
            continue;
        }
        if predecessor.is_some_and(|p| {
            p.protects(
                crate::install::UNIT,
                &legacy
                    .files
                    .iter()
                    .map(|f| (f.sha256.as_str(), f.size_bytes))
                    .collect::<Vec<_>>(),
            )
        }) {
            continue;
        }
        if remove_legacy(journal, &legacy).is_ok() && !directory.exists() {
            let _ = fs::remove_file(entry.path());
        }
    }
}
