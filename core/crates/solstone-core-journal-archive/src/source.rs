// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};

use crate::entry::{DirectoryProof, EntryProof, FileProof};
use crate::{
    ArchiveError, ArchiveMemberName, Inventory, InventoryEntry, JournalEntryKind,
    OpenedInventoryFile,
};

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

/// A frozen, capability-rooted portable archive source.
pub struct ArchiveSource {
    root: OwnedFd,
    inventory: Inventory,
}

impl ArchiveSource {
    /// Acquire `root` once and immediately freeze its portable archive inventory.
    pub fn open(root: &Path) -> Result<Self, ArchiveError> {
        let retained_root = acquire_root(root)?;
        let inventory = crate::inventory::build(&retained_root)?;
        Ok(Self {
            root: retained_root,
            inventory,
        })
    }

    /// Return the inventory frozen when this source was opened.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// Re-open a frozen entry through the retained journal descriptor.
    pub fn open_file(&self, entry: &InventoryEntry) -> Result<OpenedInventoryFile, ArchiveError> {
        let (file, proof) = open_verified_file(&self.root, entry.member_name(), entry.proof())?;
        Ok(OpenedInventoryFile::new(File::from(file), proof.size))
    }

    /// Confirm every recorded directory and leaf identity for an entry.
    pub fn revalidate(&self, entry: &InventoryEntry) -> Result<(), ArchiveError> {
        let file = open_verified_file(&self.root, entry.member_name(), entry.proof())?.0;
        drop(file);
        Ok(())
    }
}

pub(crate) fn open_initial_directory(
    parent: &impl AsFd,
    name: &OsStr,
    member: &ArchiveMemberName,
    before: &FileStat,
) -> Result<(OwnedFd, DirectoryProof), ArchiveError> {
    let kind = classify(before);
    if kind != JournalEntryKind::Directory {
        return Err(ArchiveError::UnsafeJournalEntry {
            member: member.clone(),
            kind,
        });
    }
    let opened = open_directory(parent, name, Some(member), true)?;
    let after = stat_fd(&opened, Some(member), "stat opened journal directory")?;
    let before_proof = directory_proof(before)?;
    if directory_proof(&after)? != before_proof {
        return Err(changed(Some(member)));
    }
    Ok((opened, before_proof))
}

pub(crate) fn open_initial_file(
    parent: &impl AsFd,
    name: &OsStr,
    member: &ArchiveMemberName,
    before: &FileStat,
) -> Result<FileProof, ArchiveError> {
    let kind = classify(before);
    if kind != JournalEntryKind::RegularFile {
        return Err(ArchiveError::UnsafeJournalEntry {
            member: member.clone(),
            kind,
        });
    }
    let opened = open_regular_file(parent, name, Some(member), true)?;
    let after = stat_fd(&opened, Some(member), "stat opened journal file")?;
    let before_proof = file_proof(before)?;
    if file_proof(&after)? != before_proof {
        return Err(changed(Some(member)));
    }
    Ok(before_proof)
}

pub(crate) fn list_directory(
    directory: &impl AsFd,
    member: Option<&ArchiveMemberName>,
) -> Result<Vec<OsString>, ArchiveError> {
    let mut directory = nix::dir::Dir::openat(directory, ".", DIRECTORY_FLAGS, Mode::empty())
        .map_err(|error| source_io("open journal directory for listing", member, error))?;
    let mut names = Vec::new();
    for entry in directory.iter() {
        let entry = entry.map_err(|error| source_io("list journal directory", member, error))?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        names.push(OsString::from_vec(bytes.to_vec()));
    }
    names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    Ok(names)
}

pub(crate) fn root_entry_missing(root: &OwnedFd, name: &OsStr) -> Result<bool, ArchiveError> {
    match fstatat(root, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(false),
        Err(Errno::ENOENT) => Ok(true),
        Err(error) => Err(source_io("stat archive root", None, error)),
    }
}

pub(crate) fn utf8_component(
    name: &OsStr,
    member: &ArchiveMemberName,
) -> Result<String, ArchiveError> {
    std::str::from_utf8(name.as_bytes())
        .map(str::to_owned)
        .map_err(|_| ArchiveError::UnsafeJournalEntry {
            member: member.clone(),
            kind: JournalEntryKind::Other,
        })
}

pub(crate) fn member_name(components: &[OsString]) -> Result<ArchiveMemberName, ArchiveError> {
    let mut rendered = Vec::with_capacity(components.len());
    for component in components {
        let placeholder = ArchiveMemberName::new("<invalid>".to_owned());
        rendered.push(utf8_component(component, &placeholder)?);
    }
    Ok(ArchiveMemberName::new(rendered.join("/")))
}

