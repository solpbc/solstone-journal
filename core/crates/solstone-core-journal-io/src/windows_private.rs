// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows retained-handle private directory, file, and lock capabilities.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::io::{AsHandle, AsRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_DIRECTORY, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_PATH_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce, DACL_SECURITY_INFORMATION, GetAce,
    GetKernelObjectSecurity, GetLengthSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor,
    OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED, SE_SELF_RELATIVE, SECURITY_DESCRIPTOR,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK, FILE_WRITE_DATA, FileAttributeTagInfo,
    FileDispositionInfo, FlushFileBuffers, GetFileInformationByHandleEx, GetFileType,
    GetVolumeInformationByHandleW, READ_CONTROL, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::atomic::{ATOMIC_CANDIDATE_MARKER, publication_candidate_name};
use crate::journal_root::JournalRoot;
use crate::name_admission::check_portable_component;
use crate::operational_log::on_disk_leaf_matches;
use crate::private_descriptor::{
    DIRECTORY_ACCESS_MASK, DaclState, FILE_ACCESS_MASK, LOCK_ACCESS_MASK, ParsedAce,
    ParsedDescriptor, admit_private_descriptor,
};
use crate::windows_identity::{WindowsFileIdentity, file_identity, file_link_count};
use crate::windows_lock::{WindowsLockGuard, is_contention, try_lock_exclusive};
use crate::windows_ntcreate::nt_create_relative_exact_with_descriptor;
use crate::windows_rename::rename_handle_no_replace;
use crate::windows_sync_dir::validate_windows_regular_handle;

const _: () = {
    assert!(
        DIRECTORY_ACCESS_MASK
            == FILE_LIST_DIRECTORY
                | FILE_ADD_FILE
                | FILE_ADD_SUBDIRECTORY
                | FILE_TRAVERSE
                | FILE_READ_ATTRIBUTES
                | READ_CONTROL
                | SYNCHRONIZE
    );
    assert!(
        FILE_ACCESS_MASK
            == FILE_READ_DATA
                | FILE_WRITE_DATA
                | FILE_READ_ATTRIBUTES
                | DELETE
                | READ_CONTROL
                | SYNCHRONIZE
    );
    assert!(
        LOCK_ACCESS_MASK
            == FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE
    );
};

const STAGE_WRITER_DESIRED_ACCESS: u32 = LOCK_ACCESS_MASK;
const STAGE_RENAME_DESIRED_ACCESS: u32 = DELETE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE;

const DIRECTORY_OPTIONS: u32 =
    FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
const FILE_OPTIONS: u32 =
    FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const SHARE_STAGE_WRITER: u32 = FILE_SHARE_READ | FILE_SHARE_DELETE;

const STAGE_ALLOCATE_ATTEMPTS: usize = 100;
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Execution phase where a private state operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateStatePhase {
    Admission,
    Create,
    Write,
    Flush,
    Security,
    NameBinding,
    Commit,
    PostCommitDurability,
    Cleanup,
    Lock,
}

impl fmt::Display for PrivateStatePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission => write!(f, "admission"),
            Self::Create => write!(f, "create"),
            Self::Write => write!(f, "write"),
            Self::Flush => write!(f, "flush"),
            Self::Security => write!(f, "security"),
            Self::NameBinding => write!(f, "name_binding"),
            Self::Commit => write!(f, "commit"),
            Self::PostCommitDurability => write!(f, "post_commit_durability"),
            Self::Cleanup => write!(f, "cleanup"),
            Self::Lock => write!(f, "lock"),
        }
    }
}

/// Structured error details when private file reading exceeds a requested ceiling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivateReadCeilingExceeded {
    pub observed_size: u64,
    pub ceiling: u64,
}

impl fmt::Display for PrivateReadCeilingExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "observed file size {} exceeds ceiling {}",
            self.observed_size, self.ceiling
        )
    }
}

impl Error for PrivateReadCeilingExceeded {}

/// Failure during a private state operation.
#[derive(Debug)]
pub enum PrivateStateError {
    Unsupported {
        path: PathBuf,
        reason: &'static str,
    },
    Failed {
        path: PathBuf,
        phase: PrivateStatePhase,
        source: io::Error,
    },
}

impl fmt::Display for PrivateStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { path, reason } => {
                write!(f, "{}: unsupported: {reason}", path.display())
            }
            Self::Failed {
                path,
                phase,
                source,
            } => {
                write!(f, "{}: {phase}: {source}", path.display())
            }
        }
    }
}

impl Error for PrivateStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unsupported { .. } => None,
            Self::Failed { source, .. } => Some(source),
        }
    }
}

/// Lock acquisition wait strategy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateLockWait {
    Immediate,
    Bounded(Duration),
}

/// Retained handle representing an admitted private directory.
#[derive(Debug)]
pub struct WindowsPrivateDirectory {
    handle: OwnedHandle,
    identity: WindowsFileIdentity,
    diagnostic_path: PathBuf,
}

impl WindowsPrivateDirectory {
    pub fn identity(&self) -> WindowsFileIdentity {
        self.identity
    }

    fn revalidate(&self) -> Result<(), PrivateStateError> {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        let res = {
            #[allow(unsafe_code)]
            unsafe {
                GetFileInformationByHandleEx(
                    self.handle.as_raw_handle(),
                    FileAttributeTagInfo,
                    (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                    size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
                )
            }
        };
        if res == 0 {
            return Err(PrivateStateError::Failed {
                path: self.diagnostic_path.clone(),
                phase: PrivateStatePhase::Admission,
                source: io::Error::last_os_error(),
            });
        }
        if info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(PrivateStateError::Failed {
                path: self.diagnostic_path.clone(),
                phase: PrivateStatePhase::Admission,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "retained handle is not a directory",
                ),
            });
        }
        if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(PrivateStateError::Failed {
                path: self.diagnostic_path.clone(),
                phase: PrivateStatePhase::Admission,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "retained handle is a reparse point",
                ),
            });
        }
        let current_identity = file_identity(self.handle.as_raw_handle()).map_err(|source| {
            PrivateStateError::Failed {
                path: self.diagnostic_path.clone(),
                phase: PrivateStatePhase::NameBinding,
                source,
            }
        })?;
        if current_identity != self.identity {
            return Err(PrivateStateError::Failed {
                path: self.diagnostic_path.clone(),
                phase: PrivateStatePhase::NameBinding,
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "retained directory identity changed",
                ),
            });
        }
        Ok(())
    }

    fn ensure_ntfs(&self) -> Result<(), PrivateStateError> {
        ensure_handle_is_ntfs(self.handle.as_raw_handle(), &self.diagnostic_path)
    }
}

/// Retained handle owning one whole-file advisory lock on a private lock file.
#[derive(Debug)]
pub struct WindowsPrivateLock {
    #[allow(dead_code)]
    guard: WindowsLockGuard,
    identity: WindowsFileIdentity,
    #[allow(dead_code)]
    diagnostic_path: PathBuf,
}

