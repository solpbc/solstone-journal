// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded no-follow access to one direct Windows directory.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_IF,
    FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, FileIdExtdDirectoryInformation,
    NtQueryDirectoryFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, GENERIC_READ,
    GENERIC_WRITE, RtlNtStatusToDosError, STATUS_SUCCESS,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO, FILE_ID_EXTD_DIR_INFO, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_TRAVERSE, FILE_TYPE_DISK, FileAttributeTagInfo,
    FileDispositionInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, GetFileType,
    SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX,
    FileDispositionInfoEx,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use crate::errors::FlatDirectoryError;
use crate::journal_root::JournalEntryKind;
use crate::name_admission::{NameAdmissionReason, check_portable_component};
use crate::observation::{FileObservation, FlatDirectoryEntry, NativeMtime, same_entry_metadata};
use crate::windows_identity::{WindowsFileIdentity, file_identity, file_link_count};
use crate::windows_ntcreate::{
    nt_create_relative, nt_create_relative_exact, nt_create_relative_share_read,
};

const DIRECTORY_BUFFER_BYTES: usize = 64 * 1024;
const WINDOWS_TO_UNIX_EPOCH_100NS: i128 = 116_444_736_000_000_000;
const HUNDRED_NANOSECONDS_PER_SECOND: i128 = 10_000_000;

/// An acquired Windows directory for direct-entry operations only.
pub struct WindowsFlatDirectory {
    directory: OwnedHandle,
    identity: WindowsFileIdentity,
    diagnostic_path: PathBuf,
}

impl WindowsFlatDirectory {
    fn revalidate(&self) -> Result<(), FlatDirectoryError> {
        let attributes = attribute_tag(self.directory.as_raw_handle(), &self.diagnostic_path)?;
        if !is_directory(attributes) {
            return Err(FlatDirectoryError::NotDirectory {
                path: self.diagnostic_path.clone(),
            });
        }
        if is_reparse_point(attributes) {
            return Err(FlatDirectoryError::SymlinkRefused {
                path: self.diagnostic_path.clone(),
            });
        }
        if entry_identity(self.directory.as_raw_handle(), &self.diagnostic_path)? != self.identity {
            return Err(FlatDirectoryError::IdentityChanged {
                path: self.diagnostic_path.clone(),
            });
        }
        Ok(())
    }

    fn diagnostic_entry(&self, name: &OsStr) -> PathBuf {
        self.diagnostic_path.join(name)
    }

    /// Revalidate the retained directory before a descriptor-relative operation.
    pub fn revalidate_bound(&self) -> Result<(), FlatDirectoryError> {
        self.revalidate()
    }

    /// Full identity captured when this directory was bound.
    pub const fn identity(&self) -> WindowsFileIdentity {
        self.identity
    }

    /// Compare this retained directory with the flat listing's identity representation.
    pub fn matches_entry(&self, entry: &FlatDirectoryEntry) -> bool {
        entry.kind == JournalEntryKind::Directory
            && self.identity.volume_serial() == entry.device
            && self.identity.folded_file_id() == entry.inode
    }

    /// The bound directory's identity, comparable with a media directory's.
    pub fn object_identity(&self) -> crate::journal_root::ObjectIdentity {
        crate::journal_root::ObjectIdentity::from_volume_and_file_id(
            self.identity.volume_serial(),
            self.identity.file_id(),
        )
    }

    /// Retained directory handle used as the authority for relative operations.
    #[allow(
        dead_code,
        reason = "retained for generic capability-bound file operations"
    )]
    pub(crate) fn handle(&self) -> &OwnedHandle {
        &self.directory
    }

    /// Diagnostic path only; it is never authority for a child operation.
    #[allow(
        dead_code,
        reason = "retained for generic capability-bound file operations"
    )]
    pub(crate) fn diagnostic_path(&self) -> &Path {
        &self.diagnostic_path
    }

    /// Diagnostic path for one direct child.
    #[allow(
        dead_code,
        reason = "retained for generic capability-bound file operations"
    )]
    pub(crate) fn diagnostic_entry_path(&self, name: &OsStr) -> PathBuf {
        self.diagnostic_entry(name)
    }
}

