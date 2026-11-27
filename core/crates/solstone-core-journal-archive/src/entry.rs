// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
#[cfg(unix)]
use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
#[cfg(unix)]
use std::marker::PhantomData;

/// One portable archive member name, always a UTF-8 relative name.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArchiveMemberName(String);

impl ArchiveMemberName {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// Return the portable archive-member spelling, never a host filesystem path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A top-level journal directory omitted by a portable deny-list tree prune.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SkippedRootName(String);

impl SkippedRootName {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// Return the omitted top-level name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A present top-level journal directory included in a portable archive.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IncludedRootName(String);

impl IncludedRootName {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// Return the included top-level name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Platform-native identity retained by an archive proof.
///
/// This stays archive-private: Unix and Windows acquire the proof through
/// different backends, while the portable archive surface never exposes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProofIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows(solstone_core_journal_io::ObjectIdentity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryProof {
    pub(crate) identity: ProofIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileProof {
    pub(crate) identity: ProofIdentity,
    pub(crate) size: u64,
    #[cfg(unix)]
    pub(crate) mtime_sec: i64,
    #[cfg(unix)]
    pub(crate) mtime_nsec: i64,
    #[cfg(windows)]
    pub(crate) observed: solstone_core_journal_io::WindowsInventoryEntry,
}

/// The descriptor-relative route and identities frozen during inventory.
///
/// Every directory between the retained journal root and the leaf has an
/// identity proof. A leaf-only proof would permit a replaced directory to
/// hard-link the original file and evade revalidation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EntryProof {
    pub(crate) components: Box<[OsString]>,
    pub(crate) directories: Box<[DirectoryProof]>,
    pub(crate) file: FileProof,
}

/// The descriptor-relative route and identities frozen for a directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectoryEntryProof {
    pub(crate) components: Box<[OsString]>,
    pub(crate) directories: Box<[DirectoryProof]>,
}

/// One regular file eligible for a portable archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryEntry {
    member_name: ArchiveMemberName,
    proof: EntryProof,
}

impl InventoryEntry {
    pub(crate) fn new(member_name: ArchiveMemberName, proof: EntryProof) -> Self {
        Self { member_name, proof }
    }

    /// Return the portable archive member name.
    pub fn member_name(&self) -> &ArchiveMemberName {
        &self.member_name
    }

    /// Return the size captured when this entry was inventoried.
    pub fn size(&self) -> u64 {
        self.proof.file.size
    }

    pub(crate) fn proof(&self) -> &EntryProof {
        &self.proof
    }

    pub(crate) fn set_live_snapshot_size(&mut self, size: u64) {
        self.proof.file.size = size;
    }
}

/// A frozen archive-source inventory.
#[derive(Debug, Default)]
pub struct Inventory {
    pub(crate) entries: Vec<InventoryEntry>,
    pub(crate) directory_proofs: Vec<DirectoryEntryProof>,
    pub(crate) included_root_names: Vec<IncludedRootName>,
    pub(crate) skipped_root_names: Vec<SkippedRootName>,
    pub(crate) day_count: usize,
    pub(crate) entity_count: usize,
    pub(crate) facet_count: usize,
}

impl Inventory {
    /// Return regular archive entries in lexical member order.
    pub fn entries(&self) -> &[InventoryEntry] {
        &self.entries
    }

    /// Return sorted present top-level directories included in the archive.
    pub fn included_root_names(&self) -> &[IncludedRootName] {
        &self.included_root_names
    }

    /// Return sorted present top-level directories omitted by a tree prune.
    pub fn skipped_root_names(&self) -> &[SkippedRootName] {
        &self.skipped_root_names
    }

    /// Return the frozen number of immediate eight-digit chronicle directories.
    pub fn day_count(&self) -> usize {
        self.day_count
    }

    /// Return the frozen number of immediate entity declarations.
    pub fn entity_count(&self) -> usize {
        self.entity_count
    }

    /// Return the frozen number of immediate facet declarations.
    pub fn facet_count(&self) -> usize {
        self.facet_count
    }
}