impl WindowsPrivateLock {
    pub fn identity(&self) -> WindowsFileIdentity {
        self.identity
    }
}

/// Evidence of a successful private file publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublishedPrivateFile {
    identity: WindowsFileIdentity,
}

impl PublishedPrivateFile {
    pub fn identity(&self) -> WindowsFileIdentity {
        self.identity
    }
}

/// Create or open one private directory directly beneath an admitted journal root.
pub fn create_or_open_private_directory(
    root: &JournalRoot,
    name: &str,
) -> Result<WindowsPrivateDirectory, PrivateStateError> {
    root.revalidate().map_err(|_| PrivateStateError::Failed {
        path: root.canonical_path().to_path_buf(),
        phase: PrivateStatePhase::Admission,
        source: io::Error::new(io::ErrorKind::NotFound, "root revalidation failed"),
    })?;
    ensure_handle_is_ntfs(root.as_handle().as_raw_handle(), root.canonical_path())?;
    check_portable_component(name).map_err(|_| PrivateStateError::Failed {
        path: root.canonical_path().join(name),
        phase: PrivateStatePhase::Admission,
        source: io::Error::new(io::ErrorKind::InvalidInput, "invalid portable name"),
    })?;

    create_or_open_private_directory_in_parent(
        root.as_handle().as_raw_handle(),
        root.canonical_path(),
        name,
    )
}

/// Create or open one private child directory directly beneath an admitted private parent directory.
pub fn create_or_open_private_child_directory(
    parent: &WindowsPrivateDirectory,
    name: &str,
) -> Result<WindowsPrivateDirectory, PrivateStateError> {
    parent.revalidate()?;
    parent.ensure_ntfs()?;
    check_portable_component(name).map_err(|_| PrivateStateError::Failed {
        path: parent.diagnostic_path.join(name),
        phase: PrivateStatePhase::Admission,
        source: io::Error::new(io::ErrorKind::InvalidInput, "invalid portable name"),
    })?;

    create_or_open_private_directory_in_parent(
        parent.handle.as_raw_handle(),
        &parent.diagnostic_path,
        name,
    )
}

fn create_or_open_private_directory_in_parent(
    parent_handle: RawHandle,
    parent_path: &Path,
    name: &str,
) -> Result<WindowsPrivateDirectory, PrivateStateError> {
    let diagnostic_path = parent_path.join(name);
    let os_name = OsStr::new(name);
    let owner_sid = query_process_token_user_sid(&diagnostic_path)?;
    let descriptor =
        build_owner_security_descriptor(&owner_sid, DIRECTORY_ACCESS_MASK).map_err(|source| {
            PrivateStateError::Failed {
                path: diagnostic_path.clone(),
                phase: PrivateStatePhase::Security,
                source,
            }
        })?;

    let handle = match nt_create_relative_exact_with_descriptor(
        parent_handle,
        os_name,
        DIRECTORY_ACCESS_MASK,
        FILE_OPEN,
        DIRECTORY_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(handle) => handle,
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            match nt_create_relative_exact_with_descriptor(
                parent_handle,
                os_name,
                DIRECTORY_ACCESS_MASK,
                FILE_CREATE,
                DIRECTORY_OPTIONS,
                SHARE_ALL,
                descriptor.as_ptr(),
            ) {
                Ok(created) => created,
                Err(err)
                    if matches!(
                        err.raw_os_error(),
                        Some(code) if code == ERROR_FILE_EXISTS as i32 || code == ERROR_ALREADY_EXISTS as i32
                    ) =>
                {
                    nt_create_relative_exact_with_descriptor(
                        parent_handle,
                        os_name,
                        DIRECTORY_ACCESS_MASK,
                        FILE_OPEN,
                        DIRECTORY_OPTIONS,
                        SHARE_ALL,
                        std::ptr::null(),
                    )
                    .map_err(|source| PrivateStateError::Failed {
                        path: diagnostic_path.clone(),
                        phase: PrivateStatePhase::Admission,
                        source,
                    })?
                }
                Err(source) => {
                    return Err(PrivateStateError::Failed {
                        path: diagnostic_path,
                        phase: PrivateStatePhase::Create,
                        source,
                    });
                }
            }
        }
        Err(source) => {
            return Err(PrivateStateError::Failed {
                path: diagnostic_path,
                phase: PrivateStatePhase::Admission,
                source,
            });
        }
    };

    let file_type = {
        #[allow(unsafe_code)]
        unsafe {
            GetFileType(handle.as_raw_handle())
        }
    };
    if file_type != FILE_TYPE_DISK {
        return Err(PrivateStateError::Failed {
            path: diagnostic_path,
            phase: PrivateStatePhase::Admission,
            source: io::Error::new(io::ErrorKind::InvalidData, "directory is not a disk object"),
        });
    }

    if !on_disk_leaf_matches(handle.as_raw_handle(), os_name) {
        return Err(PrivateStateError::Failed {
            path: diagnostic_path,
            phase: PrivateStatePhase::NameBinding,
            source: io::Error::new(io::ErrorKind::InvalidInput, "on-disk leaf name mismatch"),
        });
    }

    admit_handle_security_descriptor(
        handle.as_raw_handle(),
        DIRECTORY_ACCESS_MASK,
        &diagnostic_path,
    )?;

    let identity =
        file_identity(handle.as_raw_handle()).map_err(|source| PrivateStateError::Failed {
            path: diagnostic_path.clone(),
            phase: PrivateStatePhase::Admission,
            source,
        })?;

    Ok(WindowsPrivateDirectory {
        handle,
        identity,
        diagnostic_path,
    })
}