/// Validate an already-opened child as a direct regular non-reparse file.
#[allow(
    dead_code,
    reason = "retained for generic capability-bound file operations"
)]
pub(crate) fn validate_windows_regular_handle(
    handle: std::os::windows::io::RawHandle,
    path: &Path,
) -> Result<WindowsFileIdentity, FlatDirectoryError> {
    let attributes = attribute_tag(handle, path)?;
    if is_reparse_point(attributes) {
        return Err(FlatDirectoryError::SymlinkRefused {
            path: path.to_path_buf(),
        });
    }
    if is_directory(attributes) {
        return Err(FlatDirectoryError::NotRegular {
            path: path.to_path_buf(),
        });
    }
    let file_type = {
        // SAFETY: `handle` remains valid for the synchronous type query.
        #[allow(unsafe_code)]
        unsafe {
            GetFileType(handle)
        }
    };
    if file_type != FILE_TYPE_DISK {
        return Err(FlatDirectoryError::NotRegular {
            path: path.to_path_buf(),
        });
    }
    entry_identity(handle, path)
}

impl AsHandle for WindowsFlatDirectory {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.directory.as_handle()
    }
}

/// Create or accept one direct portable child directory beneath a bound parent.
pub fn create_or_open_windows_flat_directory_bound(
    parent: &impl AsHandle,
    name: &OsStr,
    parent_diagnostic: &Path,
) -> Result<WindowsFlatDirectory, FlatDirectoryError> {
    open_windows_flat_directory(parent, name, parent_diagnostic, FILE_OPEN_IF)?.ok_or_else(|| {
        FlatDirectoryError::EnumerationChanged {
            path: parent_diagnostic.join(name),
        }
    })
}

/// Open one direct portable child directory beneath a bound parent without creating it.
pub fn open_windows_flat_directory_bound(
    parent: &impl AsHandle,
    name: &OsStr,
    parent_diagnostic: &Path,
) -> Result<Option<WindowsFlatDirectory>, FlatDirectoryError> {
    open_windows_flat_directory(parent, name, parent_diagnostic, FILE_OPEN)
}

