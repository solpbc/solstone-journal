// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};

use windows_sys::Win32::Foundation::{ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::CloudFilters::{
    CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC, CfGetSyncRootInfoByPath,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo, FileIdInfo,
    GetDriveTypeW, GetFileInformationByHandleEx, GetVolumeInformationByHandleW, OPEN_EXISTING,
};
use windows_sys::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};

use super::backend::Backend;
use super::{JournalRootError, ObjectIdentity, WindowsRefusalCategory};
use crate::check_portable_component;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsAcquisitionPrimitive {
    RequestedRootOpen,
    RequestedRootAttributeTag,
    RequestedRootFileId,
    VerificationAncestorOpen,
    VerificationAncestorAttributeTag,
    FinalTargetOpen,
    FinalTargetAttributeTag,
    FinalTargetFileId,
    RootSelfFirstOpen,
    RootSelfFirstAttributeTag,
    RootSelfFirstFileId,
    RootSelfSecondOpen,
    RootSelfSecondAttributeTag,
    RootSelfSecondFileId,
    VolumeInformationByHandle,
    DriveType,
    CloudSyncRootInfo,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy)]
pub(crate) struct InjectedFault {
    pub(crate) primitive: WindowsAcquisitionPrimitive,
    pub(crate) ordinal: usize,
    pub(crate) raw_error: i32,
}

#[cfg(any(test, feature = "test-hooks"))]
struct TraceState {
    successful: Vec<WindowsAcquisitionPrimitive>,
    attempted: Vec<WindowsAcquisitionPrimitive>,
    barrier: Option<(usize, Box<dyn FnOnce()>)>,
    #[cfg(test)]
    barrier_fired: bool,
    fault: Option<InjectedFault>,
    fault_consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) struct TraceOutcome {
    #[cfg(test)]
    pub(crate) successful: Vec<WindowsAcquisitionPrimitive>,
    #[cfg(test)]
    pub(crate) attempted: Vec<WindowsAcquisitionPrimitive>,
    #[cfg(test)]
    pub(crate) barrier_fired: bool,
    pub(crate) fault_consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static ACQUISITION_TRACE: std::cell::RefCell<Option<TraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
thread_local! {
    static FILE_ID_SUBSTITUTION: std::cell::RefCell<Option<FileIdSubstitution>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
struct FileIdSubstitution {
    ordinal: usize,
    seen: usize,
    consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
struct TraceGuard;

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for TraceGuard {
    fn drop(&mut self) {
        ACQUISITION_TRACE.with(|trace| {
            trace.borrow_mut().take();
        });
    }
}

#[cfg(feature = "test-hooks")]
pub fn run_with_windows_acquisition_fault<T>(
    primitive: WindowsAcquisitionPrimitive,
    ordinal: usize,
    raw_error: i32,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, outcome) = trace_scenario(
        None,
        Some(InjectedFault {
            primitive,
            ordinal,
            raw_error,
        }),
        operation,
    );
    (result, outcome.fault_consumed)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn trace_scenario<T>(
    barrier: Option<(usize, Box<dyn FnOnce()>)>,
    fault: Option<InjectedFault>,
    operation: impl FnOnce() -> T,
) -> (T, TraceOutcome) {
    ACQUISITION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows acquisition trace is already active"
        );
        *trace.borrow_mut() = Some(TraceState {
            successful: Vec::new(),
            attempted: Vec::new(),
            barrier,
            #[cfg(test)]
            barrier_fired: false,
            fault,
            fault_consumed: false,
        });
    });
    let guard = TraceGuard;
    let result = operation();
    let state = ACQUISITION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Windows acquisition trace remains active")
    });
    drop(guard);
    (
        result,
        TraceOutcome {
            #[cfg(test)]
            successful: state.successful,
            #[cfg(test)]
            attempted: state.attempted,
            #[cfg(test)]
            barrier_fired: state.barrier_fired,
            fault_consumed: state.fault_consumed,
        },
    )
}

#[cfg(any(test, feature = "test-hooks"))]
fn attempt_acquisition(primitive: WindowsAcquisitionPrimitive) -> io::Result<()> {
    ACQUISITION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return Ok(());
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|attempted| **attempted == primitive)
            .count();
        if state
            .fault
            .is_some_and(|fault| fault.primitive == primitive && fault.ordinal == ordinal)
        {
            let fault = state.fault.take().expect("matching injected Windows fault");
            state.fault_consumed = true;
            return Err(io::Error::from_raw_os_error(fault.raw_error));
        }
        Ok(())
    })
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_success(primitive: WindowsAcquisitionPrimitive) {
    let callback = ACQUISITION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.successful.push(primitive);
        (state.barrier.as_ref().map(|(position, _)| *position) == Some(state.successful.len()))
            .then(|| {
                #[cfg(test)]
                {
                    state.barrier_fired = true;
                }
                state
                    .barrier
                    .take()
                    .expect("pending Windows acquisition barrier")
                    .1
            })
    });
    if let Some(callback) = callback {
        callback();
    }
}