/// Read one private regular file, returning `Ok(None)` for absence and failing on any refusal.
pub fn read_private_file(
    directory: &WindowsPrivateDirectory,
    name: &str,
    ceiling: u64,
) -> Result<Option<Vec<u8>>, PrivateStateError> {
    directory.revalidate()?;
    directory.ensure_ntfs()?;
    let path = directory.diagnostic_path.join(name);
    check_portable_component(name).map_err(|_| PrivateStateError::Failed {
        path: path.clone(),
        phase: PrivateStatePhase::Admission,
        source: io::Error::new(io::ErrorKind::InvalidInput, "invalid portable name"),
    })?;

    let os_name = OsStr::new(name);
    let handle = match nt_create_relative_exact_with_descriptor(
        directory.handle.as_raw_handle(),
        os_name,
        FILE_ACCESS_MASK,
        FILE_OPEN,
        FILE_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(handle) => handle,
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            return Ok(None);
        }
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(code) if code == ERROR_DIRECTORY as i32
            ) =>
        {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::Admission,
                source: err,
            });
        }
        Err(source) => {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::Security,
                source,
            });
        }
    };

    let identity =
        validate_windows_regular_handle(handle.as_raw_handle(), &path).map_err(|flat_err| {
            PrivateStateError::Failed {
                path: path.clone(),
                phase: PrivateStatePhase::Admission,
                source: io::Error::new(io::ErrorKind::InvalidData, flat_err.to_string()),
            }
        })?;

    let links =
        file_link_count(handle.as_raw_handle()).map_err(|source| PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::Admission,
            source,
        })?;
    if links != 1 {
        return Err(PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::Admission,
            source: io::Error::new(io::ErrorKind::InvalidData, "hard links count must be 1"),
        });
    }

    if !on_disk_leaf_matches(handle.as_raw_handle(), os_name) {
        return Err(PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::NameBinding,
            source: io::Error::new(io::ErrorKind::InvalidInput, "on-disk leaf name mismatch"),
        });
    }

    admit_handle_security_descriptor(handle.as_raw_handle(), FILE_ACCESS_MASK, &path)?;

    let mut file = File::from(handle);
    let metadata = file
        .metadata()
        .map_err(|source| PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::Admission,
            source,
        })?;
    let size = metadata.len();
    if size > ceiling {
        return Err(PrivateStateError::Failed {
            path,
            phase: PrivateStatePhase::Admission,
            source: io::Error::other(PrivateReadCeilingExceeded {
                observed_size: size,
                ceiling,
            }),
        });
    }

    #[cfg(any(test, feature = "test-hooks"))]
    test_hooks::trigger_read_barrier();

    let recheck_handle = match nt_create_relative_exact_with_descriptor(
        directory.handle.as_raw_handle(),
        os_name,
        FILE_ACCESS_MASK,
        FILE_OPEN,
        FILE_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(h) => h,
        Err(source) => {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::NameBinding,
                source,
            });
        }
    };
    let recheck_id = match file_identity(recheck_handle.as_raw_handle()) {
        Ok(id) => id,
        Err(source) => {
            drop(recheck_handle);
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::NameBinding,
                source,
            });
        }
    };
    drop(recheck_handle);
    if recheck_id != identity {
        return Err(PrivateStateError::Failed {
            path,
            phase: PrivateStatePhase::NameBinding,
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "file identity changed after admission",
            ),
        });
    }

    let mut buf = vec![0u8; size as usize];
    if size > 0 {
        if let Err(source) = file.read_exact(&mut buf) {
            drop(buf);
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::Write,
                source,
            });
        }
    }

    let current_identity =
        file_identity(file.as_raw_handle()).map_err(|source| PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::NameBinding,
            source,
        })?;
    if current_identity != identity {
        return Err(PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::NameBinding,
            source: io::Error::new(io::ErrorKind::InvalidData, "file identity changed"),
        });
    }

    let links_after =
        file_link_count(file.as_raw_handle()).map_err(|source| PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::Admission,
            source,
        })?;
    if links_after != 1 {
        return Err(PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::Admission,
            source: io::Error::new(io::ErrorKind::InvalidData, "hard link count changed"),
        });
    }

    if !on_disk_leaf_matches(file.as_raw_handle(), os_name) {
        return Err(PrivateStateError::Failed {
            path: path.clone(),
            phase: PrivateStatePhase::NameBinding,
            source: io::Error::new(io::ErrorKind::InvalidInput, "on-disk leaf name changed"),
        });
    }

    admit_handle_security_descriptor(file.as_raw_handle(), FILE_ACCESS_MASK, &path)?;
    directory.revalidate()?;

    Ok(Some(buf))
}

