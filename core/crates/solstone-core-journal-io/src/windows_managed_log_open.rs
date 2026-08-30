// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retained-handle opens for Windows managed-log alias and payload files.

#![allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_APPEND_DATA, FILE_READ_ATTRIBUTES, FILE_READ_DATA, SYNCHRONIZE,
};

use crate::errors::FlatDirectoryError;
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::nt_create_relative;
use crate::windows_sync_dir::{WindowsFlatDirectory, validate_windows_regular_handle};

/// A regular file opened from a retained directory and bound to its full identity.
pub(crate) struct OpenedManagedLogFile {
    file: File,
    identity: WindowsFileIdentity,
}

impl OpenedManagedLogFile {
    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(crate) fn into_file(self) -> File {
        self.file
    }

    pub(crate) const fn identity(&self) -> WindowsFileIdentity {
        self.identity
    }
}

/// Boundedly read one retained alias record, retaining the record file identity.
pub(crate) fn read_pointer_record_bounded(
    directory: &WindowsFlatDirectory,
    alias_name: &OsStr,
    limit: usize,
) -> Result<(WindowsFileIdentity, Vec<u8>), ManagedLogOpenError> {
    let mut opened = open_existing(directory, alias_name, GENERIC_READ | FILE_READ_ATTRIBUTES)?;
    let before = opened.file.metadata().map_err(|source| {
        ManagedLogOpenError::io("stat pointer record", directory, alias_name, source)
    })?;
    if before.len() > limit as u64 {
        return Err(ManagedLogOpenError::SizeLimitExceeded {
            name: alias_name.to_os_string(),
            size: before.len(),
            limit,
        });
    }
    let length = usize::try_from(before.len()).map_err(|_| {
        ManagedLogOpenError::io(
            "size pointer-record buffer",
            directory,
            alias_name,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "pointer record exceeds address space",
            ),
        )
    })?;
    let mut bytes = vec![0; length];
    read_exact(&mut opened.file, &mut bytes).map_err(|source| {
        ManagedLogOpenError::io("read pointer record", directory, alias_name, source)
    })?;
    let mut extra = [0; 1];
    if opened.file.read(&mut extra).map_err(|source| {
        ManagedLogOpenError::io("check pointer record length", directory, alias_name, source)
    })? != 0
    {
        return Err(ManagedLogOpenError::Changed {
            name: alias_name.to_os_string(),
        });
    }
    let after = opened.file.metadata().map_err(|source| {
        ManagedLogOpenError::io("restat pointer record", directory, alias_name, source)
    })?;
    if before.len() != after.len()
        || file_identity(opened.file.as_raw_handle()).map_err(|source| {
            ManagedLogOpenError::io("reidentify pointer record", directory, alias_name, source)
        })? != opened.identity
    {
        return Err(ManagedLogOpenError::Changed {
            name: alias_name.to_os_string(),
        });
    }
    directory.revalidate_bound()?;
    Ok((opened.identity, bytes))
}

/// Open one existing canonical payload for retained reading and full identity verification.
pub(crate) fn open_canonical_for_read(
    directory: &WindowsFlatDirectory,
    canonical_name: &OsStr,
    expected: WindowsFileIdentity,
) -> Result<OpenedManagedLogFile, ManagedLogOpenError> {
    let opened = open_existing(
        directory,
        canonical_name,
        GENERIC_READ | FILE_READ_ATTRIBUTES | FILE_READ_DATA,
    )?;
    verify_expected(opened, directory, canonical_name, expected)
}

/// Open one existing canonical payload for append; this never creates on miss.
pub(crate) fn open_canonical_for_append(
    directory: &WindowsFlatDirectory,
    canonical_name: &OsStr,
    expected: WindowsFileIdentity,
) -> Result<OpenedManagedLogFile, ManagedLogOpenError> {
    let opened = open_existing(
        directory,
        canonical_name,
        FILE_APPEND_DATA | FILE_READ_ATTRIBUTES,
    )?;
    verify_expected(opened, directory, canonical_name, expected)
}

/// Create a new canonical append file. Existing names are refused explicitly.
pub(crate) fn create_canonical_for_append(
    directory: &WindowsFlatDirectory,
    canonical_name: &OsStr,
) -> Result<OpenedManagedLogFile, ManagedLogOpenError> {
    open_relative(
        directory,
        canonical_name,
        FILE_APPEND_DATA | FILE_READ_ATTRIBUTES,
        FILE_CREATE,
    )
}

fn verify_expected(
    opened: OpenedManagedLogFile,
    _directory: &WindowsFlatDirectory,
    name: &OsStr,
    expected: WindowsFileIdentity,
) -> Result<OpenedManagedLogFile, ManagedLogOpenError> {
    if opened.identity != expected {
        return Err(ManagedLogOpenError::IdentityMismatch {
            name: name.to_os_string(),
        });
    }
    Ok(opened)
}

fn open_existing(
    directory: &WindowsFlatDirectory,
    name: &OsStr,
    desired_access: u32,
) -> Result<OpenedManagedLogFile, ManagedLogOpenError> {
    open_relative(directory, name, desired_access, FILE_OPEN)
}

