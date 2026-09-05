// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path};

#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(windows)]
use std::os::windows::{
    fs::{MetadataExt, OpenOptionsExt},
    io::AsHandle,
};

#[cfg(unix)]
use nix::fcntl::{AtFlags, OFlag, open, openat};
#[cfg(unix)]
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};

#[cfg(windows)]
use solstone_core_journal_io::{
    open_windows_flat_directory_bound, open_windows_regular_file_from_bound_parent,
};

#[cfg(unix)]
const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
#[cfg(unix)]
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_NONBLOCK);
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

pub(crate) fn read_bounded_regular(
    root: &Path,
    relative: &str,
    maximum: usize,
) -> io::Result<Vec<u8>> {
    let mut file = open_descendant_regular(root, relative)?;
    let metadata = file.metadata()?;
    if metadata.len() > maximum as u64 {
        return Err(too_large());
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(maximum)
            .min(maximum),
    );
    file.by_ref()
        .take((maximum as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(too_large());
    }
    Ok(bytes)
}

/// Open one regular source file without following its final path component.
pub(crate) fn open_regular_file(path: &Path) -> io::Result<File> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let leaf = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "body source has no file name")
    })?;
    open_components(parent, &[leaf])
}

/// Open a regular, no-follow descendant beneath `root`.
pub(crate) fn open_descendant_regular(root: &Path, relative: &str) -> io::Result<File> {
    let components = normal_components(Path::new(relative))?;
    open_components(root, &components)
}

fn normal_components(path: &Path) -> io::Result<Vec<&OsStr>> {
    path.components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal document path has an invalid component",
            )),
        })
        .collect()
}

#[cfg(unix)]
fn open_components(root: &Path, components: &[&OsStr]) -> io::Result<File> {
    let (leaf, directories) = components
        .split_last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty document path"))?;

    let mut directory = open(root, DIRECTORY_FLAGS, Mode::empty()).map_err(nix_io)?;
    for component in directories {
        require_entry_kind(&directory, component, SFlag::S_IFDIR)?;
        directory =
            openat(&directory, *component, DIRECTORY_FLAGS, Mode::empty()).map_err(nix_io)?;
    }
    require_entry_kind(&directory, leaf, SFlag::S_IFREG)?;
    let file: OwnedFd = openat(&directory, *leaf, FILE_FLAGS, Mode::empty()).map_err(nix_io)?;
    let stat = fstat(&file).map_err(nix_io)?;
    if !SFlag::from_bits_truncate(stat.st_mode).contains(SFlag::S_IFREG) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "journal document is not a regular file",
        ));
    }
    Ok(File::from(file))
}

#[cfg(unix)]
fn require_entry_kind(directory: &OwnedFd, name: &OsStr, required: SFlag) -> io::Result<()> {
    let stat = fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(nix_io)?;
    let actual = SFlag::from_bits_truncate(stat.st_mode);
    if actual.contains(SFlag::S_IFLNK) {
        return Err(io::Error::from_raw_os_error(nix::libc::ELOOP));
    }
    if !actual.contains(required) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "journal document path has the wrong entry type",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn nix_io(error: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

#[cfg(windows)]
fn open_components(root: &Path, components: &[&OsStr]) -> io::Result<File> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(root)?;
    let metadata = directory.metadata()?;
    if !metadata.file_type().is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "body source root is not a non-reparse directory",
        ));
    }
    open_windows_components(&directory, root, components)
}

#[cfg(windows)]
fn open_windows_components(
    parent: &impl AsHandle,
    parent_diagnostic: &Path,
    components: &[&OsStr],
) -> io::Result<File> {
    let (leaf, directories) = components
        .split_last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty document path"))?;
    open_windows_descendant(parent, parent_diagnostic, directories, leaf)
}

#[cfg(windows)]
fn open_windows_descendant(
    parent: &impl AsHandle,
    parent_diagnostic: &Path,
    directories: &[&OsStr],
    leaf: &OsStr,
) -> io::Result<File> {
    let Some((directory_name, remaining)) = directories.split_first() else {
        return open_windows_regular_file_from_bound_parent(parent, leaf, parent_diagnostic)
            .map_err(flat_directory_io)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound));
    };
    let directory = open_windows_flat_directory_bound(parent, directory_name, parent_diagnostic)
        .map_err(flat_directory_io)?
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    open_windows_descendant(
        &directory,
        &parent_diagnostic.join(directory_name),
        remaining,
        leaf,
    )
}

#[cfg(windows)]
fn flat_directory_io(error: solstone_core_journal_io::FlatDirectoryError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error)
}

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "journal document exceeds its byte limit",
    )
}