/// Publish a private file create-only with owner-only ACL and durability guarantees.
pub fn publish_private_file(
    directory: &WindowsPrivateDirectory,
    name: &str,
    bytes: &[u8],
) -> Result<PublishedPrivateFile, PrivateStateError> {
    directory.revalidate()?;
    directory.ensure_ntfs()?;
    let dest_path = directory.diagnostic_path.join(name);
    check_portable_component(name).map_err(|_| PrivateStateError::Failed {
        path: dest_path.clone(),
        phase: PrivateStatePhase::Admission,
        source: io::Error::new(io::ErrorKind::InvalidInput, "invalid portable name"),
    })?;

    let dest_os_name = OsStr::new(name);
    match nt_create_relative_exact_with_descriptor(
        directory.handle.as_raw_handle(),
        dest_os_name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(handle) => {
            drop(handle);
            return Err(PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Create,
                source: io::Error::from_raw_os_error(ERROR_ALREADY_EXISTS as i32),
            });
        }
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) => {}
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(code) if code == ERROR_DIRECTORY as i32
            ) =>
        {
            return Err(PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Admission,
                source: err,
            });
        }
        Err(source) => {
            return Err(PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Create,
                source,
            });
        }
    }

    let owner_sid = query_process_token_user_sid(&dest_path)?;
    let descriptor =
        build_owner_security_descriptor(&owner_sid, FILE_ACCESS_MASK).map_err(|source| {
            PrivateStateError::Failed {
                path: dest_path.clone(),
                phase: PrivateStatePhase::Security,
                source,
            }
        })?;

    let mut staged_handle = None;
    let mut staged_name = OsString::new();
    for _ in 0..STAGE_ALLOCATE_ATTEMPTS {
        let mut random_bytes = [0u8; 16];
        getrandom::fill(&mut random_bytes).map_err(|source| PrivateStateError::Failed {
            path: dest_path.clone(),
            phase: PrivateStatePhase::Create,
            source: io::Error::new(io::ErrorKind::Other, source.to_string()),
        })?;
        let candidate = publication_candidate_name(
            dest_os_name,
            ATOMIC_CANDIDATE_MARKER,
            &[
                std::process::id() as u128,
                u128::from_le_bytes(random_bytes),
            ],
        );
        match nt_create_relative_exact_with_descriptor(
            directory.handle.as_raw_handle(),
            &candidate,
            STAGE_WRITER_DESIRED_ACCESS,
            FILE_CREATE,
            FILE_OPTIONS,
            SHARE_STAGE_WRITER,
            descriptor.as_ptr(),
        ) {
            Ok(handle) => {
                staged_handle = Some(handle);
                staged_name = candidate;
                break;
            }
            Err(err)
                if matches!(
                    err.raw_os_error(),
                    Some(code) if code == ERROR_FILE_EXISTS as i32 || code == ERROR_ALREADY_EXISTS as i32
                ) =>
            {
                continue;
            }
            Err(source) => {
                return Err(PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::Create,
                    source,
                });
            }
        }
    }

    let staged_handle = staged_handle.ok_or_else(|| PrivateStateError::Failed {
        path: dest_path.clone(),
        phase: PrivateStatePhase::Create,
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate unique stage",
        ),
    })?;

    let stage_path = directory.diagnostic_path.join(&staged_name);

    let stage_identity =
        match validate_windows_regular_handle(staged_handle.as_raw_handle(), &stage_path) {
            Ok(id) => id,
            Err(err) => {
                drop(staged_handle);
                let cleanup_res = cleanup_stage(
                    directory.handle.as_raw_handle(),
                    &staged_name,
                    None,
                    &stage_path,
                );
                return Err(cleanup_res
                    .err()
                    .unwrap_or_else(|| PrivateStateError::Failed {
                        path: dest_path,
                        phase: PrivateStatePhase::Admission,
                        source: io::Error::new(io::ErrorKind::InvalidData, err.to_string()),
                    }));
            }
        };

    let links = match file_link_count(staged_handle.as_raw_handle()) {
        Ok(links) => links,
        Err(source) => {
            drop(staged_handle);
            let cleanup_res = cleanup_stage(
                directory.handle.as_raw_handle(),
                &staged_name,
                Some(stage_identity),
                &stage_path,
            );
            return Err(cleanup_res
                .err()
                .unwrap_or_else(|| PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::Admission,
                    source,
                }));
        }
    };
    if links != 1 {
        drop(staged_handle);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Admission,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "stage file link count must be 1",
                ),
            }));
    }

    if !on_disk_leaf_matches(staged_handle.as_raw_handle(), &staged_name) {
        drop(staged_handle);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::NameBinding,
                source: io::Error::new(io::ErrorKind::InvalidInput, "stage leaf name mismatch"),
            }));
    }

    if let Err(err) = admit_handle_security_descriptor(
        staged_handle.as_raw_handle(),
        FILE_ACCESS_MASK,
        &stage_path,
    ) {
        drop(staged_handle);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res.err().unwrap_or(err));
    }

    let mut writer_file = File::from(staged_handle);

    #[cfg(any(test, feature = "test-hooks"))]
    if test_hooks::write_fault_active() {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Write,
                source: io::Error::new(io::ErrorKind::Other, "injected write fault"),
            }));
    }

    if let Err(source) = writer_file.write_all(bytes) {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Write,
                source,
            }));
    }

    #[cfg(any(test, feature = "test-hooks"))]
    if test_hooks::pre_commit_flush_fault_active() {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Flush,
                source: io::Error::new(io::ErrorKind::Other, "injected pre-commit flush fault"),
            }));
    }

    let flushed = {
        #[allow(unsafe_code)]
        unsafe {
            FlushFileBuffers(writer_file.as_raw_handle())
        }
    };
    if flushed == 0 {
        let err = io::Error::last_os_error();
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Flush,
                source: err,
            }));
    }

    let current_id = match file_identity(writer_file.as_raw_handle()) {
        Ok(id) => id,
        Err(source) => {
            drop(writer_file);
            let cleanup_res = cleanup_stage(
                directory.handle.as_raw_handle(),
                &staged_name,
                Some(stage_identity),
                &stage_path,
            );
            return Err(cleanup_res
                .err()
                .unwrap_or_else(|| PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::NameBinding,
                    source,
                }));
        }
    };
    if current_id != stage_identity {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::NameBinding,
                source: io::Error::new(io::ErrorKind::InvalidData, "stage identity changed"),
            }));
    }

    let links = match file_link_count(writer_file.as_raw_handle()) {
        Ok(l) => l,
        Err(source) => {
            drop(writer_file);
            let cleanup_res = cleanup_stage(
                directory.handle.as_raw_handle(),
                &staged_name,
                Some(stage_identity),
                &stage_path,
            );
            return Err(cleanup_res
                .err()
                .unwrap_or_else(|| PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::Admission,
                    source,
                }));
        }
    };
    if links != 1 {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Admission,
                source: io::Error::new(io::ErrorKind::InvalidData, "stage link count changed"),
            }));
    }

    if !on_disk_leaf_matches(writer_file.as_raw_handle(), &staged_name) {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::NameBinding,
                source: io::Error::new(io::ErrorKind::InvalidInput, "stage leaf name changed"),
            }));
    }

    if let Err(err) =
        admit_handle_security_descriptor(writer_file.as_raw_handle(), FILE_ACCESS_MASK, &stage_path)
    {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res.err().unwrap_or(err));
    }

    if let Err(err) = directory.revalidate() {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res.err().unwrap_or(err));
    }

    #[cfg(any(test, feature = "test-hooks"))]
    test_hooks::trigger_descriptor_barrier(&stage_path);

    if let Err(err) =
        admit_handle_security_descriptor(writer_file.as_raw_handle(), FILE_ACCESS_MASK, &stage_path)
    {
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res.err().unwrap_or(err));
    }

    #[cfg(any(test, feature = "test-hooks"))]
    test_hooks::trigger_destination_barrier();

    match nt_create_relative_exact_with_descriptor(
        directory.handle.as_raw_handle(),
        dest_os_name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(handle) => {
            drop(handle);
            drop(writer_file);
            let cleanup_res = cleanup_stage(
                directory.handle.as_raw_handle(),
                &staged_name,
                Some(stage_identity),
                &stage_path,
            );
            return Err(cleanup_res
                .err()
                .unwrap_or_else(|| PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::Commit,
                    source: io::Error::from_raw_os_error(ERROR_ALREADY_EXISTS as i32),
                }));
        }
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) => {}
        Err(source) => {
            drop(writer_file);
            let cleanup_res = cleanup_stage(
                directory.handle.as_raw_handle(),
                &staged_name,
                Some(stage_identity),
                &stage_path,
            );
            return Err(cleanup_res
                .err()
                .unwrap_or_else(|| PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::Commit,
                    source,
                }));
        }
    }

    let rename_handle = match nt_create_relative_exact_with_descriptor(
        directory.handle.as_raw_handle(),
        &staged_name,
        STAGE_RENAME_DESIRED_ACCESS,
        FILE_OPEN,
        FILE_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(h) => h,
        Err(source) => {
            drop(writer_file);
            let cleanup_res = cleanup_stage(
                directory.handle.as_raw_handle(),
                &staged_name,
                Some(stage_identity),
                &stage_path,
            );
            return Err(cleanup_res
                .err()
                .unwrap_or_else(|| PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::Commit,
                    source,
                }));
        }
    };

    let rename_identity = match file_identity(rename_handle.as_raw_handle()) {
        Ok(id) => id,
        Err(source) => {
            drop(rename_handle);
            drop(writer_file);
            let cleanup_res = cleanup_stage(
                directory.handle.as_raw_handle(),
                &staged_name,
                Some(stage_identity),
                &stage_path,
            );
            return Err(cleanup_res
                .err()
                .unwrap_or_else(|| PrivateStateError::Failed {
                    path: dest_path,
                    phase: PrivateStatePhase::Commit,
                    source,
                }));
        }
    };
    if rename_identity != stage_identity {
        drop(rename_handle);
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Commit,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "stage rename handle identity mismatch",
                ),
            }));
    }

    if let Err(source) = rename_handle_no_replace(
        directory.handle.as_raw_handle(),
        rename_handle.as_raw_handle(),
        dest_os_name,
    ) {
        drop(rename_handle);
        drop(writer_file);
        let cleanup_res = cleanup_stage(
            directory.handle.as_raw_handle(),
            &staged_name,
            Some(stage_identity),
            &stage_path,
        );
        return Err(cleanup_res
            .err()
            .unwrap_or_else(|| PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::Commit,
                source,
            }));
    }
    drop(rename_handle);

    let proof_handle = match nt_create_relative_exact_with_descriptor(
        directory.handle.as_raw_handle(),
        dest_os_name,
        FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(h) => h,
        Err(source) => {
            drop(writer_file);
            return Err(PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::PostCommitDurability,
                source,
            });
        }
    };

    let observed_id = match file_identity(proof_handle.as_raw_handle()) {
        Ok(id) => id,
        Err(source) => {
            drop(proof_handle);
            drop(writer_file);
            return Err(PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::PostCommitDurability,
                source,
            });
        }
    };
    if observed_id != stage_identity {
        drop(proof_handle);
        drop(writer_file);
        return Err(PrivateStateError::Failed {
            path: dest_path,
            phase: PrivateStatePhase::PostCommitDurability,
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "published file identity mismatch",
            ),
        });
    }

    let links = match file_link_count(proof_handle.as_raw_handle()) {
        Ok(l) => l,
        Err(source) => {
            drop(proof_handle);
            drop(writer_file);
            return Err(PrivateStateError::Failed {
                path: dest_path,
                phase: PrivateStatePhase::PostCommitDurability,
                source,
            });
        }
    };
    if links != 1 {
        drop(proof_handle);
        drop(writer_file);
        return Err(PrivateStateError::Failed {
            path: dest_path,
            phase: PrivateStatePhase::PostCommitDurability,
            source: io::Error::new(io::ErrorKind::InvalidData, "published link count must be 1"),
        });
    }

    if !on_disk_leaf_matches(proof_handle.as_raw_handle(), dest_os_name) {
        drop(proof_handle);
        drop(writer_file);
        return Err(PrivateStateError::Failed {
            path: dest_path,
            phase: PrivateStatePhase::PostCommitDurability,
            source: io::Error::new(io::ErrorKind::InvalidInput, "published leaf name mismatch"),
        });
    }
    drop(proof_handle);

    if let Err(err) =
        admit_handle_security_descriptor(writer_file.as_raw_handle(), FILE_ACCESS_MASK, &dest_path)
    {
        drop(writer_file);
        return Err(match err {
            PrivateStateError::Failed { path, source, .. } => PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::PostCommitDurability,
                source,
            },
            other => other,
        });
    }

    #[cfg(any(test, feature = "test-hooks"))]
    if test_hooks::post_commit_flush_fault_active() {
        drop(writer_file);
        return Err(PrivateStateError::Failed {
            path: dest_path,
            phase: PrivateStatePhase::PostCommitDurability,
            source: io::Error::new(io::ErrorKind::Other, "injected post-commit flush fault"),
        });
    }

    let flushed = {
        #[allow(unsafe_code)]
        unsafe {
            FlushFileBuffers(writer_file.as_raw_handle())
        }
    };
    drop(writer_file);
    if flushed == 0 {
        return Err(PrivateStateError::Failed {
            path: dest_path,
            phase: PrivateStatePhase::PostCommitDurability,
            source: io::Error::last_os_error(),
        });
    }

    Ok(PublishedPrivateFile {
        identity: stage_identity,
    })
}