fn open_relative(
    directory: &WindowsFlatDirectory,
    name: &OsStr,
    desired_access: u32,
    disposition: u32,
) -> Result<OpenedManagedLogFile, ManagedLogOpenError> {
    directory.revalidate_bound()?;
    let path = directory.diagnostic_entry_path(name);
    let handle = nt_create_relative(
        directory.as_handle().as_raw_handle(),
        name,
        desired_access | SYNCHRONIZE,
        disposition,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
    .map_err(|source| ManagedLogOpenError::io("open managed-log entry", directory, name, source))?;
    let file = File::from(handle);
    let identity = validate_windows_regular_handle(file.as_raw_handle(), &path)?;
    directory.revalidate_bound()?;
    if file_identity(file.as_raw_handle()).map_err(|source| {
        ManagedLogOpenError::io("reidentify managed-log entry", directory, name, source)
    })? != identity
    {
        return Err(ManagedLogOpenError::Changed {
            name: name.to_os_string(),
        });
    }
    Ok(OpenedManagedLogFile { file, identity })
}

fn read_exact(reader: &mut impl Read, bytes: &mut [u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        match reader.read(&mut bytes[offset..]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// The retained-open boundary never turns a substitution into a path reopen.
#[derive(Debug)]
pub(crate) enum ManagedLogOpenError {
    FlatDirectory(FlatDirectoryError),
    Io {
        operation: &'static str,
        name: std::ffi::OsString,
        source: io::Error,
    },
    SizeLimitExceeded {
        name: std::ffi::OsString,
        size: u64,
        limit: usize,
    },
    Changed {
        name: std::ffi::OsString,
    },
    IdentityMismatch {
        name: std::ffi::OsString,
    },
}

impl ManagedLogOpenError {
    fn io(
        operation: &'static str,
        _directory: &WindowsFlatDirectory,
        name: &OsStr,
        source: io::Error,
    ) -> Self {
        Self::Io {
            operation,
            name: name.to_os_string(),
            source,
        }
    }
}

impl From<FlatDirectoryError> for ManagedLogOpenError {
    fn from(error: FlatDirectoryError) -> Self {
        Self::FlatDirectory(error)
    }
}

impl fmt::Display for ManagedLogOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatDirectory(error) => error.fmt(formatter),
            Self::Io {
                operation,
                name,
                source,
            } => write!(formatter, "{operation} failed for {name:?}: {source}"),
            Self::SizeLimitExceeded { name, size, limit } => write!(
                formatter,
                "managed-log entry {name:?} is {size} bytes, exceeding {limit}"
            ),
            Self::Changed { name } => write!(
                formatter,
                "managed-log entry changed while opening: {name:?}"
            ),
            Self::IdentityMismatch { name } => write!(
                formatter,
                "managed-log entry identity does not match: {name:?}"
            ),
        }
    }
}

impl Error for ManagedLogOpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::FlatDirectory(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn _assert_seek(_: &impl Seek) {}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::io::{AsHandle, AsRawHandle};

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_TRAVERSE, OPEN_EXISTING,
    };

    use super::*;
    use crate::locking::open_windows_path;
    use crate::test_support::TempDir;

    fn root_handle(path: &std::path::Path) -> File {
        open_windows_path(
            path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .unwrap()
    }

    fn bound_child(
        parent: &impl AsHandle,
        parent_path: &std::path::Path,
        name: &str,
    ) -> WindowsFlatDirectory {
        crate::windows_sync_dir::create_or_open_windows_flat_directory_bound(
            parent,
            OsStr::new(name),
            parent_path,
        )
        .unwrap()
    }

    #[test]
    fn retained_canonical_handles_survive_name_replacement_and_reject_identity_substitution() {
        let temporary = TempDir::new();
        let root = root_handle(temporary.path());
        let health = bound_child(&root, temporary.path(), "health");
        let name = OsStr::new("!solstone-ml-p-test.log");
        let path = temporary.path().join("health").join(name);

        let mut created = create_canonical_for_append(&health, name).unwrap();
        created
            .file_mut()
            .write_all(b"original labelled bytes")
            .unwrap();
        created.file_mut().sync_all().unwrap();
        let identity = created.identity();
        drop(created);

        let mut reader = open_canonical_for_read(&health, name, identity).unwrap();
        assert!(
            open_canonical_for_read(
                &health,
                name,
                WindowsFileIdentity::from_parts(identity.volume_serial(), [0; 16]),
            )
            .is_err()
        );
        fs::rename(&path, temporary.path().join("retired.log")).unwrap();
        fs::write(&path, b"replacement bytes").unwrap();
        reader.file_mut().seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        reader.file_mut().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original labelled bytes");
        assert_ne!(
            file_identity(fs::File::open(&path).unwrap().as_raw_handle()).unwrap(),
            identity
        );
    }

    #[test]
    fn append_open_does_not_create_missing_or_accept_directories() {
        let temporary = TempDir::new();
        let root = root_handle(temporary.path());
        let health = bound_child(&root, temporary.path(), "health");
        let missing = OsStr::new("!solstone-ml-p-missing.log");
        assert!(
            open_canonical_for_append(
                &health,
                missing,
                WindowsFileIdentity::from_parts(1, [1; 16]),
            )
            .is_err()
        );
        fs::create_dir(
            temporary
                .path()
                .join("health")
                .join("!solstone-ml-p-dir.log"),
        )
        .unwrap();
        assert!(
            create_canonical_for_append(&health, OsStr::new("!solstone-ml-p-dir.log")).is_err()
        );
    }
}
