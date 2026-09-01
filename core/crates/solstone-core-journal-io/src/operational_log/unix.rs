// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unix exclusive stage, handle-bound publish, and bound lease probe.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, FcntlArg, OFlag, fcntl, openat};
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};
use nix::unistd::{UnlinkatFlags, unlinkat};

use super::create::OplogCreateError;
use super::namespace::OplogDayHealth;
use crate::atomic::{allocate_bound_stage, publish_open_stage_no_replace};
use crate::flat_directory::stat_entry;
use crate::journal_root::JournalEntryKind;
use crate::lease::{LeaseProbe, SelfLease, acquire_self_lease, probe_file_lease};

const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct UnixIdentity {
    dev: u64,
    ino: u64,
}

pub(super) struct StagedFile {
    pub file: File,
    pub stage_name: OsString,
    pub identity: UnixIdentity,
}

pub(super) fn stage_exclusive(
    health: &OplogDayHealth,
    dest: &OsStr,
) -> Result<StagedFile, OplogCreateError> {
    let diagnostic = health.health().diagnostic_path();
    let (stage_name, stage_file) = allocate_bound_stage(health.health(), dest, diagnostic)
        .map_err(|_| OplogCreateError::io())?;
    stage_file
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|_| {
            let _ = unlinkat(
                health.health(),
                stage_name.as_os_str(),
                UnlinkatFlags::NoRemoveDir,
            );
            OplogCreateError::io()
        })?;
    set_append(&stage_file).map_err(|_| {
        let _ = unlinkat(
            health.health(),
            stage_name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        );
        OplogCreateError::io()
    })?;
    let identity = identity_of(&stage_file).map_err(|_| {
        let _ = unlinkat(
            health.health(),
            stage_name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        );
        OplogCreateError::io()
    })?;
    Ok(StagedFile {
        file: stage_file,
        stage_name,
        identity,
    })
}

pub(super) fn lease_staged(file: &File) -> Result<Option<SelfLease>, OplogCreateError> {
    acquire_self_lease(file).map_err(|_| OplogCreateError::io())
}

pub(super) fn publish_handle_bound(
    health: &OplogDayHealth,
    staged: StagedFile,
    dest: &OsStr,
) -> Result<File, PublishOutcome> {
    match linkat_empty_path(&staged.file, health.health(), dest) {
        Ok(()) => {
            unlink_stage_if_ours(health, &staged.stage_name, staged.identity);
            Ok(staged.file)
        }
        Err(Errno::EEXIST) => Err(PublishOutcome::Occupied(staged)),
        Err(_) => match publish_open_stage_no_replace(
            health.health(),
            staged.stage_name.as_os_str(),
            dest,
            staged.file,
        ) {
            Ok(file) => Ok(file),
            Err(crate::errors::AtomicWriteError::Io { source, .. })
                if source.kind() == io::ErrorKind::AlreadyExists =>
            {
                Err(PublishOutcome::OccupiedName {
                    identity: staged.identity,
                })
            }
            Err(_) => Err(PublishOutcome::NameBasedIo),
        },
    }
}

pub(super) fn publish_name_based(
    health: &OplogDayHealth,
    staged: StagedFile,
    dest: &OsStr,
) -> Result<File, PublishOutcome> {
    match nix::unistd::linkat(
        health.health(),
        staged.stage_name.as_os_str(),
        health.health(),
        dest,
        AtFlags::empty(),
    ) {
        Ok(()) => match dest_identity(health, dest) {
            Ok(identity) if identity == staged.identity => {
                unlink_stage_if_ours(health, staged.stage_name.as_os_str(), staged.identity);
                Ok(staged.file)
            }
            Ok(_) => Err(PublishOutcome::WrongIdentityPublished { file: staged.file }),
            Err(_) => Err(PublishOutcome::IoAfterPublish { file: staged.file }),
        },
        Err(Errno::EEXIST) => {
            let _ = unlinkat(
                health.health(),
                staged.stage_name.as_os_str(),
                UnlinkatFlags::NoRemoveDir,
            );
            Err(PublishOutcome::OccupiedName {
                identity: staged.identity,
            })
        }
        Err(_) => Err(PublishOutcome::NameBasedIo),
    }
}

fn dest_identity(health: &OplogDayHealth, dest: &OsStr) -> io::Result<UnixIdentity> {
    let status = fstatat(health.health(), dest, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(errno_io)?;
    Ok(UnixIdentity {
        dev: status.st_dev,
        ino: status.st_ino,
    })
}

pub(super) enum PublishOutcome {
    Occupied(StagedFile),
    OccupiedName {
        identity: UnixIdentity,
    },
    #[allow(dead_code)]
    Io(StagedFile),
    NameBasedIo,
    WrongIdentityPublished {
        file: File,
    },
    #[allow(dead_code)]
    IoAfterPublish {
        file: File,
    },
}

pub(super) fn rollback_stage(
    health: &OplogDayHealth,
    staged: &StagedFile,
) -> Result<(), OplogCreateError> {
    match unlinkat(
        health.health(),
        staged.stage_name.as_os_str(),
        UnlinkatFlags::NoRemoveDir,
    ) {
        Ok(()) => Ok(()),
        Err(Errno::ENOENT) => Ok(()),
        Err(_) => Err(OplogCreateError::own_residue()),
    }
}

pub(super) fn dest_is_foreign(
    health: &OplogDayHealth,
    dest: &OsStr,
    expected: UnixIdentity,
) -> Result<bool, OplogCreateError> {
    match stat_entry(health.health(), dest) {
        Ok(Some(entry)) if entry.kind == JournalEntryKind::RegularFile => Ok(UnixIdentity {
            dev: entry.device,
            ino: entry.inode,
        } != expected),
        Ok(Some(_)) | Ok(None) => Err(OplogCreateError::io()),
        Err(_) => Err(OplogCreateError::io()),
    }
}

pub(super) fn probe_named(health: &OplogDayHealth, leaf: &OsStr) -> LeaseProbe {
    let descriptor = match openat(health.health(), leaf, FILE_FLAGS, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(_) => return LeaseProbe::Indeterminate,
    };
    let file = File::from(descriptor);
    match fstat(&file) {
        Ok(status)
            if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG => {}
        _ => return LeaseProbe::Indeterminate,
    }
    probe_file_lease(&file)
}

fn set_append(file: &File) -> io::Result<()> {
    let raw = fcntl(file, FcntlArg::F_GETFL).map_err(errno_io)?;
    let flags = OFlag::from_bits_truncate(raw) | OFlag::O_APPEND;
    fcntl(file, FcntlArg::F_SETFL(flags)).map_err(errno_io)?;
    Ok(())
}

fn identity_of(file: &File) -> io::Result<UnixIdentity> {
    let status = fstat(file).map_err(errno_io)?;
    Ok(UnixIdentity {
        dev: status.st_dev,
        ino: status.st_ino,
    })
}

fn linkat_empty_path(file: &File, directory: impl AsFd, dest: &OsStr) -> Result<(), Errno> {
    nix::unistd::linkat(file, "", directory, dest, AtFlags::AT_EMPTY_PATH)
}

fn unlink_stage_if_ours(health: &OplogDayHealth, stage_name: &OsStr, expected: UnixIdentity) {
    match fstatat(health.health(), stage_name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status)
            if UnixIdentity {
                dev: status.st_dev,
                ino: status.st_ino,
            } == expected =>
        {
            let _ = unlinkat(health.health(), stage_name, UnlinkatFlags::NoRemoveDir);
        }
        _ => {}
    }
}

fn errno_io(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}