fn cleanup_stage(
    parent_handle: RawHandle,
    stage_name: &OsStr,
    expected_identity: Option<WindowsFileIdentity>,
    stage_path: &Path,
) -> Result<(), PrivateStateError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if test_hooks::cleanup_fault_active() {
        return Err(PrivateStateError::Failed {
            path: stage_path.to_path_buf(),
            phase: PrivateStatePhase::Cleanup,
            source: io::Error::new(io::ErrorKind::Other, "injected cleanup fault"),
        });
    }

    let Some(expected) = expected_identity else {
        return Ok(());
    };

    let opened = match nt_create_relative_exact_with_descriptor(
        parent_handle,
        stage_name,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPTIONS,
        SHARE_ALL,
        std::ptr::null(),
    ) {
        Ok(h) => h,
        Err(_) => return Ok(()),
    };

    if let Ok(actual) = file_identity(opened.as_raw_handle()) {
        if actual != expected {
            return Ok(());
        }
    } else {
        return Ok(());
    }

    let mut info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let result = {
        #[allow(unsafe_code)]
        unsafe {
            SetFileInformationByHandle(
                opened.as_raw_handle(),
                FileDispositionInfo,
                (&mut info as *mut FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
    };
    if result == 0 {
        Err(PrivateStateError::Failed {
            path: stage_path.to_path_buf(),
            phase: PrivateStatePhase::Cleanup,
            source: io::Error::last_os_error(),
        })
    } else {
        Ok(())
    }
}

/// Acquire one whole-file advisory lock on a private lock file.
pub fn acquire_private_lock(
    directory: &WindowsPrivateDirectory,
    name: &str,
    wait: PrivateLockWait,
) -> Result<WindowsPrivateLock, PrivateStateError> {
    directory.revalidate()?;
    directory.ensure_ntfs()?;
    let path = directory.diagnostic_path.join(name);
    check_portable_component(name).map_err(|_| PrivateStateError::Failed {
        path: path.clone(),
        phase: PrivateStatePhase::Admission,
        source: io::Error::new(io::ErrorKind::InvalidInput, "invalid portable name"),
    })?;

    let os_name = OsStr::new(name);
    let owner_sid = query_process_token_user_sid(&path)?;
    let descriptor =
        build_owner_security_descriptor(&owner_sid, LOCK_ACCESS_MASK).map_err(|source| {
            PrivateStateError::Failed {
                path: path.clone(),
                phase: PrivateStatePhase::Security,
                source,
            }
        })?;

    let deadline = match wait {
        PrivateLockWait::Immediate => Instant::now(),
        PrivateLockWait::Bounded(duration) => Instant::now() + duration,
    };

    loop {
        let handle = match nt_create_relative_exact_with_descriptor(
            directory.handle.as_raw_handle(),
            os_name,
            LOCK_ACCESS_MASK,
            FILE_OPEN,
            FILE_OPTIONS,
            SHARE_ALL,
            std::ptr::null(),
        ) {
            Ok(h) => h,
            Err(err)
                if matches!(
                    err.raw_os_error(),
                    Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
                ) =>
            {
                match nt_create_relative_exact_with_descriptor(
                    directory.handle.as_raw_handle(),
                    os_name,
                    LOCK_ACCESS_MASK,
                    FILE_CREATE,
                    FILE_OPTIONS,
                    SHARE_ALL,
                    descriptor.as_ptr(),
                ) {
                    Ok(created) => created,
                    Err(err)
                        if matches!(
                            err.raw_os_error(),
                            Some(code) if code == ERROR_FILE_EXISTS as i32 || code == ERROR_ALREADY_EXISTS as i32
                        ) =>
                    {
                        continue;
                    }
                    Err(source) => {
                        return Err(PrivateStateError::Failed {
                            path,
                            phase: PrivateStatePhase::Create,
                            source,
                        });
                    }
                }
            }
            Err(err)
                if matches!(
                    err.raw_os_error(),
                    Some(code) if code == ERROR_DIRECTORY as i32
                ) =>
            {
                return Err(PrivateStateError::Failed {
                    path,
                    phase: PrivateStatePhase::Admission,
                    source: err,
                });
            }
            Err(source) => {
                return Err(PrivateStateError::Failed {
                    path,
                    phase: PrivateStatePhase::Admission,
                    source,
                });
            }
        };

        let identity = match validate_windows_regular_handle(handle.as_raw_handle(), &path) {
            Ok(id) => id,
            Err(flat_err) => {
                return Err(PrivateStateError::Failed {
                    path,
                    phase: PrivateStatePhase::Admission,
                    source: io::Error::new(io::ErrorKind::InvalidData, flat_err.to_string()),
                });
            }
        };

        let links = file_link_count(handle.as_raw_handle()).map_err(|source| {
            PrivateStateError::Failed {
                path: path.clone(),
                phase: PrivateStatePhase::Admission,
                source,
            }
        })?;
        if links != 1 {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::Admission,
                source: io::Error::new(io::ErrorKind::InvalidData, "lock link count must be 1"),
            });
        }

        if !on_disk_leaf_matches(handle.as_raw_handle(), os_name) {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::NameBinding,
                source: io::Error::new(io::ErrorKind::InvalidInput, "lock leaf name mismatch"),
            });
        }

        admit_handle_security_descriptor(handle.as_raw_handle(), LOCK_ACCESS_MASK, &path)?;

        #[cfg(any(test, feature = "test-hooks"))]
        test_hooks::trigger_lock_barrier();

        let recheck_handle = match nt_create_relative_exact_with_descriptor(
            directory.handle.as_raw_handle(),
            os_name,
            LOCK_ACCESS_MASK,
            FILE_OPEN,
            FILE_OPTIONS,
            SHARE_ALL,
            std::ptr::null(),
        ) {
            Ok(h) => h,
            Err(source) => {
                return Err(PrivateStateError::Failed {
                    path,
                    phase: PrivateStatePhase::NameBinding,
                    source,
                });
            }
        };
        let recheck_id = match file_identity(recheck_handle.as_raw_handle()) {
            Ok(id) => id,
            Err(source) => {
                drop(recheck_handle);
                return Err(PrivateStateError::Failed {
                    path,
                    phase: PrivateStatePhase::NameBinding,
                    source,
                });
            }
        };
        drop(recheck_handle);
        if recheck_id != identity {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::NameBinding,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "lock file identity changed before acquire",
                ),
            });
        }

        let links_before_lock = match file_link_count(handle.as_raw_handle()) {
            Ok(l) => l,
            Err(source) => {
                return Err(PrivateStateError::Failed {
                    path,
                    phase: PrivateStatePhase::Admission,
                    source,
                });
            }
        };
        if links_before_lock != 1 {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::Admission,
                source: io::Error::new(io::ErrorKind::InvalidData, "lock link count changed"),
            });
        }

        if !on_disk_leaf_matches(handle.as_raw_handle(), os_name) {
            return Err(PrivateStateError::Failed {
                path,
                phase: PrivateStatePhase::NameBinding,
                source: io::Error::new(io::ErrorKind::InvalidInput, "lock leaf name changed"),
            });
        }

        admit_handle_security_descriptor(handle.as_raw_handle(), LOCK_ACCESS_MASK, &path)?;

        let file = File::from(handle);
        match try_lock_exclusive(file) {
            Ok(guard) => {
                return Ok(WindowsPrivateLock {
                    guard,
                    identity,
                    diagnostic_path: path,
                });
            }
            Err((file, err)) if is_contention(&err) => {
                drop(file);
                match wait {
                    PrivateLockWait::Immediate => {
                        return Err(PrivateStateError::Failed {
                            path,
                            phase: PrivateStatePhase::Lock,
                            source: err,
                        });
                    }
                    PrivateLockWait::Bounded(duration) => {
                        if duration.is_zero() || Instant::now() >= deadline {
                            return Err(PrivateStateError::Failed {
                                path,
                                phase: PrivateStatePhase::Lock,
                                source: err,
                            });
                        }
                        thread::sleep(LOCK_RETRY_INTERVAL);
                    }
                }
            }
            Err((file, source)) => {
                drop(file);
                return Err(PrivateStateError::Failed {
                    path,
                    phase: PrivateStatePhase::Lock,
                    source,
                });
            }
        }
    }
}

