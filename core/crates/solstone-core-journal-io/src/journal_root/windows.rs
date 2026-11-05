// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT, ERROR_INVALID_FUNCTION, ERROR_NOT_A_CLOUD_FILE,
    ERROR_RETRY, GetLastError, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::CloudFilters::{
    CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC, CfGetSyncRootInfoByPath,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED, FILE_ID_INFO,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TRAVERSE, FileAttributeTagInfo, FileIdInfo, GetDriveTypeW,
    GetFileInformationByHandleEx, GetVolumeInformationByHandleW, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};

use super::backend::Backend;
use super::{JournalRootError, ObjectIdentity, WindowsRefusalCategory};
use crate::check_portable_component;

const FSCTL_CLOUD_SYNC_ROOT_QUERY: u32 = 0x0009_03BC;
const CLOUD_SYNC_ROOT_QUERY_TAG: u32 = 0x9000_001A;
const CLOUD_SYNC_ROOT_QUERY_BASIC: u32 = 0xC000_0005;
const CLOUD_SYNC_ROOT_QUERY_WORDS: usize = 38;
const CLOUD_SYNC_ROOT_RETRY_DELAYS_MS: [u64; 10] =
    [100, 400, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 1000];

static DOCUMENTED_QUERY_AGREED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsAcquisitionPrimitive {
    RequestedRootOpen,
    RequestedRootAttributeTag,
    RequestedRootFileId,
    RequestedRootWatchOpen,
    RequestedRootWatchAttributeTag,
    RequestedRootWatchFileId,
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
    CloudSyncRootInfoByHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectoryOpenPurpose {
    RetainedRoot,
    NamespaceWatch,
    Verification,
}

impl DirectoryOpenPurpose {
    const fn access_and_flags(self) -> (u32, u32) {
        match self {
            Self::RetainedRoot => (
                FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ),
            Self::NamespaceWatch => (
                FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OVERLAPPED,
            ),
            Self::Verification => (
                FILE_READ_ATTRIBUTES,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsFilesystemKind {
    Ntfs,
    Refs,
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
    pub(crate) successful: Vec<WindowsAcquisitionPrimitive>,
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
    static ATTRIBUTE_TAG_SUBSTITUTION: std::cell::RefCell<Option<AttributeTagSubstitution>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
struct FileIdSubstitution {
    ordinal: usize,
    seen: usize,
    consumed: bool,
}

#[cfg(test)]
struct AttributeTagSubstitution {
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
#[derive(Debug, Eq, PartialEq)]
pub struct WindowsAcquisitionTrace {
    pub attempted: Vec<WindowsAcquisitionPrimitive>,
    pub successful: Vec<WindowsAcquisitionPrimitive>,
    pub fault_consumed: bool,
}

#[cfg(feature = "test-hooks")]
pub fn run_with_windows_acquisition_trace<T>(
    operation: impl FnOnce() -> T,
) -> (T, WindowsAcquisitionTrace) {
    let (result, outcome) = trace_scenario(None, None, operation);
    (
        result,
        WindowsAcquisitionTrace {
            attempted: outcome.attempted,
            successful: outcome.successful,
            fault_consumed: outcome.fault_consumed,
        },
    )
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
            successful: state.successful,
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
fn with_reparse_attribute_substitution<T>(
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    ATTRIBUTE_TAG_SUBSTITUTION.with(|substitution| {
        assert!(
            substitution.borrow().is_none(),
            "Windows attribute-tag substitution is already active"
        );
        *substitution.borrow_mut() = Some(AttributeTagSubstitution {
            ordinal,
            seen: 0,
            consumed: false,
        });
    });
    struct SubstitutionGuard;
    impl Drop for SubstitutionGuard {
        fn drop(&mut self) {
            ATTRIBUTE_TAG_SUBSTITUTION.with(|substitution| {
                substitution.borrow_mut().take();
            });
        }
    }
    let guard = SubstitutionGuard;
    let result = operation();
    let state = ATTRIBUTE_TAG_SUBSTITUTION.with(|substitution| {
        substitution
            .borrow_mut()
            .take()
            .expect("Windows attribute-tag substitution remains active")
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

#[cfg(test)]
fn substitute_attribute_tag(info: &mut FILE_ATTRIBUTE_TAG_INFO) {
    ATTRIBUTE_TAG_SUBSTITUTION.with(|substitution| {
        let mut substitution = substitution.borrow_mut();
        let Some(state) = substitution.as_mut() else {
            return;
        };
        state.seen += 1;
        if state.seen == state.ordinal {
            info.FileAttributes |= FILE_ATTRIBUTE_REPARSE_POINT;
            state.consumed = true;
        }
    });
}

pub(crate) struct WindowsRoot {
    root: OwnedHandle,
    namespace_watch: OwnedHandle,
    canonical: PathBuf,
    identity: ObjectIdentity,
    filesystem: WindowsFilesystemKind,
}

impl AsHandle for WindowsRoot {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.root.as_handle()
    }
}

impl WindowsRoot {
    pub(crate) fn as_watch_handle(&self) -> BorrowedHandle<'_> {
        self.namespace_watch.as_handle()
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
            .ok_or(JournalRootError::Changed)?;
        if self.filesystem == WindowsFilesystemKind::Ntfs {
            classify_cloud_sync_root_by_handle(&self.canonical, &self.root)?;
        }
        Ok(())
    }

    fn revalidate_canonical_binding(&self) -> Result<(), JournalRootError> {
        let validated = validate_path(&self.canonical)?;
        if validated.components.is_empty() {
            let handle = open_directory(
                &validated.wide,
                WindowsAcquisitionPrimitive::FinalTargetOpen,
                DirectoryOpenPurpose::Verification,
            )
            .map_err(|source| {
                after_authority(&self.canonical, "open verified journal root", source)
            })?;
            let attributes = attribute_tag(
                &handle,
                WindowsAcquisitionPrimitive::FinalTargetAttributeTag,
            )
            .map_err(|source| {
                after_authority(
                    &self.canonical,
                    "query verified journal root attributes",
                    source,
                )
            })?;
            if !is_directory(attributes) || is_reparse_point(attributes) {
                return Err(JournalRootError::Changed);
            }
            let observed = file_id(&handle, WindowsAcquisitionPrimitive::FinalTargetFileId)
                .map_err(|source| {
                    after_authority(
                        &self.canonical,
                        "query verified journal root identity",
                        source,
                    )
                })?;
            return (observed == self.identity)
                .then_some(())
                .ok_or(JournalRootError::Changed);
        }
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
                DirectoryOpenPurpose::Verification,
            )
            .map_err(|source| {
                after_authority(&self.canonical, "open journal root ancestor", source)
            })?;
            let attributes = attribute_tag(
                &handle,
                WindowsAcquisitionPrimitive::VerificationAncestorAttributeTag,
            )
            .map_err(|source| {
                after_authority(
                    &self.canonical,
                    "query journal root ancestor attributes",
                    source,
                )
            })?;
            if !is_directory(attributes) || is_reparse_point(attributes) {
                return Err(JournalRootError::Changed);
            }
        }
        append_component(&mut level, target);
        let handle = open_directory(
            &level,
            WindowsAcquisitionPrimitive::FinalTargetOpen,
            DirectoryOpenPurpose::Verification,
        )
        .map_err(|source| after_authority(&self.canonical, "open verified journal root", source))?;
        let attributes = attribute_tag(
            &handle,
            WindowsAcquisitionPrimitive::FinalTargetAttributeTag,
        )
        .map_err(|source| {
            after_authority(
                &self.canonical,
                "query verified journal root attributes",
                source,
            )
        })?;
        if !is_directory(attributes) || is_reparse_point(attributes) {
            return Err(JournalRootError::Changed);
        }
        let observed =
            file_id(&handle, WindowsAcquisitionPrimitive::FinalTargetFileId).map_err(|source| {
                after_authority(
                    &self.canonical,
                    "query verified journal root identity",
                    source,
                )
            })?;
        (observed == self.identity)
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
        DirectoryOpenPurpose::RetainedRoot,
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
    let namespace_watch = open_directory(
        &validated.wide,
        WindowsAcquisitionPrimitive::RequestedRootWatchOpen,
        DirectoryOpenPurpose::NamespaceWatch,
    )
    .map_err(|source| after_authority(root, "open journal root namespace watch", source))?;
    let watch_attributes = attribute_tag(
        &namespace_watch,
        WindowsAcquisitionPrimitive::RequestedRootWatchAttributeTag,
    )
    .map_err(|source| {
        after_authority(
            root,
            "query journal root namespace-watch attributes",
            source,
        )
    })?;
    require_authoritative_directory(root, watch_attributes)?;
    let watch_identity = file_id(
        &namespace_watch,
        WindowsAcquisitionPrimitive::RequestedRootWatchFileId,
    )
    .map_err(|source| {
        after_authority(root, "query journal root namespace-watch identity", source)
    })?;
    (watch_identity == expected)
        .then_some(())
        .ok_or(JournalRootError::Changed)?;

    if validated.components.is_empty() {
        verify_root_self(root, &validated.wide, expected)?;
    } else {
        verify_ancestors_and_target(root, &validated, expected)?;
    }

    let filesystem = classify_filesystem(root, &authoritative)?;
    classify_drive(root, &root_drive_prefix(&validated.wide))?;
    admit_for_filesystem(root, &validated.wide, &authoritative, filesystem)?;

    Ok(WindowsRoot {
        root: authoritative,
        namespace_watch,
        canonical: validated.canonical,
        identity: expected,
        filesystem,
    })
}

fn validate_path(root: &Path) -> Result<ValidatedPath, JournalRootError> {
    let mut disk = None;
    let mut saw_root = false;
    let mut components = Vec::new();
    for component in root.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(letter) => disk = Some((letter, false)),
                Prefix::VerbatimDisk(letter) => disk = Some((letter, true)),
                _ => {
                    return Err(invalid(
                        root,
                        "journal root must use a fully qualified drive path",
                        WindowsRefusalCategory::NonFullyQualifiedNamespace,
                    ));
                }
            },
            Component::RootDir => saw_root = true,
            Component::Normal(name) => {
                // Verbatim paths treat an interior slash as a normal character.
                if name.encode_wide().any(|unit| unit == u16::from(b'/')) {
                    return Err(invalid(
                        root,
                        "journal root must be lexically canonical",
                        WindowsRefusalCategory::NonCanonicalPath,
                    ));
                }
                components.push(name.to_os_string());
            }
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
    let (letter, verbatim) = disk.expect("disk prefix present");
    let reconstructed = reconstruct_path(letter, verbatim, &components);
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

fn reconstruct_path(disk: u8, verbatim: bool, components: &[OsString]) -> Vec<u16> {
    let mut wide = if verbatim {
        vec![b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]
    } else {
        Vec::new()
    };
    wide.extend([u16::from(disk), b':' as u16, b'\\' as u16]);
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
            DirectoryOpenPurpose::Verification,
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
    let handle = open_directory(
        &level,
        WindowsAcquisitionPrimitive::FinalTargetOpen,
        DirectoryOpenPurpose::Verification,
    )
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
    let handle = open_directory(wide, open_primitive, DirectoryOpenPurpose::Verification)
        .map_err(|source| after_authority(root, "open verified drive root", source))?;
    let attributes = attribute_tag(&handle, attributes_primitive)
        .map_err(|source| after_authority(root, "query verified drive root attributes", source))?;
    require_verified_directory(root, attributes)?;
    file_id(&handle, identity_primitive)
        .map_err(|source| after_authority(root, "query verified drive root identity", source))
}

fn root_drive_prefix(wide: &[u16]) -> Vec<u16> {
    // Only exact reconstructed Disk/VerbatimDisk spellings reach this helper.
    let count = if wide.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]) {
        7
    } else {
        3
    };
    let mut prefix = wide[..count].to_vec();
    prefix.push(0);
    prefix
}

fn append_component(path: &mut Vec<u16>, component: &OsString) {
    path.pop();
    if path.last() != Some(&(b'\\' as u16)) {
        path.push(b'\\' as u16);
    }
    path.extend(component.encode_wide());
    path.push(0);
}

fn open_directory(
    wide: &[u16],
    primitive: WindowsAcquisitionPrimitive,
    purpose: DirectoryOpenPurpose,
) -> io::Result<OwnedHandle> {
    let (access, flags) = purpose.access_and_flags();
    let handle = traced_win32(primitive, || {
        // SAFETY: `wide` is NUL-terminated and the remaining parameters are documented constants for `CreateFileW`.
        #[allow(unsafe_code)]
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                flags,
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
        #[cfg(test)]
        substitute_attribute_tag(&mut info);
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

fn classify_filesystem(
    root: &Path,
    handle: &OwnedHandle,
) -> Result<WindowsFilesystemKind, JournalRootError> {
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

fn classify_filesystem_name(
    root: &Path,
    name: &str,
) -> Result<WindowsFilesystemKind, JournalRootError> {
    match name {
        "NTFS" => Ok(WindowsFilesystemKind::Ntfs),
        "ReFS" => Ok(WindowsFilesystemKind::Refs),
        _ => Err(unsupported(
            root,
            "filesystem is not NTFS or ReFS",
            WindowsRefusalCategory::UnsupportedFilesystem,
        )),
    }
}

fn admit_for_filesystem(
    root: &Path,
    wide: &[u16],
    handle: &OwnedHandle,
    filesystem: WindowsFilesystemKind,
) -> Result<(), JournalRootError> {
    match filesystem {
        WindowsFilesystemKind::Ntfs => {
            classify_cloud_sync_root_by_handle(root, handle)?;
            cross_check_with_documented_query(&DOCUMENTED_QUERY_AGREED, || {
                classify_cloud_sync_root(root, wide)
            })
        }
        WindowsFilesystemKind::Refs => Ok(()),
    }
}

/// Run `documented_query` until it first agrees that a root is outside Cloud Files.
///
/// The live check below depends on a Windows control with no published contract,
/// so each process also asks the documented API. A disagreement refuses and is
/// never remembered as agreement. One agreement ends the cross-check, because the
/// documented API costs the process a handle on every nonmember answer.
fn cross_check_with_documented_query(
    agreed: &AtomicBool,
    documented_query: impl FnOnce() -> Result<(), JournalRootError>,
) -> Result<(), JournalRootError> {
    if agreed.load(Ordering::Acquire) {
        return Ok(());
    }
    documented_query()?;
    agreed.store(true, Ordering::Release);
    Ok(())
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
        // `traced_win32` can only fail before this closure through test-hook fault injection.
        unsupported(
            root,
            "Cloud Files sync-root status could not be verified",
            WindowsRefusalCategory::CloudSyncRootStatusUnverifiable(None),
        )
    })?;
    classify_cloud_sync_root_result(root, result)
}

fn classify_cloud_sync_root_by_handle(
    root: &Path,
    handle: &OwnedHandle,
) -> Result<(), JournalRootError> {
    let result = traced_win32(
        WindowsAcquisitionPrimitive::CloudSyncRootInfoByHandle,
        || {
            Ok(retry_cloud_sync_root_query(
                || issue_cloud_sync_root_query(handle),
                std::thread::sleep,
            ))
        },
    )
    .map_err(|_| {
        unsupported(
            root,
            "Cloud Files sync-root status could not be verified",
            WindowsRefusalCategory::CloudSyncRootStatusUnverifiable(None),
        )
    })?;
    classify_cloud_sync_root_by_handle_result(root, result)
}

/// Ask the Cloud Files filter about a retained handle without the exported wrapper.
///
/// `CfGetSyncRootInfoByHandle` and `CfGetSyncRootInfoByPath` issue this same control,
/// then record the call in telemetry. On a failing result that telemetry opens the
/// current process and never closes the handle, and "not a sync root" is a failing
/// result, so an ordinary journal root would leak one handle per check. Both results
/// classify the same way; anything unexpected stays a refusal.
fn issue_cloud_sync_root_query(handle: &OwnedHandle) -> i32 {
    let request = cloud_sync_root_query_request();
    let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
    let mut returned = 0;
    // SAFETY: `handle` is open and synchronous, `request` and `info` are readable and
    // writable for their exact supplied sizes, and no OVERLAPPED is passed.
    #[allow(unsafe_code)]
    let succeeded = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            FSCTL_CLOUD_SYNC_ROOT_QUERY,
            request.as_ptr().cast(),
            size_of_val(&request) as u32,
            (&mut info as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if succeeded != 0 {
        return 0;
    }
    // SAFETY: reads the calling thread's last-error value immediately after the failure.
    #[allow(unsafe_code)]
    let error = unsafe { GetLastError() };
    hresult_from_win32(error)
}

const fn cloud_sync_root_query_request() -> [u32; CLOUD_SYNC_ROOT_QUERY_WORDS] {
    let mut request = [0; CLOUD_SYNC_ROOT_QUERY_WORDS];
    request[0] = CLOUD_SYNC_ROOT_QUERY_TAG;
    request[1] = CLOUD_SYNC_ROOT_QUERY_BASIC;
    request
}

/// The filter's documented wrappers retry `ERROR_RETRY` on this schedule; exhausting it
/// returns the retry error, which classifies as unverifiable.
fn retry_cloud_sync_root_query(
    mut query: impl FnMut() -> i32,
    mut sleep: impl FnMut(Duration),
) -> i32 {
    let mut result = query();
    for delay_ms in CLOUD_SYNC_ROOT_RETRY_DELAYS_MS {
        if result != hresult_from_win32(ERROR_RETRY) {
            break;
        }
        sleep(Duration::from_millis(delay_ms));
        result = query();
    }
    result
}

fn classify_cloud_sync_root_by_handle_result(
    root: &Path,
    result: i32,
) -> Result<(), JournalRootError> {
    classify_cloud_sync_root_result(root, result)
}

fn classify_cloud_sync_root_result(root: &Path, result: i32) -> Result<(), JournalRootError> {
    match result {
        0 => Err(unsupported(
            root,
            "journal root is a Cloud Files sync root",
            WindowsRefusalCategory::CloudSyncRootRegistered,
        )),
        result
            if result == hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT)
                || result == hresult_from_win32(ERROR_NOT_A_CLOUD_FILE)
                || result == hresult_from_win32(ERROR_INVALID_FUNCTION) =>
        {
            Ok(())
        }
        _ => Err(unsupported(
            root,
            "Cloud Files sync-root status could not be verified",
            WindowsRefusalCategory::CloudSyncRootStatusUnverifiable(Some(result)),
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

    fn unsupported_category<T: std::fmt::Debug>(
        result: Result<T, JournalRootError>,
    ) -> WindowsRefusalCategory {
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
        for path in [
            r"\\?\UNC\server\share\journal",
            r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\journal",
            r"\\.\C:\journal",
        ] {
            rejection(path, WindowsRefusalCategory::NonFullyQualifiedNamespace);
        }
        for path in [
            r"\\?\C:\outer/escape\journal",
            r"\\?\C:\outer\..\journal",
            r"\\?\C:\outer\.\journal",
            r"\\?\C:\\journal",
            "\\\\?\\C:\\journal\0hidden",
        ] {
            rejection(path, WindowsRefusalCategory::NonCanonicalPath);
        }
        rejection("C:\\CON", WindowsRefusalCategory::InvalidJournalName);
    }

    #[test]
    fn drive_root_spellings_are_preserved_exactly() {
        for (path, expected) in [
            (r"C:\outer\journal", vec![67, 58, 92, 0]),
            (r"\\?\C:\outer\journal", vec![92, 92, 63, 92, 67, 58, 92, 0]),
        ] {
            let (result, outcome) = trace_scenario(None, None, || validate_path(Path::new(path)));
            let validated = result.unwrap();
            let mut exact: Vec<u16> = path.encode_utf16().collect();
            exact.push(0);
            assert_eq!(validated.wide, exact);
            assert_eq!(validated.canonical, Path::new(path));
            assert_eq!(root_drive_prefix(&validated.wide), expected);
            assert!(outcome.attempted.is_empty());
        }
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

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn canonicalized_local_drive_root_preserves_native_binding() {
        let temporary = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
            Some(root) => tempfile::tempdir_in(root).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        let plain = temporary.path().join("outer").join("journal");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("marker"), b"owner bytes").unwrap();
        let canonical = fs::canonicalize(&plain).unwrap();
        assert!(matches!(
            plain.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
        ));
        assert!(matches!(
            canonical.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_))
        ));
        let original = JournalRoot::open(&plain).expect("plain local drive root admits");
        let retained =
            JournalRoot::open(&canonical).expect("canonicalized local drive root admits");
        assert_eq!(retained.canonical_path(), canonical);
        assert_eq!(original.identity(), retained.identity());
        original.revalidate().unwrap();
        original.revalidate_canonical_binding().unwrap();
        retained.revalidate().unwrap();
        retained.revalidate_canonical_binding().unwrap();
        assert_eq!(fs::read(plain.join("marker")).unwrap(), b"owner bytes");
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
    fn cloud_sync_root_classifier_classifies_results_and_preserves_diagnostics() {
        let root = Path::new(r"C:\journal");
        let access_denied = hresult_from_win32(5); // ERROR_ACCESS_DENIED
        let cases = [
            (
                0,
                Err((
                    WindowsRefusalCategory::CloudSyncRootRegistered,
                    Some(0),
                    "unsupported journal root C:\\journal: journal root is a Cloud Files sync root",
                )),
            ),
            (
                hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT),
                Ok(()),
            ),
            (hresult_from_win32(ERROR_INVALID_FUNCTION), Ok(())),
            (hresult_from_win32(ERROR_NOT_A_CLOUD_FILE), Ok(())),
            (
                access_denied,
                Err((
                    WindowsRefusalCategory::CloudSyncRootStatusUnverifiable(Some(access_denied)),
                    Some(access_denied),
                    "unsupported journal root C:\\journal: Cloud Files sync-root status could not be verified",
                )),
            ),
            (
                0x1234_5678_i32,
                Err((
                    WindowsRefusalCategory::CloudSyncRootStatusUnverifiable(Some(0x1234_5678)),
                    Some(0x1234_5678),
                    "unsupported journal root C:\\journal: Cloud Files sync-root status could not be verified",
                )),
            ),
        ];

        for (result, expected) in cases {
            let admission = classify_cloud_sync_root_result(root, result);
            match expected {
                Ok(()) => assert!(admission.is_ok(), "HRESULT {result:#010x}"),
                Err((expected_category, expected_hresult, expected_display)) => {
                    let error = admission.expect_err("HRESULT must be refused");
                    let category = match &error {
                        JournalRootError::Unsupported {
                            category: Some(category),
                            ..
                        } => *category,
                        error => panic!("expected categorized unsupported refusal, got {error:?}"),
                    };
                    assert_eq!(category, expected_category);
                    assert_eq!(category.raw_hresult(), expected_hresult);
                    assert_eq!(error.to_string(), expected_display);
                }
            }
        }
    }

    #[test]
    fn ntfs_admission_queries_cloud_sync_root_once() {
        let (_temporary, root_path) = fixture("cloud-sync-ntfs");
        let (result, outcome) = trace_scenario(None, None, || JournalRoot::open(&root_path));
        drop(result.expect("NTFS fixture is admitted"));
        assert_eq!(
            outcome
                .successful
                .iter()
                .filter(|primitive| {
                    **primitive == WindowsAcquisitionPrimitive::CloudSyncRootInfoByHandle
                })
                .count(),
            1
        );
        assert!(
            outcome
                .successful
                .iter()
                .filter(|primitive| **primitive == WindowsAcquisitionPrimitive::CloudSyncRootInfo)
                .count()
                <= 1
        );
    }

    #[test]
    fn ntfs_revalidation_queries_cloud_sync_root_by_retained_handle() {
        let (_temporary, root_path) = fixture("cloud-sync-revalidate");
        let root = JournalRoot::open(&root_path).expect("NTFS fixture is admitted");
        let (result, outcome) = trace_scenario(None, None, || root.revalidate());
        result.expect("retained NTFS root remains revalidatable");
        assert_eq!(
            outcome
                .successful
                .iter()
                .filter(|primitive| {
                    **primitive == WindowsAcquisitionPrimitive::CloudSyncRootInfoByHandle
                })
                .count(),
            1
        );
    }

    #[test]
    fn documented_query_cross_check_refuses_until_it_agrees_then_stops_asking() {
        let root = Path::new(r"C:\journal");
        let agreed = AtomicBool::new(false);

        let refused =
            cross_check_with_documented_query(&agreed, || classify_cloud_sync_root_result(root, 0));
        assert_eq!(
            unsupported_category(refused),
            WindowsRefusalCategory::CloudSyncRootRegistered
        );
        assert!(!agreed.load(Ordering::Acquire));

        cross_check_with_documented_query(&agreed, || {
            classify_cloud_sync_root_result(
                root,
                hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT),
            )
        })
        .expect("nonmember agreement passes");
        cross_check_with_documented_query(&agreed, || panic!("agreement ends the cross-check"))
            .expect("a later admission does not ask again");
    }

    #[test]
    fn cloud_sync_root_query_retries_only_the_retry_result_and_stays_bounded() {
        let retry = hresult_from_win32(ERROR_RETRY);
        let mut answers = [retry, retry, 0].into_iter();
        let mut delays = Vec::new();
        let result = retry_cloud_sync_root_query(
            || answers.next().expect("query answer"),
            |delay| delays.push(delay.as_millis()),
        );
        assert_eq!(result, 0);
        assert_eq!(delays, [100, 400]);

        let mut calls = 0;
        let mut sleeps = 0;
        let result = retry_cloud_sync_root_query(
            || {
                calls += 1;
                retry
            },
            |_| sleeps += 1,
        );
        assert_eq!(result, retry);
        assert_eq!((calls, sleeps), (11, 10));
        assert!(classify_cloud_sync_root_result(Path::new(r"C:\journal"), result).is_err());

        let mut calls = 0;
        let invalid = hresult_from_win32(ERROR_INVALID_FUNCTION);
        let result = retry_cloud_sync_root_query(
            || {
                calls += 1;
                invalid
            },
            |_| panic!("only the retry result waits"),
        );
        assert_eq!((result, calls), (invalid, 1));
    }

    #[test]
    fn cloud_sync_root_query_request_matches_the_control_layout() {
        let request = cloud_sync_root_query_request();
        assert_eq!(size_of_val(&request), 0x98);
        assert_eq!(request[0], 0x9000_001A);
        assert_eq!(request[1], 0xC000_0005);
        assert!(request[2..].iter().all(|word| *word == 0));
    }

    #[test]
    fn retained_handle_cloud_status_preserves_unclassified_hresult() {
        let root = Path::new(r"C:\journal");
        let result = classify_cloud_sync_root_by_handle_result(root, 0x1234_5678_i32);
        let error = result.expect_err("unclassified retained-handle status must be refused");
        assert!(matches!(
            error,
            JournalRootError::Unsupported {
                category: Some(WindowsRefusalCategory::CloudSyncRootStatusUnverifiable(
                    Some(0x1234_5678)
                )),
                ..
            }
        ));
    }

    #[test]
    fn refs_admission_skips_cloud_sync_root_query() {
        let (_temporary, root_path) = fixture("cloud-sync-refs");
        let handle = open_directory(
            &validate_path(&root_path).expect("valid fixture").wide,
            WindowsAcquisitionPrimitive::RequestedRootOpen,
            DirectoryOpenPurpose::Verification,
        )
        .expect("open fixture");
        let (result, outcome) = trace_scenario(None, None, || {
            admit_for_filesystem(&root_path, &[0u16], &handle, WindowsFilesystemKind::Refs)
        });
        result.expect("ReFS admission skips Cloud Files classification");
        assert!(outcome.attempted.is_empty());
    }

    #[test]
    fn unsupported_filesystem_name_refuses_before_cloud_sync_query() {
        let (result, outcome) = trace_scenario(None, None, || {
            classify_filesystem_name(Path::new(r"C:\journal"), "exFAT")
        });
        assert_eq!(
            unsupported_category(result),
            WindowsRefusalCategory::UnsupportedFilesystem
        );
        assert!(outcome.attempted.is_empty());
    }

    #[test]
    fn cloud_file_not_a_cloud_file_regression_requires_376_admission() {
        let root = Path::new(r"C:\journal");
        let not_a_cloud_file = hresult_from_win32(ERROR_NOT_A_CLOUD_FILE);
        let accepts_only_not_under_sync_root =
            |result| result == hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT);

        assert!(!accepts_only_not_under_sync_root(not_a_cloud_file));
        assert!(classify_cloud_sync_root_result(root, not_a_cloud_file).is_ok());
    }

    #[test]
    fn cloud_file_invalid_function_regression_requires_error_1_admission() {
        let root = Path::new(r"C:\journal");
        let invalid_function = hresult_from_win32(ERROR_INVALID_FUNCTION);
        let accepts_only_legacy_pair = |result| {
            result == hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT)
                || result == hresult_from_win32(ERROR_NOT_A_CLOUD_FILE)
        };

        assert!(!accepts_only_legacy_pair(invalid_function));
        assert!(classify_cloud_sync_root_result(root, invalid_function).is_ok());
    }

    #[test]
    fn retained_root_open_profile_requests_exact_access_and_flags() {
        let (access, flags) = DirectoryOpenPurpose::RetainedRoot.access_and_flags();
        assert_eq!(
            access,
            FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE
        );
        assert_eq!(
            flags,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
        );
    }

    #[test]
    fn namespace_watch_open_profile_requests_exact_access_and_flags() {
        let (access, flags) = DirectoryOpenPurpose::NamespaceWatch.access_and_flags();
        assert_eq!(
            access,
            FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE
        );
        assert_eq!(
            flags,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OVERLAPPED
        );
    }

    #[test]
    fn verification_directory_open_profile_remains_attribute_only() {
        let (access, flags) = DirectoryOpenPurpose::Verification.access_and_flags();
        assert_eq!(access, FILE_READ_ATTRIBUTES);
        assert_eq!(
            flags,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
        );
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn requested_root_open_fault_refuses_admission() {
        let (_temporary, root_path) = fixture("requested-root-fault");
        let (result, consumed) = run_with_windows_acquisition_fault(
            WindowsAcquisitionPrimitive::RequestedRootOpen,
            1,
            5,
            || JournalRoot::open(&root_path),
        );
        assert!(consumed);
        assert!(matches!(
            result,
            Err(JournalRootError::Io {
                operation: "open journal root",
                ..
            })
        ));
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
            WindowsAcquisitionPrimitive::CloudSyncRootInfoByHandle,
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
    fn retained_root_revalidation_refuses_an_identity_change() {
        let (_temporary, root_path) = fixture("revalidate-identity-mismatch");
        let root = JournalRoot::open(&root_path).expect("admit fixture root");
        let (result, consumed) = with_file_id_identity_mismatch(1, || root.revalidate());
        assert!(consumed);
        assert!(matches!(result, Err(JournalRootError::Changed)));
    }

    #[test]
    fn retained_root_revalidation_refuses_a_mutable_reparse_state() {
        let (_temporary, root_path) = fixture("revalidate-reparse-mismatch");
        let root = JournalRoot::open(&root_path).expect("admit fixture root");
        let (result, consumed) = with_reparse_attribute_substitution(1, || root.revalidate());
        assert!(consumed);
        assert!(matches!(result, Err(JournalRootError::Changed)));
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
    fn canonical_binding_rejects_reparse_ancestor_or_retained_handle_prevents_substitution() {
        let (temporary, root_path) = nested_fixture("journal");
        let admitted = JournalRoot::open(&root_path).expect("admit nested fixture root");
        admitted.revalidate().expect("retained handle");
        admitted
            .revalidate_canonical_binding()
            .expect("recorded pathname still resolves");

        let inner = temporary.path().join("outer").join("inner");
        let moved = temporary.path().join("outer").join("inner-moved");
        match fs::rename(&inner, &moved) {
            Ok(()) => {
                symlink_dir(&moved, &inner).expect("plant reparse ancestor");

                admitted
                    .revalidate()
                    .expect("retained handle still names the admitted directory");
                assert!(
                    matches!(
                        admitted.revalidate_canonical_binding(),
                        Err(JournalRootError::Changed)
                    ),
                    "reparse substitution at an intermediate canonical component must fail the pathname walk"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                // A live admitted descendant can make an intermediate rename unavailable on
                // Windows. That is the stronger outcome: the namespace substitution cannot
                // begin, so the retained authority and its canonical binding stay valid.
                assert!(
                    !moved.exists(),
                    "blocked substitution must not create a moved ancestor"
                );
                admitted
                    .revalidate()
                    .expect("retained handle remains valid after blocked substitution");
                admitted
                    .revalidate_canonical_binding()
                    .expect("blocked substitution leaves canonical binding intact");
            }
            Err(error) => panic!("move intermediate ancestor: {error}"),
        }
    }

    #[test]
    fn real_ntfs_and_refs_controls_skip_without_environment() {
        for variable in ["JOURNAL_WIN_CI_NTFS_ROOT", "SOLSTONE_JOURNAL_WIN_REFS_ROOT"] {
            let Ok(root) = std::env::var(variable) else {
                continue;
            };
            JournalRoot::open(Path::new(&root))
                .unwrap_or_else(|error| panic!("{variable} root must admit: {error}"));
        }
    }
}