fn traced_win32<T>(
    primitive: WindowsAcquisitionPrimitive,
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = primitive;
    #[cfg(any(test, feature = "test-hooks"))]
    attempt_acquisition(primitive)?;
    let result = operation();
    #[cfg(any(test, feature = "test-hooks"))]
    if result.is_ok() {
        record_success(primitive);
    }
    result
}

#[cfg(test)]
fn with_file_id_identity_mismatch<T>(ordinal: usize, operation: impl FnOnce() -> T) -> (T, bool) {
    FILE_ID_SUBSTITUTION.with(|substitution| {
        assert!(
            substitution.borrow().is_none(),
            "Windows file-ID substitution is already active"
        );
        *substitution.borrow_mut() = Some(FileIdSubstitution {
            ordinal,
            seen: 0,
            consumed: false,
        });
    });
    struct SubstitutionGuard;
    impl Drop for SubstitutionGuard {
        fn drop(&mut self) {
            FILE_ID_SUBSTITUTION.with(|substitution| {
                substitution.borrow_mut().take();
            });
        }
    }
    let guard = SubstitutionGuard;
    let result = operation();
    let state = FILE_ID_SUBSTITUTION.with(|substitution| {
        substitution
            .borrow_mut()
            .take()
            .expect("Windows file-ID substitution remains active")
    });
    drop(guard);
    (result, state.consumed)
}

#[cfg(test)]
fn substitute_file_id(info: &mut FILE_ID_INFO) {
    FILE_ID_SUBSTITUTION.with(|substitution| {
        let mut substitution = substitution.borrow_mut();
        let Some(state) = substitution.as_mut() else {
            return;
        };
        state.seen += 1;
        if state.seen == state.ordinal {
            info.FileId.Identifier[0] ^= 1;
            state.consumed = true;
        }
    });
}

pub(crate) struct WindowsRoot {
    root: OwnedHandle,
    canonical: PathBuf,
    identity: ObjectIdentity,
}

impl AsHandle for WindowsRoot {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.root.as_handle()
    }
}

impl Backend for WindowsRoot {
    fn identity(&self) -> ObjectIdentity {
        self.identity
    }

    fn diagnostic_path(&self) -> &Path {
        &self.canonical
    }

    fn revalidate(&self) -> Result<(), JournalRootError> {
        let attributes = attribute_tag(
            &self.root,
            WindowsAcquisitionPrimitive::RequestedRootAttributeTag,
        )
        .map_err(|source| {
            source_io(
                "query retained journal root attributes",
                &self.canonical,
                source,
            )
        })?;
        if !is_directory(attributes) || is_reparse_point(attributes) {
            return Err(JournalRootError::Changed);
        }
        let identity = file_id(&self.root, WindowsAcquisitionPrimitive::RequestedRootFileId)
            .map_err(|source| {
                source_io(
                    "query retained journal root identity",
                    &self.canonical,
                    source,
                )
            })?;
        (identity == self.identity)
            .then_some(())
            .ok_or(JournalRootError::Changed)
    }
}

#[derive(Debug)]
struct ValidatedPath {
    canonical: PathBuf,
    wide: Vec<u16>,
    components: Vec<OsString>,
}

/// Acquire one retained Windows directory handle.
///
/// Identity and revalidation use only the retained handle; the identical validated
/// path spelling is used solely for drive and Cloud Files classification metadata.
/// See JOURNAL_FILESYSTEM_CONTRACT.md for the authority distinction.
pub(crate) fn acquire(root: &Path) -> Result<WindowsRoot, JournalRootError> {
    let validated = validate_path(root)?;
    let authoritative = open_directory(
        &validated.wide,
        WindowsAcquisitionPrimitive::RequestedRootOpen,
    )
    .map_err(|source| source_io("open journal root", root, source))?;
    let attributes = attribute_tag(
        &authoritative,
        WindowsAcquisitionPrimitive::RequestedRootAttributeTag,
    )
    .map_err(|source| source_io("query journal root attributes", root, source))?;
    require_authoritative_directory(root, attributes)?;
    let expected = file_id(
        &authoritative,
        WindowsAcquisitionPrimitive::RequestedRootFileId,
    )
    .map_err(|source| source_io("query journal root identity", root, source))?;

    if validated.components.is_empty() {
        verify_root_self(root, &validated.wide, expected)?;
    } else {
        verify_ancestors_and_target(root, &validated, expected)?;
    }

    classify_filesystem(root, &authoritative)?;
    classify_drive(root, &root_drive_prefix(&validated.wide))?;
    classify_cloud_sync_root(root, &validated.wide)?;

    Ok(WindowsRoot {
        root: authoritative,
        canonical: validated.canonical,
        identity: expected,
    })
}