fn ensure_handle_is_ntfs(handle: RawHandle, path: &Path) -> Result<(), PrivateStateError> {
    let mut volume_name = [0u16; 256];
    let mut filesystem_name = [0u16; 256];
    let mut serial = 0;
    let mut max_component = 0;
    let mut flags = 0;
    let result = {
        #[allow(unsafe_code)]
        unsafe {
            GetVolumeInformationByHandleW(
                handle,
                volume_name.as_mut_ptr(),
                volume_name.len() as u32,
                &mut serial,
                &mut max_component,
                &mut flags,
                filesystem_name.as_mut_ptr(),
                filesystem_name.len() as u32,
            )
        }
    };
    if result == 0 {
        return Err(PrivateStateError::Unsupported {
            path: path.to_path_buf(),
            reason: "volume filesystem could not be verified",
        });
    }

    let terminator = filesystem_name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(filesystem_name.len());
    let fs_str = String::from_utf16_lossy(&filesystem_name[..terminator]);
    if fs_str != "NTFS" {
        return Err(PrivateStateError::Unsupported {
            path: path.to_path_buf(),
            reason: "filesystem is not NTFS",
        });
    }
    Ok(())
}

fn query_process_token_user_sid(path: &Path) -> Result<Vec<u8>, PrivateStateError> {
    let mut token: HANDLE = INVALID_HANDLE_VALUE;
    let ok = {
        #[allow(unsafe_code)]
        unsafe {
            GetCurrentProcess();
            windows_sys::Win32::System::Threading::OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY,
                &mut token,
            )
        }
    };
    if ok == 0 {
        return Err(PrivateStateError::Failed {
            path: path.to_path_buf(),
            phase: PrivateStatePhase::Security,
            source: io::Error::last_os_error(),
        });
    }

    struct TokenGuard(HANDLE);
    impl Drop for TokenGuard {
        fn drop(&mut self) {
            #[allow(unsafe_code)]
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let _guard = TokenGuard(token);

    let mut needed = 0u32;
    {
        #[allow(unsafe_code)]
        unsafe {
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        }
    }
    if needed == 0 {
        return Err(PrivateStateError::Failed {
            path: path.to_path_buf(),
            phase: PrivateStatePhase::Security,
            source: io::Error::last_os_error(),
        });
    }

    let mut buffer = vec![0u8; needed as usize];
    let ok = {
        #[allow(unsafe_code)]
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        }
    };
    if ok == 0 {
        return Err(PrivateStateError::Failed {
            path: path.to_path_buf(),
            phase: PrivateStatePhase::Security,
            source: io::Error::last_os_error(),
        });
    }

    #[allow(unsafe_code)]
    let sid_bytes = unsafe {
        let token_user = buffer.as_ptr().cast::<TOKEN_USER>();
        let psid = (*token_user).User.Sid;
        if psid.is_null() {
            return Err(PrivateStateError::Failed {
                path: path.to_path_buf(),
                phase: PrivateStatePhase::Security,
                source: io::Error::new(io::ErrorKind::InvalidData, "TokenUser returned null SID"),
            });
        }
        let sid_len = GetLengthSid(psid) as usize;
        std::slice::from_raw_parts(psid.cast::<u8>(), sid_len).to_vec()
    };

    Ok(sid_bytes)
}