/// Open one direct regular non-reparse child beneath an already-bound parent.
///
/// The caller owns the lifetime and revalidation of `parent`; this function
/// obtains the child through that retained handle, never through its diagnostic
/// pathname. A missing child is `Ok(None)`.
pub fn open_windows_regular_file_from_bound_parent(
    parent: &impl AsHandle,
    name: &OsStr,
    parent_diagnostic: &Path,
) -> Result<Option<File>, FlatDirectoryError> {
    let name = validate_portable_name(name)?;
    let path = parent_diagnostic.join(&name);
    let handle = match open_relative(
        parent.as_handle().as_raw_handle(),
        &name,
        FILE_READ_ATTRIBUTES | FILE_READ_DATA,
        0,
        &path,
        FILE_OPEN,
    ) {
        Ok(handle) => handle,
        Err(FlatDirectoryError::Io { source, .. })
            if matches!(
                source.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    validate_windows_regular_handle(handle.as_raw_handle(), &path)?;
    Ok(Some(File::from(handle)))
}

/// Create or open one direct regular file for a held append and file flush.
/// Competing writes and replacement are excluded while the handle is retained.
pub fn create_or_open_windows_append_file_bound(
    parent: &impl AsHandle,
    name: &OsStr,
    parent_diagnostic: &Path,
) -> Result<File, FlatDirectoryError> {
    let name = validate_portable_name(name)?;
    let path = parent_diagnostic.join(&name);
    let failure = |source| FlatDirectoryError::Io {
        operation: "open direct writable file",
        path: path.clone(),
        source,
    };
    let handle = nt_create_relative_share_read(
        parent.as_handle().as_raw_handle(),
        &name,
        GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE,
        FILE_OPEN_IF,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT,
    )
    .map_err(failure)?;
    validate_windows_regular_handle(handle.as_raw_handle(), &path)?;
    if file_link_count(handle.as_raw_handle()).map_err(failure)? != 1 {
        return Err(failure(io::Error::new(
            io::ErrorKind::InvalidData,
            "writable file has multiple links",
        )));
    }
    Ok(File::from(handle))
}

/// List direct entries, returning `None` instead of a partial list above `maximum`.
pub fn list_windows_flat_directory(
    directory: &WindowsFlatDirectory,
    maximum: usize,
) -> Result<Option<Vec<FlatDirectoryEntry>>, FlatDirectoryError> {
    directory.revalidate()?;
    let listed = list_directory(
        directory.directory.as_raw_handle(),
        &directory.diagnostic_path,
        false,
    )?;
    let mut entries = Vec::new();
    for listed in listed {
        if entries.len() == maximum {
            return Ok(None);
        }
        let name = validate_portable_name(&listed.name)?;
        let path = directory.diagnostic_entry(&name);
        let handle = open_relative(
            directory.directory.as_raw_handle(),
            &name,
            FILE_READ_ATTRIBUTES,
            0,
            &path,
            FILE_OPEN,
        )?;
        let entry = entry_from_handle(name, handle.as_raw_handle(), &path)?;
        if entry_identity(handle.as_raw_handle(), &path)?.file_id() != listed.file_id {
            return Err(FlatDirectoryError::EnumerationChanged { path });
        }
        entries.push(entry);
    }
    directory.revalidate()?;
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Some(entries))
}

pub(crate) struct WindowsNativeListed {
    pub entry: FlatDirectoryEntry,
    pub directory: Option<WindowsFlatDirectory>,
}

pub(crate) fn list_windows_native_entries(
    directory: &WindowsFlatDirectory,
    maximum: usize,
    retain_directories: bool,
    after_enumeration: crate::cortex_use::census::CortexCensusPrimitive,
    before_directory_open: Option<crate::cortex_use::census::CortexCensusPrimitive>,
    enumeration_stage: crate::cortex_use::census::CortexCensusStage,
) -> Result<Option<Vec<WindowsNativeListed>>, crate::cortex_use::census::CortexCensusError> {
    use crate::cortex_use::census::{CortexCensusStage, checkpoint, map_listing};
    directory
        .revalidate()
        .map_err(|error| map_listing(enumeration_stage, error))?;
    let mut listed = list_directory(
        directory.directory.as_raw_handle(),
        &directory.diagnostic_path,
        true,
    )
    .map_err(|error| map_listing(enumeration_stage, error))?;
    if listed.len() > maximum {
        return Ok(None);
    }
    checkpoint(after_enumeration)?;
    listed.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    let mut entries = Vec::new();
    for listed in listed {
        let path = directory.diagnostic_entry(&listed.name);
        let handle = match open_relative_exact(
            directory.directory.as_raw_handle(),
            &listed.name,
            FILE_READ_ATTRIBUTES,
            0,
            &path,
            FILE_OPEN,
        ) {
            Ok(handle) => handle,
            Err(FlatDirectoryError::Io { source, .. })
                if matches!(
                    source.raw_os_error(),
                    Some(code)
                        if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
                ) =>
            {
                return Err(map_listing(
                    enumeration_stage,
                    FlatDirectoryError::EnumerationChanged { path },
                ));
            }
            Err(error) => return Err(map_listing(enumeration_stage, error)),
        };
        let entry = entry_from_handle(listed.name.clone(), handle.as_raw_handle(), &path)
            .map_err(|error| map_listing(enumeration_stage, error))?;
        let identity = entry_identity(handle.as_raw_handle(), &path)
            .map_err(|error| map_listing(enumeration_stage, error))?;
        if identity.file_id() != listed.file_id {
            return Err(map_listing(
                enumeration_stage,
                FlatDirectoryError::EnumerationChanged { path },
            ));
        }
        let child = if retain_directories && entry.kind == JournalEntryKind::Directory {
            if let Some(primitive) = before_directory_open {
                checkpoint(primitive)?;
            }
            let retained = open_relative_exact(
                directory.directory.as_raw_handle(),
                &listed.name,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE,
                FILE_DIRECTORY_FILE | FILE_OPEN_FOR_BACKUP_INTENT,
                &path,
                FILE_OPEN,
            )
            .map_err(|error| map_listing(CortexCensusStage::TalentOpen, error))?;
            let retained_identity = entry_identity(retained.as_raw_handle(), &path)
                .map_err(|error| map_listing(CortexCensusStage::TalentOpen, error))?;
            if retained_identity != identity {
                return Err(map_listing(
                    CortexCensusStage::TalentOpen,
                    FlatDirectoryError::IdentityChanged { path },
                ));
            }
            Some(WindowsFlatDirectory {
                directory: retained,
                identity: retained_identity,
                diagnostic_path: path,
            })
        } else {
            None
        };
        entries.push(WindowsNativeListed {
            entry,
            directory: child,
        });
    }
    Ok(Some(entries))
}

pub(crate) fn native_child_directory_identity(
    parent: &WindowsFlatDirectory,
    name: &OsStr,
) -> Result<Option<WindowsFileIdentity>, FlatDirectoryError> {
    let path = parent.diagnostic_entry(name);
    let handle = match open_relative_exact(
        parent.directory.as_raw_handle(),
        name,
        FILE_READ_ATTRIBUTES,
        0,
        &path,
        FILE_OPEN,
    ) {
        Ok(handle) => handle,
        Err(FlatDirectoryError::Io { source, .. })
            if matches!(
                source.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let attributes = attribute_tag(handle.as_raw_handle(), &path)?;
    if !is_directory(attributes) || is_reparse_point(attributes) {
        return Ok(None);
    }
    Ok(Some(entry_identity(handle.as_raw_handle(), &path)?))
}

/// Delete one direct regular file only while it is the exact observed object
/// holding the observed bytes. The handle shares reading only, so nothing can
/// write or rename it between verification and the delete, and the delete goes
/// through that verified handle with POSIX semantics where the filesystem
/// allows, so the name is gone when this returns even if another reader still
/// has it open. Windows has no directory flush, so success is
/// `RemovedUnflushed`; the caller confirms the name is gone. Any refusal leaves
/// the entry untouched.
pub fn remove_windows_observed_file_bounded(
    directory: &WindowsFlatDirectory,
    name: &OsStr,
    observed: &FileObservation,
    limit: usize,
) -> Result<crate::errors::ClaimRemovalOutcome, crate::errors::ClaimRemovalError> {
    use crate::errors::{ClaimRemovalError, ClaimRemovalOutcome};
    let preflight = |source| ClaimRemovalError::Preflight { source };
    let name = validate_portable_name(name).map_err(preflight)?;
    if observed.entry.name != name {
        return Err(ClaimRemovalError::ObservationNameMismatch {
            original: name,
            observed: observed.entry.name.clone(),
        });
    }
    directory.revalidate().map_err(preflight)?;
    let path = directory.diagnostic_entry(&name);
    let changed = || preflight(FlatDirectoryError::IdentityChanged { path: path.clone() });
    let handle = nt_create_relative_share_read(
        directory.directory.as_raw_handle(),
        &name,
        DELETE | FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
    .map_err(|source| {
        preflight(FlatDirectoryError::Io {
            operation: "open observed file for deletion",
            path: path.clone(),
            source,
        })
    })?;
    let before =
        entry_from_handle(name.clone(), handle.as_raw_handle(), &path).map_err(preflight)?;
    if before.kind != JournalEntryKind::RegularFile
        || !same_entry_metadata(&before, &observed.entry)
        || (before.size as u128) > (limit as u128)
    {
        return Err(changed());
    }
    let mut file = File::from(handle);
    let mut bytes = vec![0; observed.bytes.len()];
    read_exact(&mut file, &mut bytes).map_err(|_| changed())?;
    let mut trailing = [0u8; 1];
    if bytes != observed.bytes || !matches!(file.read(&mut trailing), Ok(0)) {
        return Err(changed());
    }
    let after = entry_from_handle(name, file.as_raw_handle(), &path).map_err(preflight)?;
    if !same_entry_metadata(&after, &observed.entry) {
        return Err(changed());
    }
    directory.revalidate().map_err(preflight)?;
    let posix = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    // SAFETY: the retained handle was opened with DELETE access and each input
    // is one initialized, correctly sized disposition structure.
    #[allow(unsafe_code)]
    let mut set = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            (&posix as *const FILE_DISPOSITION_INFO_EX).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if set == 0 {
        // Filesystems without POSIX semantics take the legacy delete-on-close.
        let legacy = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: as above, with the legacy disposition structure.
        #[allow(unsafe_code)]
        let retried = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&legacy as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        set = retried;
    }
    if set == 0 {
        return Err(ClaimRemovalError::Io {
            operation: "delete observed file",
            path,
            source: io::Error::last_os_error(),
        });
    }
    drop(file); // The delete lands when this last handle closes.
    Ok(ClaimRemovalOutcome::RemovedUnflushed)
}

/// Read one direct regular file while proving stable metadata and enforcing a byte limit.
pub fn read_windows_observed_file_bounded(
    directory: &WindowsFlatDirectory,
    name: &OsStr,
    limit: usize,
) -> Result<Option<FileObservation>, FlatDirectoryError> {
    let name = validate_portable_name(name)?;
    directory.revalidate()?;
    let path = directory.diagnostic_entry(&name);
    let Some(handle) = open_regular_relative(directory, &name, &path)? else {
        return Ok(None);
    };
    let entry = entry_from_handle(name.clone(), handle.as_raw_handle(), &path)?;
    if entry.kind != JournalEntryKind::RegularFile {
        return Err(FlatDirectoryError::NotRegular { path });
    }
    #[cfg(feature = "test-hooks")]
    crate::read_test_hook::opened(&path);
    if (entry.size as u128) > (limit as u128) {
        return Err(FlatDirectoryError::SizeLimitExceeded {
            path,
            kind: entry.kind,
            size: entry.size,
            limit,
        });
    }
    let size = usize::try_from(entry.size).map_err(|_| FlatDirectoryError::Io {
        operation: "size observed file buffer",
        path: directory.diagnostic_entry(&name),
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            "observed file exceeds address space",
        ),
    })?;
    let mut file = File::from(handle);
    let mut bytes = vec![0; size];
    if let Err(source) = read_exact(&mut file, &mut bytes) {
        if source.kind() == io::ErrorKind::UnexpectedEof {
            return Err(FlatDirectoryError::IdentityChanged {
                path: directory.diagnostic_entry(&name),
            });
        }
        return Err(FlatDirectoryError::Io {
            operation: "read observed file",
            path: directory.diagnostic_entry(&name),
            source,
        });
    }
    let after = entry_from_handle(
        name,
        file.as_raw_handle(),
        &directory.diagnostic_entry(&entry.name),
    )?;
    if !same_entry_metadata(&entry, &after) {
        return Err(FlatDirectoryError::IdentityChanged {
            path: directory.diagnostic_entry(&entry.name),
        });
    }
    directory.revalidate()?;
    Ok(Some(FileObservation { entry, bytes }))
}

fn open_windows_flat_directory(
    parent: &impl AsHandle,
    name: &OsStr,
    parent_diagnostic: &Path,
    disposition: u32,
) -> Result<Option<WindowsFlatDirectory>, FlatDirectoryError> {
    let name = validate_portable_name(name)?;
    let diagnostic_path = parent_diagnostic.join(&name);
    let handle = match open_relative(
        parent.as_handle().as_raw_handle(),
        &name,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE,
        FILE_DIRECTORY_FILE | FILE_OPEN_FOR_BACKUP_INTENT,
        &diagnostic_path,
        disposition,
    ) {
        Ok(handle) => handle,
        Err(FlatDirectoryError::Io { source, .. }) if matches!(source.raw_os_error(), Some(code) if code == ERROR_DIRECTORY as i32) =>
        {
            return Err(FlatDirectoryError::NotDirectory {
                path: diagnostic_path,
            });
        }
        Err(FlatDirectoryError::Io { source, .. })
            if disposition == FILE_OPEN
                && matches!(
                    source.raw_os_error(),
                    Some(code)
                        if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
                ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let attributes = attribute_tag(handle.as_raw_handle(), &diagnostic_path)?;
    if !is_directory(attributes) {
        return Err(FlatDirectoryError::NotDirectory {
            path: diagnostic_path,
        });
    }
    if is_reparse_point(attributes) {
        return Err(FlatDirectoryError::SymlinkRefused {
            path: diagnostic_path,
        });
    }
    let identity = entry_identity(handle.as_raw_handle(), &diagnostic_path)?;
    Ok(Some(WindowsFlatDirectory {
        directory: handle,
        identity,
        diagnostic_path,
    }))
}

fn open_regular_relative(
    directory: &WindowsFlatDirectory,
    name: &OsStr,
    path: &Path,
) -> Result<Option<OwnedHandle>, FlatDirectoryError> {
    match open_relative(
        directory.directory.as_raw_handle(),
        name,
        FILE_READ_ATTRIBUTES | FILE_READ_DATA,
        0,
        path,
        FILE_OPEN,
    ) {
        Ok(handle) => Ok(Some(handle)),
        Err(FlatDirectoryError::Io { source, .. })
            if matches!(
                source.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn open_relative(
    parent: std::os::windows::io::RawHandle,
    name: &OsStr,
    desired_access: u32,
    extra_options: u32,
    path: &Path,
    disposition: u32,
) -> Result<OwnedHandle, FlatDirectoryError> {
    nt_create_relative(
        parent,
        name,
        desired_access | SYNCHRONIZE,
        disposition,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | extra_options,
    )
    .map_err(|source| FlatDirectoryError::Io {
        operation: "open flat-directory entry",
        path: path.to_path_buf(),
        source,
    })
}

fn open_relative_exact(
    parent: std::os::windows::io::RawHandle,
    name: &OsStr,
    desired_access: u32,
    extra_options: u32,
    path: &Path,
    disposition: u32,
) -> Result<OwnedHandle, FlatDirectoryError> {
    nt_create_relative_exact(
        parent,
        name,
        desired_access | SYNCHRONIZE,
        disposition,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | extra_options,
    )
    .map_err(|source| FlatDirectoryError::Io {
        operation: "open flat-directory entry",
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) struct ListedEntry {
    pub name: OsString,
    pub file_id: [u8; 16],
}

/// Caller supplies a private enumeration file object, never a shared cursor.
pub(crate) fn list_directory_bounded(
    directory: std::os::windows::io::RawHandle,
    path: &Path,
    maximum: usize,
) -> Result<Option<Vec<ListedEntry>>, FlatDirectoryError> {
    let mut buffer = vec![0u8; DIRECTORY_BUFFER_BYTES];
    let mut entries = Vec::new();
    let mut restart = true;
    loop {
        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: private retained synchronous directory and exact live output
        // buffer; returned byte count is validated before decoding.
        #[allow(unsafe_code)]
        let result = unsafe {
            NtQueryDirectoryFile(
                directory,
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                &mut status,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                FileIdExtdDirectoryInformation,
                false,
                std::ptr::null(),
                restart,
            )
        };
        if result == STATUS_SUCCESS {
            restart = false;
            let used = status.Information;
            if used == 0 || used > buffer.len() {
                return Err(invalid_listing(path));
            }
            if !parse_directory_buffer_bounded(&buffer[..used], path, &mut entries, true, maximum)?
            {
                return Ok(None);
            }
            continue;
        }
        // SAFETY: translates only the status returned by the preceding call.
        #[allow(unsafe_code)]
        let error = unsafe { RtlNtStatusToDosError(result) };
        if error == ERROR_NO_MORE_FILES {
            return Ok(Some(entries));
        }
        return Err(FlatDirectoryError::Io {
            operation: "list bounded native directory",
            path: path.to_path_buf(),
            source: io::Error::from_raw_os_error(error as i32),
        });
    }
}

fn list_directory(
    directory: std::os::windows::io::RawHandle,
    path: &Path,
    native: bool,
) -> Result<Vec<ListedEntry>, FlatDirectoryError> {
    let mut buffer = vec![0u8; DIRECTORY_BUFFER_BYTES];
    let mut entries = Vec::new();
    let mut restart_scan = true;
    loop {
        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: `directory` is a retained synchronous directory handle and `buffer` is
        // writable for the exact supplied size through this synchronous query.
        #[allow(unsafe_code)]
        let result = unsafe {
            NtQueryDirectoryFile(
                directory,
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                &mut status,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                FileIdExtdDirectoryInformation,
                false,
                std::ptr::null(),
                restart_scan,
            )
        };
        if result == STATUS_SUCCESS {
            restart_scan = false;
            parse_directory_buffer(&buffer, path, &mut entries, native)?;
            continue;
        }
        // SAFETY: converts only the NTSTATUS returned by the preceding call.
        #[allow(unsafe_code)]
        let error = unsafe { RtlNtStatusToDosError(result) };
        if error == ERROR_NO_MORE_FILES {
            break;
        }
        return Err(FlatDirectoryError::Io {
            operation: "list flat directory",
            path: path.to_path_buf(),
            source: io::Error::from_raw_os_error(error as i32),
        });
    }
    Ok(entries)
}

fn parse_directory_buffer(
    buffer: &[u8],
    path: &Path,
    entries: &mut Vec<ListedEntry>,
    native: bool,
) -> Result<(), FlatDirectoryError> {
    parse_directory_buffer_bounded(buffer, path, entries, native, usize::MAX).map(|_| ())
}

fn parse_directory_buffer_bounded(
    buffer: &[u8],
    path: &Path,
    entries: &mut Vec<ListedEntry>,
    native: bool,
    maximum: usize,
) -> Result<bool, FlatDirectoryError> {
    let header_bytes = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
    let mut offset = 0usize;
    loop {
        let remaining = buffer.get(offset..).ok_or_else(|| invalid_listing(path))?;
        let header = remaining
            .get(..header_bytes)
            .ok_or_else(|| invalid_listing(path))?;
        let name_bytes = usize::try_from(directory_u32(
            header,
            offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength),
            path,
        )?)
        .map_err(|_| invalid_listing(path))?;
        if name_bytes % size_of::<u16>() != 0 {
            return Err(invalid_listing(path));
        }
        let record_bytes = header_bytes
            .checked_add(name_bytes)
            .ok_or_else(|| invalid_listing(path))?;
        let record = remaining
            .get(..record_bytes)
            .ok_or_else(|| invalid_listing(path))?;
        let units = record[header_bytes..]
            .chunks_exact(size_of::<u16>())
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let name = if native {
            OsString::from_wide(&units)
        } else {
            OsString::from(String::from_utf16(&units).map_err(|_| invalid_listing(path))?)
        };
        let file_id_start = offset_of!(FILE_ID_EXTD_DIR_INFO, FileId);
        let file_id_end = file_id_start
            .checked_add(16)
            .ok_or_else(|| invalid_listing(path))?;
        let file_id = record
            .get(file_id_start..file_id_end)
            .ok_or_else(|| invalid_listing(path))?
            .try_into()
            .map_err(|_| invalid_listing(path))?;
        if name != "." && name != ".." {
            if entries.len() == maximum {
                return Ok(false);
            }
            entries.push(ListedEntry { name, file_id });
        }
        let next = directory_u32(
            header,
            offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset),
            path,
        )?;
        if next == 0 {
            return Ok(true);
        }
        let next = usize::try_from(next).map_err(|_| invalid_listing(path))?;
        if next < record_bytes || next > remaining.len() {
            return Err(invalid_listing(path));
        }
        offset = offset
            .checked_add(next)
            .ok_or_else(|| invalid_listing(path))?;
    }
}

fn directory_u32(
    header: &[u8],
    field_offset: usize,
    path: &Path,
) -> Result<u32, FlatDirectoryError> {
    let end = field_offset
        .checked_add(size_of::<u32>())
        .ok_or_else(|| invalid_listing(path))?;
    let value: [u8; 4] = header
        .get(field_offset..end)
        .ok_or_else(|| invalid_listing(path))?
        .try_into()
        .map_err(|_| invalid_listing(path))?;
    Ok(u32::from_le_bytes(value))
}

fn invalid_listing(path: &Path) -> FlatDirectoryError {
    FlatDirectoryError::Io {
        operation: "parse flat-directory listing",
        path: path.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed FileIdExtdDirectoryInfo buffer",
        ),
    }
}

fn validate_portable_name(name: &OsStr) -> Result<OsString, FlatDirectoryError> {
    let text = name
        .to_str()
        .ok_or_else(|| FlatDirectoryError::InvalidName {
            name: name.to_os_string(),
            reason: NameAdmissionReason::NotUtf8,
        })?;
    check_portable_component(text).map_err(|reason| FlatDirectoryError::InvalidName {
        name: name.to_os_string(),
        reason,
    })?;
    Ok(name.to_os_string())
}

fn entry_from_handle(
    name: OsString,
    handle: std::os::windows::io::RawHandle,
    path: &Path,
) -> Result<FlatDirectoryEntry, FlatDirectoryError> {
    let attributes = attribute_tag(handle, path)?;
    let kind = if is_reparse_point(attributes) {
        JournalEntryKind::Other
    } else if is_directory(attributes) {
        JournalEntryKind::Directory
    } else {
        let file_type = {
            // SAFETY: `handle` remains valid for the metadata query.
            #[allow(unsafe_code)]
            unsafe {
                GetFileType(handle)
            }
        };
        if file_type != FILE_TYPE_DISK {
            return Err(FlatDirectoryError::NotRegular {
                path: path.to_path_buf(),
            });
        }
        JournalEntryKind::RegularFile
    };
    let identity = entry_identity(handle, path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `info` is writable for its exact size and `handle` remains valid for the call.
    #[allow(unsafe_code)]
    let result = unsafe { GetFileInformationByHandle(handle, &mut info) };
    if result == 0 {
        return Err(FlatDirectoryError::Io {
            operation: "query flat-directory entry metadata",
            path: path.to_path_buf(),
            source: io::Error::last_os_error(),
        });
    }
    let last_write = ((info.ftLastWriteTime.dwHighDateTime as u64) << 32)
        | u64::from(info.ftLastWriteTime.dwLowDateTime);
    Ok(FlatDirectoryEntry {
        name,
        kind,
        device: identity.volume_serial(),
        inode: identity.folded_file_id(),
        size: ((info.nFileSizeHigh as u64) << 32) | u64::from(info.nFileSizeLow),
        mtime: native_mtime(last_write, path)?,
    })
}

fn entry_identity(
    handle: std::os::windows::io::RawHandle,
    path: &Path,
) -> Result<WindowsFileIdentity, FlatDirectoryError> {
    file_identity(handle).map_err(|source| FlatDirectoryError::Io {
        operation: "query flat-directory entry identity",
        path: path.to_path_buf(),
        source,
    })
}

fn attribute_tag(
    handle: std::os::windows::io::RawHandle,
    path: &Path,
) -> Result<FILE_ATTRIBUTE_TAG_INFO, FlatDirectoryError> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: `info` is writable for its exact size and `handle` remains valid for the call.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    (result != 0)
        .then_some(info)
        .ok_or_else(|| FlatDirectoryError::Io {
            operation: "query flat-directory entry attributes",
            path: path.to_path_buf(),
            source: io::Error::last_os_error(),
        })
}

fn native_mtime(value: u64, path: &Path) -> Result<NativeMtime, FlatDirectoryError> {
    let ticks = i128::from(value) - WINDOWS_TO_UNIX_EPOCH_100NS;
    let seconds = ticks.div_euclid(HUNDRED_NANOSECONDS_PER_SECOND);
    let nanoseconds = ticks.rem_euclid(HUNDRED_NANOSECONDS_PER_SECOND) * 100;
    Ok(NativeMtime {
        seconds: i64::try_from(seconds).map_err(|_| FlatDirectoryError::Io {
            operation: "convert flat-directory modification time",
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidData, "modification time exceeds i64"),
        })?,
        nanoseconds: i64::try_from(nanoseconds).expect("Windows FILETIME remainder fits i64"),
    })
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

fn is_directory(attributes: FILE_ATTRIBUTE_TAG_INFO) -> bool {
    attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn is_reparse_point(attributes: FILE_ATTRIBUTE_TAG_INFO) -> bool {
    attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(test)]
mod bounded_listing_tests {
    use super::*;

    fn record(name: &str, identity: [u8; 16]) -> Vec<u8> {
        let units = name.encode_utf16().collect::<Vec<_>>();
        let header = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
        let mut bytes = vec![0; header + units.len() * 2];
        let length = offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength);
        bytes[length..length + 4].copy_from_slice(&((units.len() * 2) as u32).to_le_bytes());
        let id = offset_of!(FILE_ID_EXTD_DIR_INFO, FileId);
        bytes[id..id + 16].copy_from_slice(&identity);
        for (index, unit) in units.into_iter().enumerate() {
            bytes[header + index * 2..header + index * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn bounded_decoder_preserves_full_ids_and_zero_limit_is_unknown() {
        let path = Path::new("diagnostic-only");
        let mut high = [0; 16];
        high[0] = 1;
        high[8] = 1;
        let mut decoded = Vec::new();
        for id in [[0; 16], high] {
            assert!(
                parse_directory_buffer_bounded(&record("leaf", id), path, &mut decoded, true, 2)
                    .unwrap()
            );
        }
        assert_ne!(decoded[0].file_id, decoded[1].file_id);
        assert_eq!(decoded[1].file_id, high);
        let mut decoded = Vec::new();
        assert!(
            !parse_directory_buffer_bounded(&record("leaf", high), path, &mut decoded, true, 0)
                .unwrap()
        );
        assert!(decoded.is_empty());
        for name in [".", ".."] {
            assert!(
                parse_directory_buffer_bounded(&record(name, high), path, &mut decoded, true, 0)
                    .unwrap()
            );
        }
        assert!(decoded.is_empty());
    }

    #[test]
    fn bounded_decoder_refuses_partial_and_malformed_records() {
        let path = Path::new("diagnostic-only");
        let valid = record("leaf", [7; 16]);
        let mut malformed = Vec::new();
        malformed.push(valid[..offset_of!(FILE_ID_EXTD_DIR_INFO, FileName) - 1].to_vec());
        malformed.push(valid[..valid.len() - 1].to_vec());
        let mut odd = valid.clone();
        let length = offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength);
        odd[length..length + 4].copy_from_slice(&3u32.to_le_bytes());
        malformed.push(odd);
        let mut overlap = valid;
        let next = offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset);
        overlap[next..next + 4].copy_from_slice(&1u32.to_le_bytes());
        malformed.push(overlap);
        for buffer in malformed {
            assert!(
                parse_directory_buffer_bounded(&buffer, path, &mut Vec::new(), true, 10).is_err()
            );
        }
    }
}

#[cfg(test)]
mod observed_removal_tests {
    use super::*;
    use crate::errors::ClaimRemovalOutcome;

    #[test]
    fn only_the_exact_observed_file_is_deleted_through_its_verified_handle() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::JournalRoot::open(temp.path()).unwrap();
        std::fs::create_dir(temp.path().join("hosted")).unwrap();
        let file = temp.path().join("hosted/binding.json");
        std::fs::write(&file, b"original").unwrap();
        let directory = open_windows_flat_directory_bound(&root, OsStr::new("hosted"), temp.path())
            .unwrap()
            .unwrap();
        let name = OsStr::new("binding.json");
        let observed = read_windows_observed_file_bounded(&directory, name, 1024)
            .unwrap()
            .unwrap();
        // Same size and modification time, different bytes: only the byte
        // comparison can refuse it, and the file stays as it is.
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::fs::write(&file, b"changed!").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            modified
        );
        assert!(remove_windows_observed_file_bounded(&directory, name, &observed, 1024).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"changed!");
        // A different name than the observation is refused before any open.
        assert!(
            remove_windows_observed_file_bounded(&directory, OsStr::new("other"), &observed, 1024)
                .is_err()
        );
        let observed = read_windows_observed_file_bounded(&directory, name, 1024)
            .unwrap()
            .unwrap();
        assert!(matches!(
            remove_windows_observed_file_bounded(&directory, name, &observed, 1024),
            Ok(ClaimRemovalOutcome::RemovedUnflushed)
        ));
        assert!(!file.exists());
        // Nothing left to remove.
        assert!(remove_windows_observed_file_bounded(&directory, name, &observed, 1024).is_err());
    }
}