pub(crate) fn is_directory(stat: &FileStat) -> bool {
    classify(stat) == JournalEntryKind::Directory
}

pub(crate) fn is_regular(stat: &FileStat) -> bool {
    classify(stat) == JournalEntryKind::RegularFile
}

pub(crate) fn stat_entry_for_count(
    parent: &impl AsFd,
    name: &OsStr,
    member: &ArchiveMemberName,
) -> Result<FileStat, ArchiveError> {
    stat_entry(parent, name, Some(member), "stat journal entry")
}

pub(crate) fn classify(stat: &FileStat) -> JournalEntryKind {
    classify_mode(SFlag::from_bits_truncate(stat.st_mode))
}

pub(crate) fn classify_mode(mode: SFlag) -> JournalEntryKind {
    match mode & SFlag::S_IFMT {
        SFlag::S_IFREG => JournalEntryKind::RegularFile,
        SFlag::S_IFDIR => JournalEntryKind::Directory,
        SFlag::S_IFLNK => JournalEntryKind::Symlink,
        SFlag::S_IFIFO => JournalEntryKind::Fifo,
        SFlag::S_IFSOCK => JournalEntryKind::Socket,
        SFlag::S_IFCHR => JournalEntryKind::CharacterDevice,
        SFlag::S_IFBLK => JournalEntryKind::BlockDevice,
        _ => JournalEntryKind::Other,
    }
}

fn acquire_root(root: &Path) -> Result<OwnedFd, ArchiveError> {
    if !root.is_absolute() {
        return Err(ArchiveError::InvalidJournal {
            root: root.to_path_buf(),
            reason: "journal root must be absolute",
        });
    }
    let canonical = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ArchiveError::InvalidJournal {
                root: root.to_path_buf(),
                reason: "journal root does not exist",
            });
        }
        Err(source) => {
            return Err(ArchiveError::SourceIo {
                operation: "canonicalize journal root",
                member: None,
                source,
            });
        }
    };
    let metadata = fs::metadata(&canonical).map_err(|source| ArchiveError::SourceIo {
        operation: "stat canonical journal root",
        member: None,
        source,
    })?;
    if !metadata.is_dir() {
        return Err(ArchiveError::InvalidJournal {
            root: root.to_path_buf(),
            reason: "journal root is not a directory",
        });
    }
    let expected = DirectoryProof {
        device: metadata.dev(),
        inode: metadata.ino(),
    };

    let mut current = open("/", DIRECTORY_FLAGS, Mode::empty())
        .map_err(|error| source_io("open filesystem root", None, error))?;
    let components = canonical_components(&canonical, root)?;
    for component in components {
        let before = stat_entry(&current, &component, None, "stat canonical root component")
            .map_err(acquisition_error)?;
        if !is_directory(&before) {
            return Err(changed(None));
        }
        let opened = open_directory(&current, &component, None, true)?;
        let after = stat_fd(&opened, None, "stat opened canonical root component")?;
        if directory_proof(&before)? != directory_proof(&after)? {
            return Err(changed(None));
        }
        current = opened;
    }
    let final_stat = stat_fd(&current, None, "stat acquired journal root")?;
    if directory_proof(&final_stat)? != expected {
        return Err(changed(None));
    }
    Ok(current)
}

fn canonical_components(canonical: &Path, original: &Path) -> Result<Vec<OsString>, ArchiveError> {
    let mut components = Vec::new();
    for component in canonical.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) if std::str::from_utf8(name.as_bytes()).is_ok() => {
                components.push(name.to_os_string());
            }
            Component::Normal(_) => {
                return Err(ArchiveError::InvalidJournal {
                    root: original.to_path_buf(),
                    reason: "canonical journal root has a non-UTF-8 ancestor",
                });
            }
            _ => {
                return Err(ArchiveError::InvalidJournal {
                    root: original.to_path_buf(),
                    reason: "canonical journal root is not absolute",
                });
            }
        }
    }
    Ok(components)
}

fn open_verified_file(
    root: &OwnedFd,
    member: &ArchiveMemberName,
    proof: &EntryProof,
) -> Result<(OwnedFd, FileProof), ArchiveError> {
    let mut current = open_verified_route(root, member, proof)?;
    let Some(name) = proof.components.last() else {
        return Err(changed(Some(member)));
    };
    let before = stat_entry(&current, name, Some(member), "stat inventoried file")
        .map_err(|error| revalidation_error(error, member))?;
    if !is_regular(&before) || file_proof(&before)? != proof.file {
        return Err(changed(Some(member)));
    }
    let opened = open_regular_file(&current, name, Some(member), true)?;
    let after = stat_fd(&opened, Some(member), "stat opened inventoried file")?;
    if file_proof(&after)? != proof.file || file_proof(&after)? != file_proof(&before)? {
        return Err(changed(Some(member)));
    }
    current = opened;
    Ok((current, proof.file))
}