enum OpenedBacking<'a> {
    #[cfg(unix)]
    Unix {
        file: File,
        expected_identity: ProofIdentity,
        expected_mtime: Option<(i64, i64)>,
        allow_append_only: bool,
        member: ArchiveMemberName,
        _source: PhantomData<&'a ()>,
    },
    #[cfg(windows)]
    WindowsStrict {
        reader: solstone_core_journal_io::WindowsCheckedFileReader<'a>,
        root: &'a solstone_core_journal_io::JournalRoot,
    },
    #[cfg(windows)]
    WindowsLive {
        reader: solstone_core_journal_io::WindowsLiveFileReader<'a>,
        root: &'a solstone_core_journal_io::JournalRoot,
    },
    Snapshot(Cursor<&'a [u8]>),
}

/// A verified regular file opened from an inventory entry.
pub struct OpenedInventoryFile<'a> {
    backing: OpenedBacking<'a>,
    inventoried_size: u64,
    observed_size_at_open: u64,
    allow_append_only: bool,
}

impl<'a> OpenedInventoryFile<'a> {
    #[cfg(unix)]
    pub(crate) fn new(
        file: File,
        inventoried_size: u64,
        observed_size_at_open: u64,
        expected_identity: ProofIdentity,
        expected_mtime: Option<(i64, i64)>,
        allow_append_only: bool,
        member: ArchiveMemberName,
    ) -> Self {
        Self {
            backing: OpenedBacking::Unix {
                file,
                expected_identity,
                expected_mtime,
                allow_append_only,
                member,
                _source: PhantomData,
            },
            inventoried_size,
            observed_size_at_open,
            allow_append_only,
        }
    }

    #[cfg(windows)]
    pub(crate) fn new(
        reader: solstone_core_journal_io::WindowsCheckedFileReader<'a>,
        root: &'a solstone_core_journal_io::JournalRoot,
        inventoried_size: u64,
    ) -> Self {
        Self {
            backing: OpenedBacking::WindowsStrict { reader, root },
            inventoried_size,
            observed_size_at_open: inventoried_size,
            allow_append_only: false,
        }
    }

    #[cfg(windows)]
    pub(crate) fn new_windows_live(
        reader: solstone_core_journal_io::WindowsLiveFileReader<'a>,
        root: &'a solstone_core_journal_io::JournalRoot,
        allow_append_only: bool,
    ) -> Self {
        let observed_size_at_open = reader.observed_size();
        let inventoried_size = reader.expected_size();
        Self {
            backing: OpenedBacking::WindowsLive { reader, root },
            inventoried_size,
            observed_size_at_open,
            allow_append_only,
        }
    }

    pub(crate) fn from_snapshot(bytes: &'a [u8]) -> Self {
        let size = bytes.len() as u64;
        Self {
            backing: OpenedBacking::Snapshot(Cursor::new(bytes)),
            inventoried_size: size,
            observed_size_at_open: size,
            allow_append_only: false,
        }
    }

    /// Return the entry size captured at inventory time.
    pub fn inventoried_size(&self) -> u64 {
        self.inventoried_size
    }

    /// Whether an append-only file may legally have bytes past `inventoried_size`.
    pub(crate) fn allow_append_only(&self) -> bool {
        self.allow_append_only
    }

    /// Whether an append-only file was observed larger than its `t=0` prefix.
    pub(crate) fn grew_since_inventory(&self) -> bool {
        self.allow_append_only && self.observed_size_at_open > self.inventoried_size
    }

    /// Consume this wrapper and return its already-verified file descriptor.
    #[cfg(unix)]
    pub fn into_file(self) -> File {
        match self.backing {
            OpenedBacking::Unix { file, .. } => file,
            OpenedBacking::Snapshot(_) => {
                panic!("into_file cannot be called on an in-memory live metadata snapshot")
            }
        }
    }