struct BuiltDescriptor {
    sd: SECURITY_DESCRIPTOR,
    #[allow(dead_code)]
    acl_buffer: Vec<u8>,
}

impl BuiltDescriptor {
    fn as_ptr(&self) -> *const std::ffi::c_void {
        (&self.sd as *const SECURITY_DESCRIPTOR).cast()
    }
}

fn build_owner_security_descriptor(owner_sid: &[u8], mask: u32) -> io::Result<BuiltDescriptor> {
    let mut sd = SECURITY_DESCRIPTOR {
        Revision: 0,
        Sbz1: 0,
        Control: 0,
        Owner: std::ptr::null_mut(),
        Group: std::ptr::null_mut(),
        Sacl: std::ptr::null_mut(),
        Dacl: std::ptr::null_mut(),
    };

    let init_ok = {
        #[allow(unsafe_code)]
        unsafe {
            InitializeSecurityDescriptor((&mut sd as *mut SECURITY_DESCRIPTOR).cast(), 1)
        }
    };
    if init_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let psid = owner_sid.as_ptr() as PSID;
    let set_owner_ok = {
        #[allow(unsafe_code)]
        unsafe {
            SetSecurityDescriptorOwner((&mut sd as *mut SECURITY_DESCRIPTOR).cast(), psid, 0)
        }
    };
    if set_owner_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let acl_size = size_of::<ACL>()
        .checked_add(size_of::<ACCESS_ALLOWED_ACE>())
        .and_then(|s| s.checked_sub(size_of::<u32>()))
        .and_then(|s| s.checked_add(owner_sid.len()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ACL size overflow"))?;

    let mut acl_buffer = vec![0u8; acl_size];
    let acl_ptr = acl_buffer.as_mut_ptr().cast::<ACL>();

    let init_acl_ok = {
        #[allow(unsafe_code)]
        unsafe {
            InitializeAcl(acl_ptr, acl_size as u32, ACL_REVISION as u32)
        }
    };
    if init_acl_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let add_ace_ok = {
        #[allow(unsafe_code)]
        unsafe {
            AddAccessAllowedAce(acl_ptr, ACL_REVISION as u32, mask, psid)
        }
    };
    if add_ace_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let set_dacl_ok = {
        #[allow(unsafe_code)]
        unsafe {
            SetSecurityDescriptorDacl((&mut sd as *mut SECURITY_DESCRIPTOR).cast(), 1, acl_ptr, 0)
        }
    };
    if set_dacl_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let set_ctrl_ok = {
        #[allow(unsafe_code)]
        unsafe {
            SetSecurityDescriptorControl(
                (&mut sd as *mut SECURITY_DESCRIPTOR).cast(),
                SE_DACL_PROTECTED,
                SE_DACL_PROTECTED,
            )
        }
    };
    if set_ctrl_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(BuiltDescriptor { sd, acl_buffer })
}

fn admit_handle_security_descriptor(
    handle: RawHandle,
    needed_mask: u32,
    diagnostic_path: &Path,
) -> Result<(), PrivateStateError> {
    let mut needed = 0u32;
    {
        #[allow(unsafe_code)]
        unsafe {
            GetKernelObjectSecurity(
                handle,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
        }
    }
    if needed == 0 {
        return Err(PrivateStateError::Failed {
            path: diagnostic_path.to_path_buf(),
            phase: PrivateStatePhase::Security,
            source: io::Error::last_os_error(),
        });
    }

    let mut buffer = vec![0u8; needed as usize];
    let ok = {
        #[allow(unsafe_code)]
        unsafe {
            GetKernelObjectSecurity(
                handle,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        }
    };
    if ok == 0 {
        return Err(PrivateStateError::Failed {
            path: diagnostic_path.to_path_buf(),
            phase: PrivateStatePhase::Security,
            source: io::Error::last_os_error(),
        });
    }

    let parsed =
        parse_security_descriptor_bytes(&buffer).map_err(|source| PrivateStateError::Failed {
            path: diagnostic_path.to_path_buf(),
            phase: PrivateStatePhase::Security,
            source,
        })?;

    admit_private_descriptor(&parsed, needed_mask).map_err(|reason| PrivateStateError::Failed {
        path: diagnostic_path.to_path_buf(),
        phase: PrivateStatePhase::Security,
        source: io::Error::new(io::ErrorKind::PermissionDenied, reason),
    })
}

fn parse_security_descriptor_bytes(buffer: &[u8]) -> io::Result<ParsedDescriptor> {
    let sd_ptr = buffer.as_ptr() as *const SECURITY_DESCRIPTOR;

    let mut owner_psid: PSID = std::ptr::null_mut();
    let mut owner_defaulted = 0i32;
    let get_owner_ok = {
        #[allow(unsafe_code)]
        unsafe {
            GetSecurityDescriptorOwner(
                sd_ptr.cast_mut().cast(),
                &mut owner_psid,
                &mut owner_defaulted,
            )
        }
    };
    if get_owner_ok == 0 || owner_psid.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing descriptor owner",
        ));
    }

    let owner_sid = {
        #[allow(unsafe_code)]
        unsafe {
            let len = GetLengthSid(owner_psid) as usize;
            std::slice::from_raw_parts(owner_psid.cast::<u8>(), len).to_vec()
        }
    };

    let mut control: u16 = 0;
    let mut revision: u32 = 0;
    let get_ctrl_ok = {
        #[allow(unsafe_code)]
        unsafe {
            GetSecurityDescriptorControl(sd_ptr.cast_mut().cast(), &mut control, &mut revision)
        }
    };
    if get_ctrl_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut dacl_present = 0i32;
    let mut dacl_ptr: *mut ACL = std::ptr::null_mut();
    let mut dacl_defaulted = 0i32;
    let get_dacl_ok = {
        #[allow(unsafe_code)]
        unsafe {
            GetSecurityDescriptorDacl(
                sd_ptr.cast_mut().cast(),
                &mut dacl_present,
                &mut dacl_ptr,
                &mut dacl_defaulted,
            )
        }
    };
    if get_dacl_ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let dacl_state = if dacl_present == 0 {
        DaclState::Absent
    } else if dacl_ptr.is_null() {
        DaclState::Null
    } else {
        #[allow(unsafe_code)]
        let count = unsafe { (*dacl_ptr).AceCount };
        let mut aces = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut ace_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let get_ace_ok = {
                #[allow(unsafe_code)]
                unsafe {
                    GetAce(dacl_ptr, i as u32, &mut ace_ptr)
                }
            };
            if get_ace_ok == 0 || ace_ptr.is_null() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "GetAce failed"));
            }

            #[allow(unsafe_code)]
            unsafe {
                let ace = ace_ptr.cast::<ACCESS_ALLOWED_ACE>();
                let ace_type = (*ace).Header.AceType;
                let ace_flags = (*ace).Header.AceFlags;
                let mask = (*ace).Mask;
                let sid_ptr = (&(*ace).SidStart as *const u32).cast::<std::ffi::c_void>();
                let sid_len = GetLengthSid(sid_ptr as PSID) as usize;
                let sid = std::slice::from_raw_parts(sid_ptr.cast::<u8>(), sid_len).to_vec();
                aces.push(ParsedAce {
                    ace_type,
                    ace_flags,
                    mask,
                    sid,
                });
            }
        }
        DaclState::Present(aces)
    };

    Ok(ParsedDescriptor {
        owner_sid,
        owner_defaulted: owner_defaulted != 0,
        dacl: dacl_state,
        dacl_defaulted: dacl_defaulted != 0,
        dacl_auto_inherited: (control & crate::private_descriptor::SE_DACL_AUTO_INHERITED) != 0,
        dacl_protected: (control & SE_DACL_PROTECTED) != 0,
        self_relative: (control & SE_SELF_RELATIVE) != 0,
        raw_control: control,
    })
}