fn open_verified_route(
    root: &OwnedFd,
    member: &ArchiveMemberName,
    proof: &EntryProof,
) -> Result<OwnedFd, ArchiveError> {
    if proof.components.len() != proof.directories.len().saturating_add(1) {
        return Err(changed(Some(member)));
    }
    let mut current = openat(root, ".", DIRECTORY_FLAGS, Mode::empty())
        .map_err(|error| source_io("open retained journal root", Some(member), error))?;
    for (name, expected) in proof.components.iter().zip(proof.directories.iter()) {
        let before = stat_entry(&current, name, Some(member), "stat inventoried directory")
            .map_err(|error| revalidation_error(error, member))?;
        if !is_directory(&before) || directory_proof(&before)? != *expected {
            return Err(changed(Some(member)));
        }
        let opened = open_directory(&current, name, Some(member), true)?;
        let after = stat_fd(&opened, Some(member), "stat opened inventoried directory")?;
        if directory_proof(&after)? != *expected
            || directory_proof(&after)? != directory_proof(&before)?
        {
            return Err(changed(Some(member)));
        }
        current = opened;
    }
    Ok(current)
}

fn stat_entry(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    operation: &'static str,
) -> Result<FileStat, ArchiveError> {
    fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|error| source_io(operation, member, error))
}

fn stat_fd(
    fd: &impl AsFd,
    member: Option<&ArchiveMemberName>,
    operation: &'static str,
) -> Result<FileStat, ArchiveError> {
    fstat(fd).map_err(|error| source_io(operation, member, error))
}

fn open_directory(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    changed_on_race: bool,
) -> Result<OwnedFd, ArchiveError> {
    openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(|error| {
        if changed_on_race && is_race_error(error) {
            changed(member)
        } else {
            source_io("open journal directory", member, error)
        }
    })
}

fn open_regular_file(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    changed_on_race: bool,
) -> Result<OwnedFd, ArchiveError> {
    openat(parent, name, FILE_FLAGS, Mode::empty()).map_err(|error| {
        if changed_on_race && is_race_error(error) {
            changed(member)
        } else {
            source_io("open journal file", member, error)
        }
    })
}

fn directory_proof(stat: &FileStat) -> Result<DirectoryProof, ArchiveError> {
    Ok(DirectoryProof {
        device: stat_identifier(stat.st_dev)?,
        inode: stat_identifier(stat.st_ino)?,
    })
}

fn file_proof(stat: &FileStat) -> Result<FileProof, ArchiveError> {
    let size = u64::try_from(stat.st_size).map_err(|_| ArchiveError::SourceIo {
        operation: "read regular-file size",
        member: None,
        source: io::Error::new(io::ErrorKind::InvalidData, "regular-file size is negative"),
    })?;
    Ok(FileProof {
        device: stat_identifier(stat.st_dev)?,
        inode: stat_identifier(stat.st_ino)?,
        size,
    })
}

fn stat_identifier(value: impl TryInto<u64>) -> Result<u64, ArchiveError> {
    value.try_into().map_err(|_| ArchiveError::SourceIo {
        operation: "read source file identity",
        member: None,
        source: io::Error::new(io::ErrorKind::InvalidData, "source identity is negative"),
    })
}

fn source_io(
    operation: &'static str,
    member: Option<&ArchiveMemberName>,
    error: Errno,
) -> ArchiveError {
    ArchiveError::SourceIo {
        operation,
        member: member.cloned(),
        source: io::Error::from_raw_os_error(error as i32),
    }
}

fn changed(member: Option<&ArchiveMemberName>) -> ArchiveError {
    ArchiveError::SourceChanged {
        member: member.cloned(),
    }
}

fn is_race_error(error: Errno) -> bool {
    matches!(error, Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP)
}

fn acquisition_error(error: ArchiveError) -> ArchiveError {
    match error {
        ArchiveError::SourceIo {
            source,
            operation,
            member,
        } if source
            .raw_os_error()
            .is_some_and(|raw| is_race_error(Errno::from_raw(raw))) =>
        {
            let _ = (operation, member);
            changed(None)
        }
        other => other,
    }
}

fn revalidation_error(error: ArchiveError, member: &ArchiveMemberName) -> ArchiveError {
    match error {
        ArchiveError::SourceIo { source, .. }
            if source
                .raw_os_error()
                .is_some_and(|raw| is_race_error(Errno::from_raw(raw))) =>
        {
            changed(Some(member))
        }
        other => other,
    }
}