fn validate_path(root: &Path) -> Result<ValidatedPath, JournalRootError> {
    let mut disk = None;
    let mut saw_root = false;
    let mut components = Vec::new();
    for component in root.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(letter) => disk = Some(letter),
                _ => {
                    return Err(invalid(
                        root,
                        "journal root must use a fully qualified drive path",
                        WindowsRefusalCategory::NonFullyQualifiedNamespace,
                    ));
                }
            },
            Component::RootDir => saw_root = true,
            Component::Normal(name) => components.push(name.to_os_string()),
            Component::CurDir | Component::ParentDir => {
                return Err(invalid(
                    root,
                    "journal root must be lexically canonical",
                    WindowsRefusalCategory::NonCanonicalPath,
                ));
            }
        }
    }
    if disk.is_none() || !saw_root {
        return Err(invalid(
            root,
            "journal root must be absolute",
            WindowsRefusalCategory::NonAbsolutePath,
        ));
    }
    let reconstructed = reconstruct_path(disk.expect("disk prefix present"), &components);
    let original: Vec<u16> = root.as_os_str().encode_wide().collect();
    if original != reconstructed[..reconstructed.len() - 1] {
        return Err(invalid(
            root,
            "journal root must be lexically canonical",
            WindowsRefusalCategory::NonCanonicalPath,
        ));
    }
    if original.contains(&0) {
        return Err(invalid(
            root,
            "journal root contains an interior NUL",
            WindowsRefusalCategory::NonCanonicalPath,
        ));
    }
    if let Some(name) = components.last() {
        let Some(name) = name.to_str() else {
            return Err(invalid(
                root,
                "journal root has an invalid portable name",
                WindowsRefusalCategory::InvalidJournalName,
            ));
        };
        if check_portable_component(name).is_err() {
            return Err(invalid(
                root,
                "journal root has an invalid portable name",
                WindowsRefusalCategory::InvalidJournalName,
            ));
        }
    }
    Ok(ValidatedPath {
        canonical: root.to_path_buf(),
        wide: reconstructed,
        components,
    })
}

fn reconstruct_path(disk: u8, components: &[OsString]) -> Vec<u16> {
    let mut wide = vec![u16::from(disk), b':' as u16, b'\\' as u16];
    for (index, component) in components.iter().enumerate() {
        if index > 0 {
            wide.push(b'\\' as u16);
        }
        wide.extend(component.encode_wide());
    }
    wide.push(0);
    wide
}

fn verify_ancestors_and_target(
    root: &Path,
    validated: &ValidatedPath,
    expected: ObjectIdentity,
) -> Result<(), JournalRootError> {
    let mut level = root_drive_prefix(&validated.wide);
    let (target, ancestors) = validated
        .components
        .split_last()
        .expect("non-root path has final component");
    for ancestor in ancestors {
        append_component(&mut level, ancestor);
        let handle = open_directory(
            &level,
            WindowsAcquisitionPrimitive::VerificationAncestorOpen,
        )
        .map_err(|source| after_authority(root, "open journal root ancestor", source))?;
        let attributes = attribute_tag(
            &handle,
            WindowsAcquisitionPrimitive::VerificationAncestorAttributeTag,
        )
        .map_err(|source| {
            after_authority(root, "query journal root ancestor attributes", source)
        })?;
        require_verified_directory(root, attributes)?;
    }
    append_component(&mut level, target);
    let handle = open_directory(&level, WindowsAcquisitionPrimitive::FinalTargetOpen)
        .map_err(|source| after_authority(root, "open verified journal root", source))?;
    let attributes = attribute_tag(
        &handle,
        WindowsAcquisitionPrimitive::FinalTargetAttributeTag,
    )
    .map_err(|source| after_authority(root, "query verified journal root attributes", source))?;
    require_verified_directory(root, attributes)?;
    let observed = file_id(&handle, WindowsAcquisitionPrimitive::FinalTargetFileId)
        .map_err(|source| after_authority(root, "query verified journal root identity", source))?;
    (observed == expected)
        .then_some(())
        .ok_or(JournalRootError::Changed)
}

fn verify_root_self(
    root: &Path,
    wide: &[u16],
    expected: ObjectIdentity,
) -> Result<(), JournalRootError> {
    let first = verify_root_self_open(
        root,
        wide,
        WindowsAcquisitionPrimitive::RootSelfFirstOpen,
        WindowsAcquisitionPrimitive::RootSelfFirstAttributeTag,
        WindowsAcquisitionPrimitive::RootSelfFirstFileId,
    )?;
    let second = verify_root_self_open(
        root,
        wide,
        WindowsAcquisitionPrimitive::RootSelfSecondOpen,
        WindowsAcquisitionPrimitive::RootSelfSecondAttributeTag,
        WindowsAcquisitionPrimitive::RootSelfSecondFileId,
    )?;
    (first == expected && second == first)
        .then_some(())
        .ok_or(JournalRootError::Changed)
}

