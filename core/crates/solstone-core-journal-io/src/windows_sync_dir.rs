// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded no-follow access to one direct Windows directory.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::mem::{offset_of, size_of};
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_IF,
    FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, FileIdExtdDirectoryInformation,
    NtQueryDirectoryFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, RtlNtStatusToDosError,
    STATUS_SUCCESS,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_ID_EXTD_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_READ_DATA, FILE_TRAVERSE, FILE_TYPE_DISK, FileAttributeTagInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx, GetFileType, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use crate::errors::FlatDirectoryError;
use crate::journal_root::JournalEntryKind;
use crate::name_admission::{NameAdmissionReason, check_portable_component};
use crate::observation::{FileObservation, FlatDirectoryEntry, NativeMtime, same_entry_metadata};
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::nt_create_relative;

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

/// List direct entries, returning `None` instead of a partial list above `maximum`.
pub fn list_windows_flat_directory(
    directory: &WindowsFlatDirectory,
    maximum: usize,
) -> Result<Option<Vec<FlatDirectoryEntry>>, FlatDirectoryError> {
    directory.revalidate()?;
    let listed = list_directory(
        directory.directory.as_raw_handle(),
        &directory.diagnostic_path,
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

struct ListedEntry {
    name: OsString,
    file_id: [u8; 16],
}

fn list_directory(
    directory: std::os::windows::io::RawHandle,
    path: &Path,
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
            parse_directory_buffer(&buffer, path, &mut entries)?;
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
) -> Result<(), FlatDirectoryError> {
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
        let name = record[header_bytes..]
            .chunks_exact(size_of::<u16>())
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let name = String::from_utf16(&name).map_err(|_| invalid_listing(path))?;
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
            entries.push(ListedEntry {
                name: OsString::from(name),
                file_id,
            });
        }
        let next = directory_u32(
            header,
            offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset),
            path,
        )?;
        if next == 0 {
            return Ok(());
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
    if is_reparse_point(attributes) {
        return Err(FlatDirectoryError::NotRegular {
            path: path.to_path_buf(),
        });
    }
    let kind = if is_directory(attributes) {
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
