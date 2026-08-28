// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable direct-entry observations shared by platform-specific directory I/O.

use std::ffi::OsString;

use crate::journal_root::JournalEntryKind;

/// A native-precision modification timestamp from a no-follow metadata result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeMtime {
    /// Whole seconds in the platform metadata timestamp.
    pub seconds: i64,
    /// Nanoseconds in the platform metadata timestamp.
    pub nanoseconds: i64,
}

/// Metadata for one direct entry in a flat directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlatDirectoryEntry {
    /// The direct entry name, with no path interpretation.
    pub name: OsString,
    /// No-follow filesystem kind.
    pub kind: JournalEntryKind,
    /// Platform-native volume or device identity from the no-follow metadata result.
    pub device: u64,
    /// Platform-native file identity representation from the no-follow metadata result.
    pub inode: u64,
    /// Byte size from the no-follow metadata result.
    pub size: u64,
    /// Native-precision modification time from the no-follow metadata result.
    pub mtime: NativeMtime,
}

/// Complete stable observation of a direct regular-file entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileObservation {
    /// Entry metadata observed before and after the complete read.
    pub entry: FlatDirectoryEntry,
    /// Exact bytes read while the stable metadata remained unchanged.
    pub bytes: Vec<u8>,
}

pub(crate) fn same_entry_metadata(left: &FlatDirectoryEntry, right: &FlatDirectoryEntry) -> bool {
    left.kind == right.kind
        && left.device == right.device
        && left.inode == right.inode
        && left.size == right.size
        && left.mtime == right.mtime
}

#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn same_observation(left: &FileObservation, right: &FileObservation) -> bool {
    same_entry_metadata(&left.entry, &right.entry) && left.bytes == right.bytes
}
