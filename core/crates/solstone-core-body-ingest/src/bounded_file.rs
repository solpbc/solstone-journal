// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::path::{Component, Path};

use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_NONBLOCK);

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

fn open_descendant_regular(root: &Path, relative: &str) -> io::Result<File> {
    let components = Path::new(relative)
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal document path has an invalid component",
            )),
        })
        .collect::<io::Result<Vec<&OsStr>>>()?;
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

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "journal document exceeds its byte limit",
    )
}

fn nix_io(error: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}