fn verify_root_self_open(
    root: &Path,
    wide: &[u16],
    open_primitive: WindowsAcquisitionPrimitive,
    attributes_primitive: WindowsAcquisitionPrimitive,
    identity_primitive: WindowsAcquisitionPrimitive,
) -> Result<ObjectIdentity, JournalRootError> {
    let handle = open_directory(wide, open_primitive)
        .map_err(|source| after_authority(root, "open verified drive root", source))?;
    let attributes = attribute_tag(&handle, attributes_primitive)
        .map_err(|source| after_authority(root, "query verified drive root attributes", source))?;
    require_verified_directory(root, attributes)?;
    file_id(&handle, identity_primitive)
        .map_err(|source| after_authority(root, "query verified drive root identity", source))
}

fn root_drive_prefix(wide: &[u16]) -> Vec<u16> {
    vec![wide[0], wide[1], wide[2], 0]
}

fn append_component(path: &mut Vec<u16>, component: &OsString) {
    path.pop();
    if path.last() != Some(&(b'\\' as u16)) {
        path.push(b'\\' as u16);
    }
    path.extend(component.encode_wide());
    path.push(0);
}

fn open_directory(wide: &[u16], primitive: WindowsAcquisitionPrimitive) -> io::Result<OwnedHandle> {
    let handle = traced_win32(primitive, || {
        // SAFETY: `wide` is NUL-terminated and the remaining parameters are documented constants for `CreateFileW`.
        #[allow(unsafe_code)]
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        (handle != INVALID_HANDLE_VALUE)
            .then_some(handle)
            .ok_or_else(io::Error::last_os_error)
    })?;
    // SAFETY: `CreateFileW` returned a non-invalid owned handle exactly once.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    Ok(handle)
}

fn attribute_tag(
    handle: &OwnedHandle,
    primitive: WindowsAcquisitionPrimitive,
) -> io::Result<FILE_ATTRIBUTE_TAG_INFO> {
    traced_win32(primitive, || {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: `info` is writable for its exact buffer size and the retained handle is valid for `GetFileInformationByHandleEx`.
        #[allow(unsafe_code)]
        let result = unsafe {
            GetFileInformationByHandleEx(
                handle.as_raw_handle(),
                FileAttributeTagInfo,
                (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        (result != 0)
            .then_some(info)
            .ok_or_else(io::Error::last_os_error)
    })
}

fn file_id(
    handle: &OwnedHandle,
    primitive: WindowsAcquisitionPrimitive,
) -> io::Result<ObjectIdentity> {
    traced_win32(primitive, || {
        let mut info = FILE_ID_INFO::default();
        // SAFETY: `info` is writable for its exact buffer size and the retained handle is valid for `GetFileInformationByHandleEx`.
        #[allow(unsafe_code)]
        let result = unsafe {
            GetFileInformationByHandleEx(
                handle.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        };
        #[cfg(test)]
        substitute_file_id(&mut info);
        (result != 0)
            .then_some(ObjectIdentity::from_volume_and_file_id(
                info.VolumeSerialNumber,
                info.FileId.Identifier,
            ))
            .ok_or_else(io::Error::last_os_error)
    })
}

fn classify_filesystem(root: &Path, handle: &OwnedHandle) -> Result<(), JournalRootError> {
    let name = traced_win32(
        WindowsAcquisitionPrimitive::VolumeInformationByHandle,
        || {
            let mut volume_name = [0u16; 256];
            let mut filesystem_name = [0u16; 256];
            let mut serial = 0;
            let mut maximum_component_length = 0;
            let mut flags = 0;
            // SAFETY: both UTF-16 buffers are writable for their exact supplied lengths for `GetVolumeInformationByHandleW`.
            #[allow(unsafe_code)]
            let result = unsafe {
                GetVolumeInformationByHandleW(
                    handle.as_raw_handle(),
                    volume_name.as_mut_ptr(),
                    volume_name.len() as u32,
                    &mut serial,
                    &mut maximum_component_length,
                    &mut flags,
                    filesystem_name.as_mut_ptr(),
                    filesystem_name.len() as u32,
                )
            };
            if result == 0 {
                return Err(io::Error::last_os_error());
            }
            let terminator = filesystem_name
                .iter()
                .position(|unit| *unit == 0)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "filesystem name is not NUL-terminated",
                    )
                })?;
            String::from_utf16(&filesystem_name[..terminator]).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "filesystem name is not UTF-16")
            })
        },
    )
    .map_err(|_| {
        unsupported(
            root,
            "filesystem type could not be verified",
            WindowsRefusalCategory::UnsupportedFilesystem,
        )
    })?;
    classify_filesystem_name(root, &name)
}

fn classify_filesystem_name(root: &Path, name: &str) -> Result<(), JournalRootError> {
    match name {
        "NTFS" | "ReFS" => Ok(()),
        _ => Err(unsupported(
            root,
            "filesystem is not NTFS or ReFS",
            WindowsRefusalCategory::UnsupportedFilesystem,
        )),
    }
}