    /// Complete the verified read. On Windows this rechecks leaf metadata,
    /// retained root identity, and (in strict mode) the recursive namespace
    /// witness before releasing the stream. On Unix in live mode this rechecks
    /// identity and size/mtime bounds on the opened descriptor.
    pub fn finish(self) -> Result<(), crate::ArchiveError> {
        match self.backing {
            #[cfg(unix)]
            OpenedBacking::Unix {
                file,
                expected_identity,
                expected_mtime,
                allow_append_only,
                member,
                ..
            } => {
                if !allow_append_only && expected_mtime.is_none() {
                    return Ok(());
                }
                let stat = nix::sys::stat::fstat(&file).map_err(|error| {
                    crate::ArchiveError::SourceIo {
                        operation: "stat opened journal file after read",
                        member: Some(member.clone()),
                        source: io::Error::from_raw_os_error(error as i32),
                    }
                })?;
                #[allow(
                    clippy::useless_conversion,
                    reason = "st_dev and st_ino are signed or narrower on macOS and some Unix targets"
                )]
                let dev =
                    u64::try_from(stat.st_dev).map_err(|_| crate::ArchiveError::SourceChanged {
                        member: Some(member.clone()),
                    })?;
                #[allow(
                    clippy::useless_conversion,
                    reason = "st_dev and st_ino are signed or narrower on macOS and some Unix targets"
                )]
                let ino =
                    u64::try_from(stat.st_ino).map_err(|_| crate::ArchiveError::SourceChanged {
                        member: Some(member.clone()),
                    })?;
                let size = u64::try_from(stat.st_size).map_err(|_| {
                    crate::ArchiveError::SourceChanged {
                        member: Some(member.clone()),
                    }
                })?;
                let identity = ProofIdentity::Unix {
                    device: dev,
                    inode: ino,
                };
                if identity != expected_identity {
                    return Err(crate::ArchiveError::SourceChanged {
                        member: Some(member),
                    });
                }
                if allow_append_only {
                    if size < self.inventoried_size {
                        return Err(crate::ArchiveError::SourceChanged {
                            member: Some(member),
                        });
                    }
                } else {
                    #[allow(clippy::unnecessary_cast)]
                    let mtime_changed = expected_mtime.is_some_and(|(sec, nsec)| {
                        stat.st_mtime as i64 != sec || stat.st_mtime_nsec as i64 != nsec
                    });
                    if size != self.inventoried_size || mtime_changed {
                        return Err(crate::ArchiveError::SourceChanged {
                            member: Some(member),
                        });
                    }
                }
                Ok(())
            }
            #[cfg(windows)]
            OpenedBacking::WindowsStrict { reader, root } => reader
                .finish()
                .map_err(|error| crate::windows_source::map_windows_error(root, error, true)),
            #[cfg(windows)]
            OpenedBacking::WindowsLive { reader, root } => reader
                .finish()
                .map_err(|error| crate::windows_source::map_windows_error(root, error, true)),
            OpenedBacking::Snapshot(_) => Ok(()),
        }
    }

    #[cfg(all(windows, test))]
    pub(crate) fn read_all_checked(mut self) -> Result<Vec<u8>, crate::ArchiveError> {
        let mut bytes = Vec::new();
        self.read_to_end(&mut bytes)
            .map_err(|source| crate::ArchiveError::SourceIo {
                operation: "read observed file",
                member: None,
                source,
            })?;
        self.finish()?;
        Ok(bytes)
    }
}

impl Read for OpenedInventoryFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.backing {
            #[cfg(unix)]
            OpenedBacking::Unix { file, .. } => file.read(buf),
            #[cfg(windows)]
            OpenedBacking::WindowsStrict { reader, .. } => reader.read(buf),
            #[cfg(windows)]
            OpenedBacking::WindowsLive { reader, .. } => reader.read(buf),
            OpenedBacking::Snapshot(cursor) => cursor.read(buf),
        }
    }
}

impl Seek for OpenedInventoryFile<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match &mut self.backing {
            #[cfg(unix)]
            OpenedBacking::Unix { file, .. } => file.seek(pos),
            #[cfg(windows)]
            OpenedBacking::WindowsStrict { reader, .. } => reader.seek(pos),
            #[cfg(windows)]
            OpenedBacking::WindowsLive { reader, .. } => reader.seek(pos),
            OpenedBacking::Snapshot(cursor) => cursor.seek(pos),
        }
    }
}
