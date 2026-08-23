// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-component `openat`/`fstatat` helpers. No inventory, no proofs.

use std::os::fd::{AsFd, OwnedFd};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};

use crate::error::{ConvergenceError, DurableRole};

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

pub(crate) fn open_dir(
    parent: &impl AsFd,
    name: &str,
) -> Result<Option<OwnedFd>, ConvergenceError> {
    match fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Err(Errno::ENOENT) => return Ok(None),
        Err(source) => return io("stat bound directory", DurableRole::Directory, source),
        Ok(status)
            if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR =>
        {
            return Err(ConvergenceError::Io {
                operation: "stat bound directory",
                role: DurableRole::Directory,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "bound name is not a directory",
                ),
            });
        }
        Ok(_) => {}
    }
    match openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(Some(fd)),
        Err(Errno::ENOENT) => Ok(None),
        Err(source) => io("open bound directory", DurableRole::Directory, source),
    }
}

pub(crate) fn open_file(
    parent: &impl AsFd,
    name: &str,
) -> Result<Option<OwnedFd>, ConvergenceError> {
    match fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Err(Errno::ENOENT) => return Ok(None),
        Err(source) => return io("stat bound file", DurableRole::Record, source),
        Ok(status)
            if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG =>
        {
            return Err(ConvergenceError::Io {
                operation: "stat bound file",
                role: DurableRole::Record,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "bound name is not a regular file",
                ),
            });
        }
        Ok(_) => {}
    }
    let fd = match openat(parent, name, FILE_FLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ENOENT) => return Ok(None),
        Err(source) => return io("open bound file", DurableRole::Record, source),
    };
    let opened = fstat(&fd).map_err(|source| ConvergenceError::Io {
        operation: "fstat bound file",
        role: DurableRole::Record,
        source: std::io::Error::from_raw_os_error(source as i32),
    })?;
    if SFlag::from_bits_truncate(opened.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
        return Err(ConvergenceError::Io {
            operation: "fstat bound file",
            role: DurableRole::Record,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "bound name is not a regular file",
            ),
        });
    }
    Ok(Some(fd))
}

fn io<T>(operation: &'static str, role: DurableRole, source: Errno) -> Result<T, ConvergenceError> {
    Err(ConvergenceError::Io {
        operation,
        role,
        source: std::io::Error::from_raw_os_error(source as i32),
    })
}
