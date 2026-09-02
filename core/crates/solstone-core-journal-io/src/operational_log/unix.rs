// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unix exclusive stage, no-replace rename, and bound lease probe.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::unix::fs::PermissionsExt;

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl, openat};
use nix::sys::stat::{Mode, SFlag, fstat};

use super::create::OplogCreatePrimitive;
use super::namespace::OplogDayHealth;
use super::reason::{NamedOccupant, NamedOpen, OplogFileIdentity, StageError, StageLeftoverCause};
use crate::atomic::allocate_bound_stage;
use crate::lease::{LeaseProbe, SelfLease, acquire_self_lease, probe_file_lease};

const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

pub(super) struct StagedFile {
    pub file: File,
    pub stage_name: OsString,
    pub identity: OplogFileIdentity,
}

#[allow(clippy::unnecessary_cast)]
fn identity_and_nlink_from_stat(status: &nix::sys::stat::FileStat) -> (OplogFileIdentity, u64) {
    (
        OplogFileIdentity::from_unix(status.st_dev as u64, status.st_ino as u64),
        status.st_nlink as u64,
    )
}

pub(super) fn stage_exclusive(
    health: &OplogDayHealth,
    dest: &OsStr,
) -> Result<StagedFile, StageError> {
    let diagnostic = health.health().diagnostic_path();
    let (stage_name, stage_file) = allocate_bound_stage(health.health(), dest, diagnostic)
        .map_err(|_| StageError::Allocate)?;
    super::create::barrier(OplogCreatePrimitive::AfterAllocateBeforePrepare);
    if super::create::force_stage_permission_fail()
        || stage_file
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .is_err()
    {
        return Err(leftover(
            stage_file,
            stage_name,
            StageLeftoverCause::Permission,
        ));
    }
    if super::create::force_stage_append_fail() || set_append(&stage_file).is_err() {
        return Err(leftover(stage_file, stage_name, StageLeftoverCause::Append));
    }
    if super::create::force_stage_identity_fail() {
        return Err(leftover(
            stage_file,
            stage_name,
            StageLeftoverCause::Identity,
        ));
    }
    let Ok(identity) = identity_of(&stage_file) else {
        return Err(leftover(
            stage_file,
            stage_name,
            StageLeftoverCause::Identity,
        ));
    };
    Ok(StagedFile {
        file: stage_file,
        stage_name,
        identity,
    })
}

fn leftover(file: File, name: OsString, cause: StageLeftoverCause) -> StageError {
    let identity = identity_of(&file).ok();
    drop(file);
    StageError::Leftover {
        name,
        cause,
        identity,
    }
}

pub(super) fn lease_staged(file: &File) -> io::Result<Option<SelfLease>> {
    acquire_self_lease(file).map_err(|error| io::Error::other(error.to_string()))
}

pub(super) fn rename_stage(
    health: &OplogDayHealth,
    staged: &StagedFile,
    dest: &OsStr,
) -> io::Result<()> {
    if super::create::force_publish_io() {
        return Err(io::Error::from_raw_os_error(nix::libc::EIO));
    }
    crate::claim_remove::rename_no_replace(health.health(), staged.stage_name.as_os_str(), dest)
}

pub(super) fn inspect_named(health: &OplogDayHealth, name: &OsStr) -> io::Result<NamedOccupant> {
    open_named(health, name).map(|opened| opened.occupant())
}

pub(super) fn open_named(health: &OplogDayHealth, name: &OsStr) -> io::Result<NamedOpen> {
    match openat(health.health(), name, FILE_FLAGS, Mode::empty()) {
        Err(Errno::ENOENT) => Ok(NamedOpen::Absent),
        Err(error) => Err(errno_io(error)),
        Ok(descriptor) => {
            let file = File::from(descriptor);
            match fstat(&file) {
                Ok(status)
                    if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT
                        == SFlag::S_IFREG =>
                {
                    let (identity, nlink) = identity_and_nlink_from_stat(&status);
                    Ok(NamedOpen::Regular {
                        file,
                        identity,
                        nlink,
                    })
                }
                Ok(_) => Ok(NamedOpen::Other),
                Err(error) => Err(errno_io(error)),
            }
        }
    }
}

pub(super) fn probe_named(
    health: &OplogDayHealth,
    leaf: &OsStr,
    identity: OplogFileIdentity,
) -> LeaseProbe {
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
    let Ok(actual) = identity_of(&file) else {
        return LeaseProbe::Indeterminate;
    };
    if actual != identity {
        return LeaseProbe::Indeterminate;
    }
    probe_file_lease(&file)
}

fn set_append(file: &File) -> io::Result<()> {
    let raw = fcntl(file, FcntlArg::F_GETFL).map_err(errno_io)?;
    let flags = OFlag::from_bits_truncate(raw) | OFlag::O_APPEND;
    fcntl(file, FcntlArg::F_SETFL(flags)).map_err(errno_io)?;
    Ok(())
}

pub(super) fn identity_of(file: &File) -> io::Result<OplogFileIdentity> {
    let status = fstat(file).map_err(errno_io)?;
    Ok(identity_and_nlink_from_stat(&status).0)
}

pub(super) fn nlink_of(file: &File) -> io::Result<u64> {
    let status = fstat(file).map_err(errno_io)?;
    Ok(identity_and_nlink_from_stat(&status).1)
}

fn errno_io(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    #[test]
    fn raw_stat_normalization_matches_metadata_ext_across_hard_link() {
        let temp = tempfile::tempdir_in("/var/tmp").unwrap();
        let path = temp.path().join("original");
        std::fs::write(&path, b"identity").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let identity = identity_of(&file).unwrap();
        let nlink = nlink_of(&file).unwrap();
        let metadata = file.metadata().unwrap();
        assert_eq!(identity.dev, metadata.dev());
        assert_eq!(identity.ino, metadata.ino());
        assert_eq!(nlink, metadata.nlink());

        let linked = temp.path().join("linked");
        std::fs::hard_link(&path, &linked).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let linked_identity = identity_of(&file).unwrap();
        let linked_nlink = nlink_of(&file).unwrap();
        let linked_metadata = file.metadata().unwrap();
        assert_eq!(linked_identity.dev, identity.dev);
        assert_eq!(linked_identity.ino, identity.ino);
        assert_eq!(linked_nlink, nlink + 1);
        assert_eq!(linked_identity.dev, linked_metadata.dev());
        assert_eq!(linked_identity.ino, linked_metadata.ino());
        assert_eq!(linked_nlink, linked_metadata.nlink());
    }
}