#[cfg(any(test, feature = "test-hooks"))]
pub mod test_hooks {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static WRITE_FAULT: RefCell<bool> = const { RefCell::new(false) };
        static PRE_COMMIT_FLUSH_FAULT: RefCell<bool> = const { RefCell::new(false) };
        static POST_COMMIT_FLUSH_FAULT: RefCell<bool> = const { RefCell::new(false) };
        static DESTINATION_BARRIER: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static DESCRIPTOR_BARRIER: RefCell<Option<Box<dyn FnOnce(&Path)>>> = const { RefCell::new(None) };
        static READ_BARRIER: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static LOCK_BARRIER: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static CLEANUP_FAULT: RefCell<bool> = const { RefCell::new(false) };
    }

    pub(crate) fn write_fault_active() -> bool {
        WRITE_FAULT.with(|f| *f.borrow())
    }

    pub(crate) fn pre_commit_flush_fault_active() -> bool {
        PRE_COMMIT_FLUSH_FAULT.with(|f| *f.borrow())
    }

    pub(crate) fn post_commit_flush_fault_active() -> bool {
        POST_COMMIT_FLUSH_FAULT.with(|f| *f.borrow())
    }

    pub(crate) fn cleanup_fault_active() -> bool {
        CLEANUP_FAULT.with(|f| *f.borrow())
    }

    pub(crate) fn trigger_destination_barrier() {
        DESTINATION_BARRIER.with(|b| {
            if let Some(cb) = b.borrow_mut().take() {
                cb();
            }
        });
    }

    pub(crate) fn trigger_descriptor_barrier(path: &Path) {
        DESCRIPTOR_BARRIER.with(|b| {
            if let Some(cb) = b.borrow_mut().take() {
                cb(path);
            }
        });
    }

    pub(crate) fn trigger_read_barrier() {
        READ_BARRIER.with(|b| {
            if let Some(cb) = b.borrow_mut().take() {
                cb();
            }
        });
    }

    pub(crate) fn trigger_lock_barrier() {
        LOCK_BARRIER.with(|b| {
            if let Some(cb) = b.borrow_mut().take() {
                cb();
            }
        });
    }

    pub fn run_with_private_write_fault<T>(f: impl FnOnce() -> T) -> T {
        WRITE_FAULT.with(|flag| *flag.borrow_mut() = true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                WRITE_FAULT.with(|flag| *flag.borrow_mut() = false);
            }
        }
        let _reset = Reset;
        f()
    }

    pub fn run_with_private_pre_commit_flush_fault<T>(f: impl FnOnce() -> T) -> T {
        PRE_COMMIT_FLUSH_FAULT.with(|flag| *flag.borrow_mut() = true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                PRE_COMMIT_FLUSH_FAULT.with(|flag| *flag.borrow_mut() = false);
            }
        }
        let _reset = Reset;
        f()
    }

    pub fn run_with_private_post_commit_flush_fault<T>(f: impl FnOnce() -> T) -> T {
        POST_COMMIT_FLUSH_FAULT.with(|flag| *flag.borrow_mut() = true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                POST_COMMIT_FLUSH_FAULT.with(|flag| *flag.borrow_mut() = false);
            }
        }
        let _reset = Reset;
        f()
    }

    pub fn run_with_private_destination_barrier<T>(
        barrier: impl FnOnce() + 'static,
        f: impl FnOnce() -> T,
    ) -> T {
        DESTINATION_BARRIER.with(|b| *b.borrow_mut() = Some(Box::new(barrier)));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                DESTINATION_BARRIER.with(|b| {
                    b.borrow_mut().take();
                });
            }
        }
        let _reset = Reset;
        f()
    }

    pub fn run_with_private_descriptor_barrier<T>(
        barrier: impl FnOnce(&Path) + 'static,
        f: impl FnOnce() -> T,
    ) -> T {
        DESCRIPTOR_BARRIER.with(|b| *b.borrow_mut() = Some(Box::new(barrier)));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                DESCRIPTOR_BARRIER.with(|b| {
                    b.borrow_mut().take();
                });
            }
        }
        let _reset = Reset;
        f()
    }

    pub fn run_with_private_read_barrier<T>(
        barrier: impl FnOnce() + 'static,
        f: impl FnOnce() -> T,
    ) -> T {
        READ_BARRIER.with(|b| *b.borrow_mut() = Some(Box::new(barrier)));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                READ_BARRIER.with(|b| {
                    b.borrow_mut().take();
                });
            }
        }
        let _reset = Reset;
        f()
    }

    pub fn run_with_private_lock_barrier<T>(
        barrier: impl FnOnce() + 'static,
        f: impl FnOnce() -> T,
    ) -> T {
        LOCK_BARRIER.with(|b| *b.borrow_mut() = Some(Box::new(barrier)));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                LOCK_BARRIER.with(|b| {
                    b.borrow_mut().take();
                });
            }
        }
        let _reset = Reset;
        f()
    }

    pub fn run_with_private_cleanup_fault<T>(f: impl FnOnce() -> T) -> T {
        CLEANUP_FAULT.with(|flag| *flag.borrow_mut() = true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                CLEANUP_FAULT.with(|flag| *flag.borrow_mut() = false);
            }
        }
        let _reset = Reset;
        f()
    }
}
