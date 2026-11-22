// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::Path;
use std::sync::Arc;

use solstone_core_format::stored_markdown::{Document, MAX_RECEIPT};
use solstone_core_journal_io::{JournalRoot, media::MediaDirectory};

/// Both publication paths hold the output's lock. A receipt is optional, and
/// is invalidated first because two writers can produce identical English.
pub(super) fn with_receipt(
    journal: &Path,
    path: &Path,
    document: &Document,
    write: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
    let root = Arc::new(JournalRoot::open(journal).map_err(io::Error::other)?);
    let relative = path.strip_prefix(journal).map_err(io::Error::other)?;
    let (parent, name) = relative.parent().zip(relative.file_name()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "missing display receipt path")
    })?;
    let next = document.display_receipt_bytes().unwrap_or_else(|error| {
        log::warn!("talent display receipt unavailable: {error}");
        None
    });
    let previous = match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.len() > MAX_RECEIPT as u64 => None,
        Ok(_) => MediaDirectory::open(Arc::clone(&root), parent)?
            .read_file_bounded(name, MAX_RECEIPT)?
            .map(|file| file.bytes)
            .filter(|bytes| !bytes.is_empty()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    let changed = previous != next;
    if changed && previous.is_some() {
        publish(&root, parent, name, &[])?;
    }
    let body_changed = write()?;
    if changed
        && let Some(bytes) = next
        && let Err(error) = publish(&root, parent, name, &bytes)
    {
        log::warn!("talent display receipt not saved: {error}");
    }
    Ok(body_changed || changed)
}

#[cfg(unix)]
fn publish(
    root: &JournalRoot,
    parent: &Path,
    name: &std::ffi::OsStr,
    bytes: &[u8],
) -> io::Result<()> {
    use solstone_core_journal_io::{
        FlatDirectory, atomic::atomic_replace_bound_checked, create_or_open_flat_directory_bound,
    };
    let mut directory = FlatDirectory::from_root(root).map_err(io::Error::other)?;
    let mut path = root.canonical_path().to_owned();
    for component in parent.components() {
        let name = component.as_os_str();
        directory = create_or_open_flat_directory_bound(&directory, name, 0o700, &path)
            .map_err(io::Error::other)?;
        path.push(name);
    }
    atomic_replace_bound_checked(&directory, name, bytes, 0o600, &|| {
        root.revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        directory.revalidate().map_err(|error| error.to_string())
    })
    .map_err(io::Error::other)?;
    Ok(())
}

#[cfg(windows)]
fn publish(
    root: &JournalRoot,
    parent: &Path,
    name: &std::ffi::OsStr,
    bytes: &[u8],
) -> io::Result<()> {
    use solstone_core_journal_io::{
        AtomicWriteOptions, WindowsFlatDirectory, atomic_replace_checked,
        create_or_open_windows_flat_directory_bound,
    };
    let mut directory: Option<WindowsFlatDirectory> = None;
    let mut path = root.canonical_path().to_owned();
    for component in parent.components() {
        let name = component.as_os_str();
        let next = match &directory {
            Some(dir) => create_or_open_windows_flat_directory_bound(dir, name, &path),
            None => create_or_open_windows_flat_directory_bound(root, name, &path),
        }
        .map_err(io::Error::other)?;
        directory = Some(next);
        path.push(name);
    }
    let directory = directory
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing receipt directory"))?;
    atomic_replace_checked(
        path.join(name),
        bytes,
        AtomicWriteOptions { mode: Some(0o600) },
        &|| {
            root.revalidate_canonical_binding()
                .map_err(|error| error.to_string())?;
            directory
                .revalidate_bound()
                .map_err(|error| error.to_string())
        },
    )
    .map_err(io::Error::other)
}