fn classify_drive(root: &Path, wide: &[u16]) -> Result<(), JournalRootError> {
    let drive_type = traced_win32(WindowsAcquisitionPrimitive::DriveType, || {
        // SAFETY: `wide` is the validated, NUL-terminated drive root passed to `GetDriveTypeW`.
        #[allow(unsafe_code)]
        let value = unsafe { GetDriveTypeW(wide.as_ptr()) };
        Ok(value)
    })
    .map_err(|source| source_io("query journal root drive type", root, source))?;
    classify_drive_type(root, drive_type)
}

fn classify_drive_type(root: &Path, drive_type: u32) -> Result<(), JournalRootError> {
    match drive_type {
        DRIVE_FIXED => Ok(()),
        DRIVE_REMOTE => Err(unsupported(
            root,
            "journal root is on a non-local volume",
            WindowsRefusalCategory::NonLocalVolume,
        )),
        DRIVE_REMOVABLE | DRIVE_CDROM => Err(unsupported(
            root,
            "journal root is on removable or optical media",
            WindowsRefusalCategory::RemovableOrOpticalVolume,
        )),
        DRIVE_RAMDISK => Err(unsupported(
            root,
            "journal root is on a RAM disk",
            WindowsRefusalCategory::RamDiskVolume,
        )),
        _ => Err(unsupported(
            root,
            "journal root volume type is unknown",
            WindowsRefusalCategory::UnknownVolumeType,
        )),
    }
}

fn classify_cloud_sync_root(root: &Path, wide: &[u16]) -> Result<(), JournalRootError> {
    let result = traced_win32(WindowsAcquisitionPrimitive::CloudSyncRootInfo, || {
        let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
        let mut returned = 0;
        // SAFETY: `wide` is NUL-terminated and `info` is writable for its exact size for `CfGetSyncRootInfoByPath`.
        #[allow(unsafe_code)]
        let result = unsafe {
            CfGetSyncRootInfoByPath(
                wide.as_ptr(),
                CF_SYNC_ROOT_INFO_BASIC,
                (&mut info as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
                size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
                &mut returned,
            )
        };
        Ok(result)
    })
    .map_err(|_| {
        unsupported(
            root,
            "Cloud Files sync-root status could not be verified",
            WindowsRefusalCategory::CloudSyncRootStatusUnverifiable,
        )
    })?;
    classify_cloud_sync_root_result(root, result)
}

fn classify_cloud_sync_root_result(root: &Path, result: i32) -> Result<(), JournalRootError> {
    match result {
        0 => Err(unsupported(
            root,
            "journal root is a Cloud Files sync root",
            WindowsRefusalCategory::CloudSyncRootRegistered,
        )),
        result if result == hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT) => Ok(()),
        _ => Err(unsupported(
            root,
            "Cloud Files sync-root status could not be verified",
            WindowsRefusalCategory::CloudSyncRootStatusUnverifiable,
        )),
    }
}

const fn hresult_from_win32(error: u32) -> i32 {
    (0x8007_0000 | (error & 0x0000_FFFF)) as i32
}

fn require_authoritative_directory(
    root: &Path,
    attributes: FILE_ATTRIBUTE_TAG_INFO,
) -> Result<(), JournalRootError> {
    if is_reparse_point(attributes) {
        return Err(unsupported(
            root,
            "journal root is a reparse point",
            WindowsRefusalCategory::ReparsePoint,
        ));
    }
    if !is_directory(attributes) {
        return Err(JournalRootError::Invalid {
            root: root.to_path_buf(),
            reason: "journal root is not a directory",
            category: None,
        });
    }
    Ok(())
}

fn require_verified_directory(
    root: &Path,
    attributes: FILE_ATTRIBUTE_TAG_INFO,
) -> Result<(), JournalRootError> {
    if is_reparse_point(attributes) {
        return Err(unsupported(
            root,
            "journal root contains a reparse point",
            WindowsRefusalCategory::ReparsePoint,
        ));
    }
    is_directory(attributes)
        .then_some(())
        .ok_or(JournalRootError::Changed)
}

