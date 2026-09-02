// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unix exclusive stage, no-replace publish, and bound lease probe.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::fs::PermissionsExt;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, FcntlArg, OFlag, fcntl, openat};
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};

use super::create::OplogCreateError;
use super::namespace::OplogDayHealth;
use crate::atomic::allocate_bound_stage;
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

#[allow(clippy::unnecessary_cast)]
fn identity_and_nlink_from_stat(status: &nix::sys::stat::FileStat) -> (UnixIdentity, u64) {
    (
        UnixIdentity {
            dev: status.st_dev as u64,
            ino: status.st_ino as u64,
        },
        status.st_nlink as u64,
    )
}

pub(super) struct StagedFile {
    pub file: File,
    pub stage_name: OsString,
    pub identity: UnixIdentity,
}

/// Bounded leftover classification after refusing a pathname unlink.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct OplogResidue {
    class: OplogResidueClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogResidueClass {
    OwnNoncanonical,
    OwnLanded,
    ForeignNoncanonical,
    ForeignLanded,
}

impl OplogResidueClass {
    const fn token(self) -> &'static str {
        match self {
            Self::OwnNoncanonical => "oplog_residue_own_noncanonical",
            Self::OwnLanded => "oplog_residue_own_landed",
            Self::ForeignNoncanonical => "oplog_residue_foreign_noncanonical",
            Self::ForeignLanded => "oplog_residue_foreign_landed",
        }
    }
}

impl OplogResidue {
    const fn new(class: OplogResidueClass) -> Self {
        Self { class }
    }

    fn token(self) -> &'static str {
        self.class.token()
    }
}

impl fmt::Display for OplogResidue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

impl fmt::Debug for OplogResidue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogResidue {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

pub(super) fn stage_exclusive(
    health: &OplogDayHealth,
    dest: &OsStr,
) -> Result<StagedFile, OplogCreateError> {
    let diagnostic = health.health().diagnostic_path();
    let (stage_name, stage_file) = allocate_bound_stage(health.health(), dest, diagnostic)
        .map_err(|_| OplogCreateError::io())?;
    if stage_file
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .is_err()
    {
        drop(stage_file);
        let _ = classify_named(health, stage_name.as_os_str(), None);
        return Err(OplogCreateError::own_residue());
    }
    if set_append(&stage_file).is_err() {
        drop(stage_file);
        let _ = classify_named(health, stage_name.as_os_str(), None);
        return Err(OplogCreateError::own_residue());
    }
    let Ok((identity, _)) = identity_of(&stage_file) else {
        drop(stage_file);
        let _ = classify_named(health, stage_name.as_os_str(), None);
        return Err(OplogCreateError::own_residue());
    };
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
    if super::create::force_publish_io() {
        return Err(PublishOutcome::Io(staged));
    }
    match crate::claim_remove::rename_no_replace(
        health.health(),
        staged.stage_name.as_os_str(),
        dest,
    ) {
        Ok(()) => postcheck_published(health, staged, dest),
        Err(error) if error.raw_os_error() == Some(nix::libc::EEXIST) => {
            Err(PublishOutcome::Occupied(staged))
        }
        Err(_) => Err(PublishOutcome::Io(staged)),
    }
}

fn postcheck_published(
    health: &OplogDayHealth,
    staged: StagedFile,
    dest: &OsStr,
) -> Result<File, PublishOutcome> {
    let Ok((dest_identity, dest_nlink)) = dest_identity_and_nlink(health, dest) else {
        return Err(PublishOutcome::IoAfterPublish { file: staged.file });
    };
    let Ok((file_identity, _)) = identity_of(&staged.file) else {
        return Err(PublishOutcome::IoAfterPublish { file: staged.file });
    };
    let residue = if dest_identity != staged.identity || dest_identity != file_identity {
        OplogResidue::new(OplogResidueClass::ForeignLanded)
    } else if dest_nlink != 1 {
        OplogResidue::new(OplogResidueClass::OwnLanded)
    } else {
        return Ok(staged.file);
    };
    match residue.class {
        OplogResidueClass::ForeignLanded => {
            Err(PublishOutcome::WrongIdentityPublished { file: staged.file })
        }
        OplogResidueClass::OwnLanded => Err(PublishOutcome::Aliased { file: staged.file }),
        OplogResidueClass::OwnNoncanonical | OplogResidueClass::ForeignNoncanonical => {
            Err(PublishOutcome::IoAfterPublish { file: staged.file })
        }
    }
}

fn dest_identity_and_nlink(
    health: &OplogDayHealth,
    dest: &OsStr,
) -> io::Result<(UnixIdentity, u64)> {
    if super::create::force_dest_identity_io() {
        return Err(io::Error::from_raw_os_error(nix::libc::EIO));
    }
    let status = fstatat(health.health(), dest, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(errno_io)?;
    Ok(identity_and_nlink_from_stat(&status))
}

pub(super) enum PublishOutcome {
    Occupied(StagedFile),
    Io(StagedFile),
    WrongIdentityPublished { file: File },
    IoAfterPublish { file: File },
    Aliased { file: File },
}

pub(super) fn rollback_stage(
    health: &OplogDayHealth,
    staged: &StagedFile,
) -> Result<(), OplogCreateError> {
    match classify_named(health, staged.stage_name.as_os_str(), Some(staged.identity)) {
        Ok(_) | Err(Errno::ENOENT) => Ok(()),
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

fn identity_of(file: &File) -> io::Result<(UnixIdentity, u64)> {
    let status = fstat(file).map_err(errno_io)?;
    Ok(identity_and_nlink_from_stat(&status))
}

fn classify_named(
    health: &OplogDayHealth,
    name: &OsStr,
    expected: Option<UnixIdentity>,
) -> Result<OplogResidue, Errno> {
    let status = fstatat(health.health(), name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    let (observed, _) = identity_and_nlink_from_stat(&status);
    let own = expected.is_none_or(|expected| expected == observed);
    Ok(OplogResidue::new(if own {
        OplogResidueClass::OwnNoncanonical
    } else {
        OplogResidueClass::ForeignNoncanonical
    }))
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
        let (identity, nlink) = identity_of(&file).unwrap();
        let metadata = file.metadata().unwrap();
        assert_eq!(identity.dev, metadata.dev());
        assert_eq!(identity.ino, metadata.ino());
        assert_eq!(nlink, metadata.nlink());

        let linked = temp.path().join("linked");
        std::fs::hard_link(&path, &linked).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let (linked_identity, linked_nlink) = identity_of(&file).unwrap();
        let linked_metadata = file.metadata().unwrap();
        assert_eq!(linked_identity.dev, identity.dev);
        assert_eq!(linked_identity.ino, identity.ino);
        assert_eq!(linked_nlink, nlink + 1);
        assert_eq!(linked_identity.dev, linked_metadata.dev());
        assert_eq!(linked_identity.ino, linked_metadata.ino());
        assert_eq!(linked_nlink, linked_metadata.nlink());
    }
}
