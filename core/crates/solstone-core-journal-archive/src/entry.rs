// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
#[cfg(unix)]
use std::fs::File;

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

/// A verified regular file opened from an inventory entry.
pub struct OpenedInventoryFile {
    #[cfg(unix)]
    file: File,
    #[cfg(windows)]
    bytes: Vec<u8>,
    inventoried_size: u64,
}

impl OpenedInventoryFile {
    #[cfg(unix)]
    pub(crate) fn new(file: File, inventoried_size: u64) -> Self {
        Self {
            file,
            inventoried_size,
        }
    }

    #[cfg(windows)]
    pub(crate) fn from_bytes(bytes: Vec<u8>, inventoried_size: u64) -> Self {
        Self {
            bytes,
            inventoried_size,
        }
    }

    /// Return the entry size captured at inventory time.
    pub fn inventoried_size(&self) -> u64 {
        self.inventoried_size
    }

    /// Consume this wrapper and return its already-verified file descriptor.
    #[cfg(unix)]
    pub fn into_file(self) -> File {
        self.file
    }

    /// Consume this wrapper and return its already-verified complete contents.
    #[cfg(windows)]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}