fn is_directory(attributes: FILE_ATTRIBUTE_TAG_INFO) -> bool {
    attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn is_reparse_point(attributes: FILE_ATTRIBUTE_TAG_INFO) -> bool {
    attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn invalid(
    root: &Path,
    reason: &'static str,
    category: WindowsRefusalCategory,
) -> JournalRootError {
    JournalRootError::Invalid {
        root: root.to_path_buf(),
        reason,
        category: Some(category),
    }
}

fn unsupported(
    root: &Path,
    reason: &'static str,
    category: WindowsRefusalCategory,
) -> JournalRootError {
    JournalRootError::Unsupported {
        root: root.to_path_buf(),
        reason,
        category: Some(category),
    }
}

fn source_io(operation: &'static str, path: &Path, source: io::Error) -> JournalRootError {
    JournalRootError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

fn after_authority(root: &Path, operation: &'static str, source: io::Error) -> JournalRootError {
    if source.raw_os_error().is_some_and(is_race_error) {
        JournalRootError::Changed
    } else {
        source_io(operation, root, source)
    }
}

fn is_race_error(error: i32) -> bool {
    matches!(error as u32, 2 | 3 | 267)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JournalRoot;
    use crate::test_support::TempDir;
    use std::fs;
    use std::os::windows::fs::symlink_dir;
    use windows_sys::Win32::System::WindowsProgramming::{DRIVE_NO_ROOT_DIR, DRIVE_UNKNOWN};

    #[derive(Debug, Eq, PartialEq)]
    struct FixtureSnapshot {
        marker: Vec<u8>,
        children: Vec<OsString>,
        readonly: bool,
        identity: ObjectIdentity,
    }

    fn snapshot_fixture(path: &Path) -> FixtureSnapshot {
        let root = JournalRoot::open(path).expect("fixture must be an admitted journal root");
        let mut children = fs::read_dir(path)
            .expect("list fixture")
            .map(|entry| entry.expect("fixture entry").file_name())
            .collect::<Vec<_>>();
        children.sort();
        FixtureSnapshot {
            marker: fs::read(path.join("marker")).expect("read fixture marker"),
            children,
            readonly: fs::metadata(path)
                .expect("fixture metadata")
                .permissions()
                .readonly(),
            identity: root.identity(),
        }
    }

    fn fixture(name: &str) -> (TempDir, PathBuf) {
        let temporary = TempDir::new();
        let root = temporary.path().join(name);
        fs::create_dir_all(root.join("child")).expect("create fixture children");
        fs::write(root.join("marker"), b"journal fixture").expect("write fixture marker");
        (temporary, root)
    }

    fn nested_fixture(name: &str) -> (TempDir, PathBuf) {
        let temporary = TempDir::new();
        let root = temporary.path().join("outer").join("inner").join(name);
        fs::create_dir_all(root.join("child")).expect("create fixture children");
        fs::write(root.join("marker"), b"journal fixture").expect("write fixture marker");
        (temporary, root)
    }

    fn unsupported_category(result: Result<(), JournalRootError>) -> WindowsRefusalCategory {
        match result {
            Err(JournalRootError::Unsupported {
                category: Some(category),
                ..
            }) => category,
            result => panic!("expected categorized unsupported refusal, got {result:?}"),
        }
    }

    fn rejection(path: &str, category: WindowsRefusalCategory) {
        let (result, outcome) = trace_scenario(None, None, || validate_path(Path::new(path)));
        let error = result.expect_err("root form must be rejected");
        assert!(
            matches!(error, JournalRootError::Invalid { category: Some(actual), .. } if actual == category)
        );
        assert!(outcome.attempted.is_empty());
    }

    #[test]
    fn lexical_rejections_make_no_win32_calls() {
        rejection("journal", WindowsRefusalCategory::NonAbsolutePath);
        rejection("C:journal", WindowsRefusalCategory::NonAbsolutePath);
        rejection("C:\\\\journal", WindowsRefusalCategory::NonCanonicalPath);
        rejection("C:/journal", WindowsRefusalCategory::NonCanonicalPath);
        rejection(
            "C:\\journal\\..\\other",
            WindowsRefusalCategory::NonCanonicalPath,
        );
        rejection(
            "\\\\server\\share\\journal",
            WindowsRefusalCategory::NonFullyQualifiedNamespace,
        );
        rejection(
            "\\\\?\\C:\\journal",
            WindowsRefusalCategory::NonFullyQualifiedNamespace,
        );
        rejection("C:\\CON", WindowsRefusalCategory::InvalidJournalName);
    }

    #[test]
    fn filesystem_classifier_accepts_only_ntfs_and_refs() {
        let root = Path::new(r"C:\journal");
        for name in ["NTFS", "ReFS"] {
            assert!(classify_filesystem_name(root, name).is_ok(), "{name}");
        }
        for name in ["FAT", "FAT32", "exFAT", ""] {
            assert_eq!(
                unsupported_category(classify_filesystem_name(root, name)),
                WindowsRefusalCategory::UnsupportedFilesystem,
                "{name}"
            );
        }
    }

    #[test]
    fn drive_classifier_assigns_each_refusal_category() {
        let root = Path::new(r"C:\journal");
        assert!(classify_drive_type(root, DRIVE_FIXED).is_ok());
        for (drive_type, category) in [
            (DRIVE_REMOTE, WindowsRefusalCategory::NonLocalVolume),
            (
                DRIVE_REMOVABLE,
                WindowsRefusalCategory::RemovableOrOpticalVolume,
            ),
            (
                DRIVE_CDROM,
                WindowsRefusalCategory::RemovableOrOpticalVolume,
            ),
            (DRIVE_RAMDISK, WindowsRefusalCategory::RamDiskVolume),
            (DRIVE_UNKNOWN, WindowsRefusalCategory::UnknownVolumeType),
            (DRIVE_NO_ROOT_DIR, WindowsRefusalCategory::UnknownVolumeType),
            (u32::MAX, WindowsRefusalCategory::UnknownVolumeType),
        ] {
            assert_eq!(
                unsupported_category(classify_drive_type(root, drive_type)),
                category,
                "drive type {drive_type}"
            );
        }
    }

    #[test]
    fn nested_path_admission_uses_a_drive_root_for_drive_classification() {
        let (_temporary, root_path) = nested_fixture("drive-root");
        let validated = validate_path(&root_path).expect("nested fixture path is valid");
        let drive_root = root_drive_prefix(&validated.wide);
        assert_eq!(
            drive_root,
            vec![validated.wide[0], b':' as u16, b'\\' as u16, 0]
        );
        assert_ne!(
            drive_root, validated.wide,
            "fixture is below the drive root"
        );

        let (result, outcome) = trace_scenario(None, None, || JournalRoot::open(&root_path));
        drop(result.expect("nested fixture is admitted from its fixed drive"));
        assert!(
            outcome
                .successful
                .contains(&WindowsAcquisitionPrimitive::DriveType),
            "nested admission reaches drive-type classification"
        );
    }

    #[test]
    fn cloud_sync_root_classifier_requires_verified_non_membership() {
        let root = Path::new(r"C:\journal");
        assert_eq!(
            unsupported_category(classify_cloud_sync_root_result(root, 0)),
            WindowsRefusalCategory::CloudSyncRootRegistered
        );
        assert!(
            classify_cloud_sync_root_result(
                root,
                hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT)
            )
            .is_ok()
        );
        assert_eq!(
            unsupported_category(classify_cloud_sync_root_result(root, 1)),
            WindowsRefusalCategory::CloudSyncRootStatusUnverifiable
        );
    }

    #[test]
    fn trace_records_barriers_and_one_shot_faults_without_a_filesystem_fixture() {
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let observer = std::rc::Rc::clone(&fired);
        let (result, outcome) = trace_scenario(
            Some((1, Box::new(move || observer.set(true)))),
            None,
            || traced_win32(WindowsAcquisitionPrimitive::DriveType, || Ok(())),
        );
        result.expect("synthetic traced operation");
        assert_eq!(outcome.successful, [WindowsAcquisitionPrimitive::DriveType]);
        assert!(outcome.barrier_fired);
        assert!(fired.get());

        let (result, outcome) = trace_scenario(
            None,
            Some(InjectedFault {
                primitive: WindowsAcquisitionPrimitive::DriveType,
                ordinal: 1,
                raw_error: 5,
            }),
            || traced_win32(WindowsAcquisitionPrimitive::DriveType, || Ok(())),
        );
        assert_eq!(result.unwrap_err().raw_os_error(), Some(5));
        assert!(outcome.fault_consumed);
    }

    #[test]
    fn every_acquisition_primitive_has_a_one_shot_fault_ordinal() {
        let primitives = [
            WindowsAcquisitionPrimitive::RequestedRootOpen,
            WindowsAcquisitionPrimitive::RequestedRootAttributeTag,
            WindowsAcquisitionPrimitive::RequestedRootFileId,
            WindowsAcquisitionPrimitive::VerificationAncestorOpen,
            WindowsAcquisitionPrimitive::VerificationAncestorAttributeTag,
            WindowsAcquisitionPrimitive::FinalTargetOpen,
            WindowsAcquisitionPrimitive::FinalTargetAttributeTag,
            WindowsAcquisitionPrimitive::FinalTargetFileId,
            WindowsAcquisitionPrimitive::RootSelfFirstOpen,
            WindowsAcquisitionPrimitive::RootSelfFirstAttributeTag,
            WindowsAcquisitionPrimitive::RootSelfFirstFileId,
            WindowsAcquisitionPrimitive::RootSelfSecondOpen,
            WindowsAcquisitionPrimitive::RootSelfSecondAttributeTag,
            WindowsAcquisitionPrimitive::RootSelfSecondFileId,
            WindowsAcquisitionPrimitive::VolumeInformationByHandle,
            WindowsAcquisitionPrimitive::DriveType,
            WindowsAcquisitionPrimitive::CloudSyncRootInfo,
        ];
        for primitive in primitives {
            let (result, outcome) = trace_scenario(
                None,
                Some(InjectedFault {
                    primitive,
                    ordinal: 1,
                    raw_error: 5,
                }),
                || traced_win32(primitive, || Ok(())),
            );
            assert_eq!(result.unwrap_err().raw_os_error(), Some(5));
            assert!(outcome.fault_consumed, "{primitive:?}");
        }
    }

    #[test]
    fn reparse_root_and_ancestor_are_refused_without_fixture_mutation() {
        let (temporary, target) = fixture("target");
        let before = snapshot_fixture(&target);
        let root_link = temporary.path().join("root-link");
        let ancestor_link = temporary.path().join("ancestor-link");
        if symlink_dir(&target, &root_link).is_err()
            || symlink_dir(&target, &ancestor_link).is_err()
        {
            eprintln!(
                "skipping reparse fixture test: symlink creation unavailable (no Developer Mode / elevated privilege)"
            );
            return;
        }
        let root_result = JournalRoot::open(&root_link);
        let ancestor_result = JournalRoot::open(&ancestor_link.join("child"));
        for result in [root_result, ancestor_result] {
            assert!(matches!(
                result,
                Err(JournalRootError::Unsupported {
                    category: Some(WindowsRefusalCategory::ReparsePoint),
                    ..
                })
            ));
        }
        assert_eq!(snapshot_fixture(&target), before);
    }

    #[test]
    fn retained_authority_survives_namespace_rename_and_fixture_restoration() {
        let (temporary, root_path) = fixture("journal");
        let before = snapshot_fixture(&root_path);
        let root = JournalRoot::open(&root_path).expect("admit fixture root");
        let moved = temporary.path().join("journal-moved");
        fs::rename(&root_path, &moved).expect("rename admitted namespace");
        root.revalidate()
            .expect("retained handle remains authoritative after rename");
        fs::rename(&moved, &root_path).expect("restore fixture namespace");
        assert_eq!(root.identity(), before.identity);
        assert_eq!(snapshot_fixture(&root_path), before);
    }

    #[test]
    fn final_identity_recheck_refuses_a_substituted_identity_without_mutation() {
        let (_temporary, root_path) = fixture("identity-mismatch");
        let before = snapshot_fixture(&root_path);
        let (result, consumed) =
            with_file_id_identity_mismatch(2, || JournalRoot::open(&root_path));
        assert!(consumed);
        assert!(matches!(result, Err(JournalRootError::Changed)));
        assert_eq!(snapshot_fixture(&root_path), before);
    }

    #[test]
    fn post_authoritative_open_barrier_detects_namespace_replacement() {
        let (temporary, root_path) = fixture("barrier");
        let before = snapshot_fixture(&root_path);
        let moved = temporary.path().join("barrier-moved");
        let callback_root = root_path.clone();
        let callback_moved = moved.clone();
        let (result, outcome) = trace_scenario(
            Some((
                1,
                Box::new(move || {
                    fs::rename(callback_root, callback_moved).expect("move root at barrier")
                }),
            )),
            None,
            || JournalRoot::open(&root_path),
        );
        assert!(outcome.barrier_fired);
        assert!(matches!(result, Err(JournalRootError::Changed)));
        fs::rename(&moved, &root_path).expect("restore barrier fixture");
        assert_eq!(snapshot_fixture(&root_path), before);
    }

    #[test]
    fn ancestor_attribute_barrier_detects_final_target_replacement() {
        let (temporary, root_path) = nested_fixture("journal");
        let before = snapshot_fixture(&root_path);
        let (baseline, trace) = trace_scenario(None, None, || JournalRoot::open(&root_path));
        drop(baseline.expect("admit nested fixture root"));
        let barrier_position = trace
            .successful
            .iter()
            .rposition(|primitive| {
                *primitive == WindowsAcquisitionPrimitive::VerificationAncestorAttributeTag
            })
            .map(|index| index + 1)
            .expect("nested fixture has an inspected ancestor");
        let moved = temporary.path().join("journal-moved");
        let callback_root = root_path.clone();
        let callback_moved = moved.clone();

        let (result, outcome) = trace_scenario(
            Some((
                barrier_position,
                Box::new(move || {
                    fs::rename(&callback_root, &callback_moved)
                        .expect("move final target after ancestor inspection");
                    fs::create_dir_all(callback_root.join("child"))
                        .expect("create replacement final target");
                    fs::write(callback_root.join("marker"), b"replacement marker")
                        .expect("write replacement marker");
                }),
            )),
            None,
            || JournalRoot::open(&root_path),
        );

        assert!(outcome.barrier_fired);
        assert_eq!(
            outcome.successful[barrier_position - 1],
            WindowsAcquisitionPrimitive::VerificationAncestorAttributeTag
        );
        assert!(matches!(result, Err(JournalRootError::Changed)));
        fs::remove_dir_all(&root_path).expect("remove replacement final target");
        fs::rename(&moved, &root_path).expect("restore original final target");
        assert_eq!(snapshot_fixture(&root_path), before);
    }

    #[test]
    fn real_ntfs_and_refs_controls_skip_without_environment() {
        for variable in ["JOURNAL_WIN_CI_NTFS_ROOT", "JOURNAL_WIN_CI_REFS_ROOT"] {
            let Ok(root) = std::env::var(variable) else {
                continue;
            };
            JournalRoot::open(Path::new(&root))
                .unwrap_or_else(|error| panic!("{variable} root must admit: {error}"));
        }
    }
}
