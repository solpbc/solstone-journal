// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Handle-relative, witnessed Windows source inventory operations.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_IO_INCOMPLETE, ERROR_NO_MORE_FILES, ERROR_NOT_FOUND,
    ERROR_NOTIFY_ENUM_DIR, ERROR_OPERATION_ABORTED, ERROR_PATH_NOT_FOUND, INVALID_HANDLE_VALUE,
    OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, STATUS_SUCCESS, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_ID_EXTD_DIR_INFO, FILE_ID_INFO, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_READ_ATTRIBUTES,
    FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    FILE_TYPE_DISK, FileAttributeTagInfo, FileIdExtdDirectoryInfo, FileIdInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx, GetFileType, ReadDirectoryChangesW,
    SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::CreateEventW;

use crate::inventory_budget::{CheckedReadUsage, InventoryUsage};
use crate::{
    InventoryBudget, InventoryBudgetLimit, JournalEntryKind, JournalRoot, JournalRootError,
    ObjectIdentity, check_portable_component,
};

const DIRECTORY_BUFFER_BYTES: usize = 64 * 1024;
const WATCH_BUFFER_BYTES: usize = 64 * 1024;
const WATCH_BUFFER_DWORDS: usize = WATCH_BUFFER_BYTES / size_of::<u32>();

/// Native post-admission primitives covered by the Windows inventory test hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsInventoryPrimitive {
    BorrowAdmittedRootForListing,
    BorrowAdmittedRootForRelativeOpen,
    BorrowAdmittedRootForWatch,
    WatchArm,
    DirectoryList,
    BeforeDescendantOpen,
    DescendantOpen,
    BeforeDescendantListingOpen,
    DescendantListingOpen,
    DescendantListingIdentityRecheck,
    DescendantAttributeTag,
    DescendantFileId,
    DescendantFileType,
    WitnessCheck,
    WitnessCancelIoEx,
    WitnessDrainCompleted,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy)]
struct InventoryInjectedFault {
    primitive: WindowsInventoryPrimitive,
    ordinal: usize,
    raw_error: i32,
}

#[cfg(any(test, feature = "test-hooks"))]
struct InventoryTraceState {
    attempted: Vec<WindowsInventoryPrimitive>,
    successful: Vec<WindowsInventoryPrimitive>,
    barrier: Option<(WindowsInventoryPrimitive, Box<dyn FnOnce()>)>,
    barrier_fired: bool,
    #[cfg(test)]
    force_namespace_change: bool,
    fault: Option<InventoryInjectedFault>,
    fault_consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static INVENTORY_TRACE: std::cell::RefCell<Option<InventoryTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
struct InventoryTraceGuard;

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for InventoryTraceGuard {
    fn drop(&mut self) {
        INVENTORY_TRACE.with(|trace| {
            trace.borrow_mut().take();
        });
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn attempt_inventory(primitive: WindowsInventoryPrimitive) -> io::Result<()> {
    INVENTORY_TRACE.with(|trace| {
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
            let fault = state.fault.take().expect("matching inventory fault");
            state.fault_consumed = true;
            return Err(io::Error::from_raw_os_error(fault.raw_error));
        }
        Ok(())
    })
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_inventory_success(primitive: WindowsInventoryPrimitive) {
    let callback = INVENTORY_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.successful.push(primitive);
        (state.barrier.as_ref().map(|(at, _)| *at) == Some(primitive)).then(|| {
            #[cfg(test)]
            {
                state.force_namespace_change = true;
            }
            state.barrier_fired = true;
            state.barrier.take().expect("pending inventory barrier").1
        })
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(test)]
fn inventory_barrier(primitive: WindowsInventoryPrimitive) {
    let callback = INVENTORY_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        (state.barrier.as_ref().map(|(at, _)| *at) == Some(primitive)).then(|| {
            state.barrier_fired = true;
            state.force_namespace_change = true;
            state.barrier.take().expect("pending inventory barrier").1
        })
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(not(test))]
fn inventory_barrier(_primitive: WindowsInventoryPrimitive) {}

fn traced_inventory<T>(
    primitive: WindowsInventoryPrimitive,
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = primitive;
    #[cfg(any(test, feature = "test-hooks"))]
    attempt_inventory(primitive)?;
    let result = operation();
    #[cfg(any(test, feature = "test-hooks"))]
    if result.is_ok() {
        record_inventory_success(primitive);
    }
    result
}

#[cfg(test)]
fn force_namespace_change() -> bool {
    INVENTORY_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.force_namespace_change)
    })
}

#[cfg(any(test, feature = "test-hooks"))]
/// Run an inventory operation with one native primitive forced to fail.
pub fn run_with_windows_inventory_fault<T>(
    primitive: WindowsInventoryPrimitive,
    ordinal: usize,
    raw_error: i32,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, outcome) = trace_inventory_scenario(
        None,
        Some(InventoryInjectedFault {
            primitive,
            ordinal,
            raw_error,
        }),
        operation,
    );
    (result, outcome.fault_consumed)
}

#[cfg(feature = "test-hooks")]
#[derive(Debug, Eq, PartialEq)]
pub struct WindowsInventoryTrace {
    pub attempted: Vec<WindowsInventoryPrimitive>,
    pub successful: Vec<WindowsInventoryPrimitive>,
    pub fault_consumed: bool,
}

#[cfg(feature = "test-hooks")]
pub fn run_with_windows_inventory_trace<T>(
    operation: impl FnOnce() -> T,
) -> (T, WindowsInventoryTrace) {
    let (result, outcome) = trace_inventory_scenario(None, None, operation);
    (
        result,
        WindowsInventoryTrace {
            attempted: outcome.attempted,
            successful: outcome.successful,
            fault_consumed: outcome.fault_consumed,
        },
    )
}

#[cfg(any(test, feature = "test-hooks"))]
struct InventoryTraceOutcome {
    attempted: Vec<WindowsInventoryPrimitive>,
    successful: Vec<WindowsInventoryPrimitive>,
    #[cfg(test)]
    barrier_fired: bool,
    fault_consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
fn trace_inventory_scenario<T>(
    barrier: Option<(WindowsInventoryPrimitive, Box<dyn FnOnce()>)>,
    fault: Option<InventoryInjectedFault>,
    operation: impl FnOnce() -> T,
) -> (T, InventoryTraceOutcome) {
    INVENTORY_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows inventory trace is already active"
        );
        *trace.borrow_mut() = Some(InventoryTraceState {
            attempted: Vec::new(),
            successful: Vec::new(),
            barrier,
            barrier_fired: false,
            #[cfg(test)]
            force_namespace_change: false,
            fault,
            fault_consumed: false,
        });
    });
    let guard = InventoryTraceGuard;
    let result = operation();
    let state = INVENTORY_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Windows inventory trace remains active")
    });
    drop(guard);
    (
        result,
        InventoryTraceOutcome {
            attempted: state.attempted,
            successful: state.successful,
            #[cfg(test)]
            barrier_fired: state.barrier_fired,
            fault_consumed: state.fault_consumed,
        },
    )
}

/// One recursively observed Windows entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsInventoryEntry {
    relative_path: PathBuf,
    kind: JournalEntryKind,
    identity: ObjectIdentity,
    size: u64,
    last_write_time: u64,
    route: Vec<WindowsRouteComponent>,
}

impl WindowsInventoryEntry {
    /// Relative portable path below the admitted journal root.
    #[must_use]
    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    /// Verified no-follow kind of this entry.
    #[must_use]
    pub const fn kind(&self) -> JournalEntryKind {
        self.kind
    }

    /// Verified volume/file identity of this entry.
    #[must_use]
    pub const fn identity(&self) -> ObjectIdentity {
        self.identity
    }

    /// Verified byte length from the retained entry handle.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WindowsRouteComponent {
    name: OsString,
    identity: ObjectIdentity,
    kind: JournalEntryKind,
}

/// Complete recursive inventory admitted by one namespace witness.
#[derive(Debug)]
pub struct WindowsInventory {
    entries: Vec<WindowsInventoryEntry>,
}

impl WindowsInventory {
    /// Borrow the complete, sorted inventory.
    #[must_use]
    pub fn entries(&self) -> &[WindowsInventoryEntry] {
        &self.entries
    }

    /// Consume the complete, sorted inventory.
    #[must_use]
    pub fn into_entries(self) -> Vec<WindowsInventoryEntry> {
        self.entries
    }
}

/// Failure while building or reading a witnessed Windows source inventory.
#[derive(Debug)]
pub enum WindowsInventoryError {
    Root(JournalRootError),
    Unsupported {
        operation: &'static str,
        source: io::Error,
    },
    BudgetExceeded {
        limit: InventoryBudgetLimit,
    },
    InvalidName {
        path: PathBuf,
    },
    ReparsePoint {
        path: PathBuf,
    },
    NotDirectory {
        path: PathBuf,
    },
    NotRegular {
        path: PathBuf,
    },
    IdentityChanged {
        path: PathBuf,
    },
    NamespaceChanged {
        path: PathBuf,
    },
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for WindowsInventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root(error) => error.fmt(formatter),
            Self::Unsupported { operation, source } => {
                write!(formatter, "{operation} is unsupported: {source}")
            }
            Self::BudgetExceeded { limit } => {
                write!(formatter, "inventory budget exceeded: {limit:?}")
            }
            Self::InvalidName { path } => write!(
                formatter,
                "invalid portable inventory name: {}",
                path.display()
            ),
            Self::ReparsePoint { path } => write!(
                formatter,
                "inventory reparse point refused: {}",
                path.display()
            ),
            Self::NotDirectory { path } => write!(
                formatter,
                "inventory entry is not a directory: {}",
                path.display()
            ),
            Self::NotRegular { path } => write!(
                formatter,
                "inventory entry is not a regular file: {}",
                path.display()
            ),
            Self::IdentityChanged { path } => {
                write!(formatter, "inventory identity changed: {}", path.display())
            }
            Self::NamespaceChanged { path } => {
                write!(formatter, "inventory namespace changed: {}", path.display())
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} failed for {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for WindowsInventoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Root(error) => Some(error),
            Self::Unsupported { source, .. } | Self::Io { source, .. } => Some(source),
            Self::BudgetExceeded { .. }
            | Self::InvalidName { .. }
            | Self::ReparsePoint { .. }
            | Self::NotDirectory { .. }
            | Self::NotRegular { .. }
            | Self::IdentityChanged { .. }
            | Self::NamespaceChanged { .. } => None,
        }
    }
}

/// Build one complete recursive Windows inventory, or return no inventory.
pub fn enumerate_windows_inventory(
    root: &JournalRoot,
    budget: InventoryBudget,
) -> Result<WindowsInventory, WindowsInventoryError> {
    with_witness(root, |witness| {
        let mut usage = InventoryUsage::new();
        let mut entries = Vec::new();
        let listed_root = borrow_admitted_root(
            root,
            WindowsInventoryPrimitive::BorrowAdmittedRootForListing,
        )?;
        walk_directory(
            root,
            listed_root.as_raw_handle(),
            PathBuf::new(),
            String::new(),
            Vec::new(),
            0,
            budget,
            &mut usage,
            &mut entries,
            witness,
        )?;
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(WindowsInventory { entries })
    })
}

fn borrow_admitted_root<'root>(
    root: &'root JournalRoot,
    primitive: WindowsInventoryPrimitive,
) -> Result<BorrowedHandle<'root>, WindowsInventoryError> {
    traced_inventory(primitive, || Ok(root.as_handle())).map_err(|source| {
        WindowsInventoryError::Io {
            operation: "borrow admitted journal root",
            path: root.canonical_path().to_path_buf(),
            source,
        }
    })
}

/// Per-session accounting for checked Windows file reads.
pub struct WindowsCheckedReadSession {
    budget: InventoryBudget,
    usage: CheckedReadUsage,
}

impl WindowsCheckedReadSession {
    /// Begin a fresh checked-read session.
    #[must_use]
    pub const fn new(budget: InventoryBudget) -> Self {
        Self {
            budget,
            usage: CheckedReadUsage::new(),
        }
    }

    /// Read one observed regular entry while proving its complete route remains bound.
    pub fn read(
        &mut self,
        root: &JournalRoot,
        entry: &WindowsInventoryEntry,
    ) -> Result<Vec<u8>, WindowsInventoryError> {
        if entry.kind != JournalEntryKind::RegularFile {
            return Err(WindowsInventoryError::NotRegular {
                path: entry.relative_path.clone(),
            });
        }
        with_witness(root, |witness| {
            test_hook_witness_progress("before-route-reopen");
            let handle = reopen_verified_route(root, entry, FILE_READ_DATA | FILE_READ_ATTRIBUTES)?;
            test_hook_witness_progress("after-route-reopen");
            let before = file_metadata(&handle, &entry.relative_path)?;
            test_hook_witness_progress("after-before-metadata");
            if before.identity != entry.identity
                || before.size != entry.size
                || before.last_write_time != entry.last_write_time
            {
                return Err(WindowsInventoryError::IdentityChanged {
                    path: entry.relative_path.clone(),
                });
            }
            let size = usize::try_from(before.size).map_err(|_| WindowsInventoryError::Io {
                operation: "size checked-read buffer",
                path: entry.relative_path.clone(),
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "observed file exceeds address space",
                ),
            })?;
            self.usage
                .check_reserve(self.budget, size)
                .map_err(|limit| WindowsInventoryError::BudgetExceeded { limit })?;
            let mut bytes = vec![0; size];
            let mut file = std::fs::File::from(handle);
            read_exact(&mut file, &mut bytes).map_err(|source| WindowsInventoryError::Io {
                operation: "read observed file",
                path: entry.relative_path.clone(),
                source,
            })?;
            let handle = file.into();
            test_hook_witness_progress("after-read");
            let after = file_metadata(&handle, &entry.relative_path)?;
            test_hook_witness_progress("after-read-metadata");
            if after != before {
                return Err(WindowsInventoryError::IdentityChanged {
                    path: entry.relative_path.clone(),
                });
            }
            witness.check()?;
            self.usage.commit(size);
            Ok(bytes)
        })
    }
}

/// Read one observed regular entry in a fresh checked-read session.
pub fn read_windows_inventory_file(
    root: &JournalRoot,
    entry: &WindowsInventoryEntry,
    budget: InventoryBudget,
) -> Result<Vec<u8>, WindowsInventoryError> {
    WindowsCheckedReadSession::new(budget).read(root, entry)
}

fn with_witness<'root, T>(
    root: &'root JournalRoot,
    operation: impl FnOnce(&mut NamespaceWitness<'root>) -> Result<T, WindowsInventoryError>,
) -> Result<T, WindowsInventoryError> {
    test_hook_witness_progress("before-revalidate");
    root.revalidate().map_err(WindowsInventoryError::Root)?;
    test_hook_witness_progress("before-arm");
    let mut witness = NamespaceWitness::arm(root)?;
    test_hook_witness_progress("after-arm");
    let result = operation(&mut witness).and_then(|value| {
        test_hook_witness_progress("after-operation");
        root.revalidate().map_err(WindowsInventoryError::Root)?;
        test_hook_witness_progress("after-revalidate");
        witness.check()?;
        test_hook_witness_progress("after-check");
        Ok(value)
    });
    test_hook_witness_progress("before-drain");
    let cleanup = witness.cancel_and_drain();
    test_hook_witness_progress("after-drain");
    result.and_then(|value| cleanup.map(|()| value))
}

#[cfg(feature = "test-hooks")]
fn test_hook_witness_progress(stage: &str) {
    println!("JOURNAL_WIN_CI_TEST_HOOK_WITNESS={stage}");
}

#[cfg(not(feature = "test-hooks"))]
fn test_hook_witness_progress(_stage: &str) {}

struct NamespaceWitness<'root> {
    handle: BorrowedHandle<'root>,
    _event: OwnedHandle,
    buffer: Vec<u32>,
    overlapped: Box<OVERLAPPED>,
    armed: bool,
    root: PathBuf,
}

impl<'root> NamespaceWitness<'root> {
    fn arm(root: &'root JournalRoot) -> Result<Self, WindowsInventoryError> {
        let handle =
            borrow_admitted_root(root, WindowsInventoryPrimitive::BorrowAdmittedRootForWatch)?;
        // The witness borrows the admitted root but gives its one asynchronous request a private
        // completion event. That event remains live through cancellation and drain, so result
        // checks never infer this request's state from unrelated handle activity.
        let raw_event = {
            // SAFETY: the unnamed manual-reset event starts unsignaled and is immediately owned
            // below after the null-handle check.
            #[allow(unsafe_code)]
            unsafe {
                CreateEventW(std::ptr::null(), 1, 0, std::ptr::null())
            }
        };
        if raw_event.is_null() {
            return Err(WindowsInventoryError::Io {
                operation: "create recursive namespace witness event",
                path: root.canonical_path().to_path_buf(),
                source: io::Error::last_os_error(),
            });
        }
        // SAFETY: `CreateEventW` returned a non-null event handle owned by this witness.
        #[allow(unsafe_code)]
        let event = unsafe { OwnedHandle::from_raw_handle(raw_event) };
        // Windows retains the exact `OVERLAPPED` address until the asynchronous request is
        // drained. Heap allocation prevents `NamespaceWitness` returning from `arm` from moving
        // that address.
        let mut overlapped = Box::new(OVERLAPPED::default());
        overlapped.hEvent = event.as_raw_handle();
        let mut witness = Self {
            handle,
            _event: event,
            buffer: vec![0; WATCH_BUFFER_DWORDS],
            overlapped,
            armed: false,
            root: root.canonical_path().to_path_buf(),
        };
        traced_inventory(WindowsInventoryPrimitive::WatchArm, || {
            // SAFETY: the witness borrows the retained asynchronous directory handle,
            // `Vec<u32>` supplies the documented DWORD-aligned buffer, and both that buffer and
            // `OVERLAPPED` remain live until the request drains.
            #[allow(unsafe_code)]
            let result = unsafe {
                ReadDirectoryChangesW(
                    witness.handle.as_raw_handle(),
                    witness.buffer.as_mut_ptr().cast(),
                    WATCH_BUFFER_BYTES as u32,
                    1,
                    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                    std::ptr::null_mut(),
                    witness.overlapped.as_mut(),
                    None,
                )
            };
            (result != 0)
                .then_some(())
                .ok_or_else(io::Error::last_os_error)
        })
        .map_err(|source| {
            if source.raw_os_error() == Some(ERROR_NOTIFY_ENUM_DIR as i32) {
                WindowsInventoryError::NamespaceChanged {
                    path: root.canonical_path().to_path_buf(),
                }
            } else {
                WindowsInventoryError::Unsupported {
                    operation: "arm recursive namespace witness",
                    source,
                }
            }
        })?;
        witness.armed = true;
        Ok(witness)
    }

    fn check(&mut self) -> Result<(), WindowsInventoryError> {
        #[cfg(test)]
        if force_namespace_change() {
            return Err(WindowsInventoryError::NamespaceChanged {
                path: self.root.clone(),
            });
        }
        let mut bytes = 0;
        let result = traced_inventory(WindowsInventoryPrimitive::WitnessCheck, || {
            // SAFETY: `overlapped` belongs to the one outstanding watch request on `handle`, and `bytes` is writable for the documented output length.
            #[allow(unsafe_code)]
            let completed = unsafe {
                GetOverlappedResult(
                    self.handle.as_raw_handle(),
                    self.overlapped.as_ref(),
                    &mut bytes,
                    0,
                )
            };
            if completed != 0 {
                return Ok(Some(bytes));
            }
            let source = io::Error::last_os_error();
            (source.raw_os_error() == Some(ERROR_IO_INCOMPLETE as i32))
                .then_some(None)
                .ok_or(source)
        });
        match result {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(WindowsInventoryError::NamespaceChanged {
                path: self.root.clone(),
            }),
            Err(source) if source.raw_os_error() == Some(ERROR_NOTIFY_ENUM_DIR as i32) => {
                Err(WindowsInventoryError::NamespaceChanged {
                    path: self.root.clone(),
                })
            }
            Err(source) => Err(WindowsInventoryError::Unsupported {
                operation: "check recursive namespace witness",
                source,
            }),
        }
    }

    #[cfg(test)]
    fn wait_for_notification(&mut self) -> Result<u32, WindowsInventoryError> {
        let mut bytes = 0;
        // SAFETY: the outstanding request owns this exact `OVERLAPPED`, and `bytes` is writable for the documented completion length while the test waits for the mutation it just initiated.
        #[allow(unsafe_code)]
        let completed = unsafe {
            GetOverlappedResult(
                self.handle.as_raw_handle(),
                self.overlapped.as_ref(),
                &mut bytes,
                1,
            )
        };
        if completed != 0 {
            return Ok(bytes);
        }
        let source = io::Error::last_os_error();
        if source.raw_os_error() == Some(ERROR_NOTIFY_ENUM_DIR as i32) {
            return Err(WindowsInventoryError::NamespaceChanged {
                path: self.root.clone(),
            });
        }
        Err(WindowsInventoryError::Unsupported {
            operation: "wait for recursive namespace witness",
            source,
        })
    }

    fn cancel_and_drain(&mut self) -> Result<(), WindowsInventoryError> {
        if !self.armed {
            return Ok(());
        }
        test_hook_witness_progress("before-cancel");
        traced_inventory(WindowsInventoryPrimitive::WitnessCancelIoEx, || {
            // SAFETY: this witness's exact, heap-stable `OVERLAPPED` remains allocated until the
            // drain below completes. The retained root may serve other callers, so cancellation
            // must not affect any request other than this witness.
            #[allow(unsafe_code)]
            let cancelled =
                unsafe { CancelIoEx(self.handle.as_raw_handle(), self.overlapped.as_ref()) };
            if cancelled != 0 {
                return Ok(());
            }
            let source = io::Error::last_os_error();
            (source.raw_os_error() == Some(ERROR_NOT_FOUND as i32))
                .then_some(())
                .ok_or(source)
        })
        .map_err(|source| WindowsInventoryError::Unsupported {
            operation: "cancel recursive namespace witness",
            source,
        })?;
        test_hook_witness_progress("after-cancel");
        let mut bytes = 0;
        test_hook_witness_progress("before-drain-result");
        let drained = traced_inventory(WindowsInventoryPrimitive::WitnessDrainCompleted, || {
            // SAFETY: the request was either completed or cancellation was requested above, and both the request's `OVERLAPPED` and output length remain live for this drain.
            #[allow(unsafe_code)]
            let drained = unsafe {
                GetOverlappedResult(
                    self.handle.as_raw_handle(),
                    self.overlapped.as_ref(),
                    &mut bytes,
                    1,
                )
            };
            if drained != 0 {
                return Ok(Some(bytes));
            }
            let source = io::Error::last_os_error();
            (source.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32))
                .then_some(None)
                .ok_or(source)
        })
        .map_err(|source| WindowsInventoryError::Unsupported {
            operation: "drain recursive namespace witness",
            source,
        })?;
        test_hook_witness_progress("after-drain-result");
        self.armed = false;
        drained
            .is_none()
            .then_some(())
            .ok_or_else(|| WindowsInventoryError::NamespaceChanged {
                path: self.root.clone(),
            })
    }
}

impl Drop for NamespaceWitness<'_> {
    fn drop(&mut self) {
        let _ = self.cancel_and_drain();
    }
}

fn walk_directory(
    root: &JournalRoot,
    directory: std::os::windows::io::RawHandle,
    relative: PathBuf,
    member: String,
    route: Vec<WindowsRouteComponent>,
    depth: usize,
    budget: InventoryBudget,
    usage: &mut InventoryUsage,
    entries: &mut Vec<WindowsInventoryEntry>,
    witness: &mut NamespaceWitness,
) -> Result<(), WindowsInventoryError> {
    InventoryUsage::check_depth(budget, depth)
        .map_err(|limit| WindowsInventoryError::BudgetExceeded { limit })?;
    let listed = list_directory(directory, &relative)?;
    for listed in listed {
        usage
            .observe_entry(budget)
            .map_err(|limit| WindowsInventoryError::BudgetExceeded { limit })?;
        let (name, child_member) = checked_name(&listed.name, &relative, &member, budget)?;
        let mut child_path = relative.clone();
        child_path.push(&name);
        let next_depth = depth
            .checked_add(1)
            .ok_or(WindowsInventoryError::BudgetExceeded {
                limit: InventoryBudgetLimit::Depth,
            })?;
        InventoryUsage::check_depth(budget, next_depth)
            .map_err(|limit| WindowsInventoryError::BudgetExceeded { limit })?;
        let child = open_relative(directory, &name, FILE_READ_ATTRIBUTES, false, &child_path)?;
        let attributes = attribute_tag(&child, &child_path)?;
        require_no_reparse(attributes, &child_path)?;
        let identity = file_id(&child, &child_path)?;
        if !identity.matches_windows_file_id(listed.file_id) {
            return Err(WindowsInventoryError::IdentityChanged { path: child_path });
        }
        let kind = if is_directory(attributes) {
            JournalEntryKind::Directory
        } else {
            require_regular(&child, &child_path)?;
            JournalEntryKind::RegularFile
        };
        let metadata = file_metadata(&child, &child_path)?;
        let listed_child = (kind == JournalEntryKind::Directory)
            .then(|| open_relative_for_directory_listing(directory, &name, identity, &child_path))
            .transpose()?;
        let mut child_route = route.clone();
        child_route.push(WindowsRouteComponent {
            name,
            identity,
            kind,
        });
        entries.push(WindowsInventoryEntry {
            relative_path: child_path.clone(),
            kind,
            identity,
            size: metadata.size,
            last_write_time: metadata.last_write_time,
            route: child_route.clone(),
        });
        if let Some(listed_child) = listed_child {
            walk_directory(
                root,
                listed_child.as_raw_handle(),
                child_path,
                child_member,
                child_route,
                next_depth,
                budget,
                usage,
                entries,
                witness,
            )?;
        }
        witness.check()?;
        root.revalidate().map_err(WindowsInventoryError::Root)?;
    }
    Ok(())
}

struct ListedEntry {
    name: OsString,
    file_id: [u8; 16],
}

fn list_directory(
    directory: std::os::windows::io::RawHandle,
    relative: &Path,
) -> Result<Vec<ListedEntry>, WindowsInventoryError> {
    let mut buffer = vec![0u8; DIRECTORY_BUFFER_BYTES];
    let mut entries = Vec::new();
    loop {
        let result = traced_inventory(WindowsInventoryPrimitive::DirectoryList, || {
            // SAFETY: `directory` is a retained directory-list handle and `buffer` is writable for its exact supplied size.
            #[allow(unsafe_code)]
            let result = unsafe {
                GetFileInformationByHandleEx(
                    directory,
                    FileIdExtdDirectoryInfo,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                )
            };
            (result != 0)
                .then_some(())
                .ok_or_else(io::Error::last_os_error)
        });
        match result {
            Ok(()) => parse_directory_buffer(&buffer, relative, &mut entries)?,
            Err(source) if source.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) => break,
            Err(source) => {
                return Err(WindowsInventoryError::Io {
                    operation: "list retained directory",
                    path: relative.to_path_buf(),
                    source,
                });
            }
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

fn parse_directory_buffer(
    buffer: &[u8],
    relative: &Path,
    entries: &mut Vec<ListedEntry>,
) -> Result<(), WindowsInventoryError> {
    let header_bytes = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
    let mut offset = 0usize;
    loop {
        let remaining = buffer
            .get(offset..)
            .ok_or_else(|| invalid_directory_buffer(relative))?;
        let header = remaining
            .get(..header_bytes)
            .ok_or_else(|| invalid_directory_buffer(relative))?;
        let name_bytes = usize::try_from(directory_u32(
            header,
            offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength),
            relative,
        )?)
        .map_err(|_| invalid_directory_buffer(relative))?;
        if name_bytes % size_of::<u16>() != 0 {
            return Err(invalid_directory_buffer(relative));
        }
        let record_bytes = header_bytes
            .checked_add(name_bytes)
            .ok_or_else(|| invalid_directory_buffer(relative))?;
        let record = remaining
            .get(..record_bytes)
            .ok_or_else(|| invalid_directory_buffer(relative))?;
        let name = record[header_bytes..]
            .chunks_exact(size_of::<u16>())
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let name = String::from_utf16(&name).map_err(|_| WindowsInventoryError::InvalidName {
            path: relative.to_path_buf(),
        })?;
        let file_id_start = offset_of!(FILE_ID_EXTD_DIR_INFO, FileId);
        let file_id_end = file_id_start
            .checked_add(16)
            .ok_or_else(|| invalid_directory_buffer(relative))?;
        let file_id = record
            .get(file_id_start..file_id_end)
            .ok_or_else(|| invalid_directory_buffer(relative))?
            .try_into()
            .map_err(|_| invalid_directory_buffer(relative))?;
        // FileIdExtdDirectoryInfo includes the directory's synthetic self and
        // parent records.  They are protocol entries rather than members of
        // the journal namespace, so omit exactly those two spellings before
        // portable-component admission.  Every other name remains subject to
        // the normal checked-name path below.
        if name != "." && name != ".." {
            entries.push(ListedEntry {
                name: OsString::from(name),
                file_id,
            });
        }
        let next_entry_offset = directory_u32(
            header,
            offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset),
            relative,
        )?;
        if next_entry_offset == 0 {
            break;
        }
        let next =
            usize::try_from(next_entry_offset).map_err(|_| invalid_directory_buffer(relative))?;
        if next < record_bytes || next > remaining.len() {
            return Err(invalid_directory_buffer(relative));
        }
        offset = offset
            .checked_add(next)
            .ok_or_else(|| invalid_directory_buffer(relative))?;
    }
    Ok(())
}

fn directory_u32(
    header: &[u8],
    field_offset: usize,
    relative: &Path,
) -> Result<u32, WindowsInventoryError> {
    let field_end = field_offset
        .checked_add(size_of::<u32>())
        .ok_or_else(|| invalid_directory_buffer(relative))?;
    let field: [u8; 4] = header
        .get(field_offset..field_end)
        .ok_or_else(|| invalid_directory_buffer(relative))?
        .try_into()
        .map_err(|_| invalid_directory_buffer(relative))?;
    Ok(u32::from_le_bytes(field))
}

fn invalid_directory_buffer(relative: &Path) -> WindowsInventoryError {
    WindowsInventoryError::Io {
        operation: "parse retained directory listing",
        path: relative.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed FileIdExtdDirectoryInfo buffer",
        ),
    }
}

fn checked_name(
    name: &OsStr,
    parent: &Path,
    parent_member: &str,
    budget: InventoryBudget,
) -> Result<(OsString, String), WindowsInventoryError> {
    let text = name
        .to_str()
        .ok_or_else(|| WindowsInventoryError::InvalidName {
            path: parent.join(name),
        })?;
    check_portable_component(text).map_err(|_| WindowsInventoryError::InvalidName {
        path: parent.join(name),
    })?;
    let member = if parent_member.is_empty() {
        text.to_owned()
    } else {
        format!("{parent_member}/{text}")
    };
    InventoryUsage::check_member(budget, &member)
        .map_err(|limit| WindowsInventoryError::BudgetExceeded { limit })?;
    let path = parent.join(name);
    InventoryUsage::check_relative_path(budget, &path)
        .map_err(|limit| WindowsInventoryError::BudgetExceeded { limit })?;
    Ok((name.to_os_string(), member))
}

fn open_relative(
    parent: std::os::windows::io::RawHandle,
    name: &OsStr,
    desired_access: u32,
    directory_only: bool,
    path: &Path,
) -> Result<OwnedHandle, WindowsInventoryError> {
    inventory_barrier(WindowsInventoryPrimitive::BeforeDescendantOpen);
    let wide = name.encode_wide().collect::<Vec<_>>();
    let byte_length = wide
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| WindowsInventoryError::InvalidName {
            path: path.to_path_buf(),
        })?;
    let mut object_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: wide.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent,
        ObjectName: &mut object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle = INVALID_HANDLE_VALUE;
    let mut status = windows_sys::Win32::System::IO::IO_STATUS_BLOCK::default();
    let options = FILE_OPEN_REPARSE_POINT
        | FILE_OPEN_FOR_BACKUP_INTENT
        | FILE_SYNCHRONOUS_IO_NONALERT
        | if directory_only {
            FILE_DIRECTORY_FILE
        } else {
            0
        };
    let handle = traced_inventory(WindowsInventoryPrimitive::DescendantOpen, || {
        // SAFETY: `attributes` refers to the live UTF-16 component and retained parent handle; all output pointers refer to initialized local storage, and the synchronous request does not outlive them.
        #[allow(unsafe_code)]
        let result = unsafe {
            NtCreateFile(
                &mut handle,
                desired_access | SYNCHRONIZE,
                &attributes,
                &mut status,
                std::ptr::null(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_OPEN,
                options,
                std::ptr::null(),
                0,
            )
        };
        (result == STATUS_SUCCESS).then_some(handle).ok_or_else(|| {
            // SAFETY: `RtlNtStatusToDosError` converts the just-returned NTSTATUS without borrowing any caller memory.
            #[allow(unsafe_code)]
            let error = unsafe { RtlNtStatusToDosError(result) };
            io::Error::from_raw_os_error(error as i32)
        })
    })
    .map_err(|source| relative_open_error(path, source))?;
    // SAFETY: `NtCreateFile` returned one owned valid handle and the conversion occurs exactly once.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn open_relative_for_directory_listing(
    parent: std::os::windows::io::RawHandle,
    name: &OsStr,
    expected_identity: ObjectIdentity,
    path: &Path,
) -> Result<OwnedHandle, WindowsInventoryError> {
    inventory_barrier(WindowsInventoryPrimitive::BeforeDescendantListingOpen);
    let handle = open_relative(
        parent,
        name,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE,
        true,
        path,
    )?;
    let handle = traced_inventory(WindowsInventoryPrimitive::DescendantListingOpen, || {
        Ok(handle)
    })
    .map_err(|source| WindowsInventoryError::Io {
        operation: "trace retained relative directory listing open",
        path: path.to_path_buf(),
        source,
    })?;
    let attributes = attribute_tag(&handle, path)?;
    require_no_reparse(attributes, path)?;
    if !is_directory(attributes) {
        return Err(WindowsInventoryError::NotDirectory {
            path: path.to_path_buf(),
        });
    }
    let identity = file_id(&handle, path)?;
    let matches = identity == expected_identity;
    match traced_inventory(
        WindowsInventoryPrimitive::DescendantListingIdentityRecheck,
        || {
            matches.then_some(()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "descendant directory identity changed before listing",
                )
            })
        },
    ) {
        Ok(()) => Ok(handle),
        Err(_) if !matches => Err(WindowsInventoryError::IdentityChanged {
            path: path.to_path_buf(),
        }),
        Err(source) => Err(WindowsInventoryError::Io {
            operation: "reverify retained relative directory identity",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn relative_open_error(path: &Path, source: io::Error) -> WindowsInventoryError {
    if matches!(
        source.raw_os_error(),
        Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
    ) {
        WindowsInventoryError::IdentityChanged {
            path: path.to_path_buf(),
        }
    } else {
        WindowsInventoryError::Io {
            operation: "open retained relative entry",
            path: path.to_path_buf(),
            source,
        }
    }
}

fn attribute_tag(
    handle: &OwnedHandle,
    path: &Path,
) -> Result<FILE_ATTRIBUTE_TAG_INFO, WindowsInventoryError> {
    traced_inventory(WindowsInventoryPrimitive::DescendantAttributeTag, || {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: `info` is writable for its exact size and `handle` remains valid for this metadata query.
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
    .map_err(|source| WindowsInventoryError::Io {
        operation: "query retained entry attributes",
        path: path.to_path_buf(),
        source,
    })
}

fn file_id(handle: &OwnedHandle, path: &Path) -> Result<ObjectIdentity, WindowsInventoryError> {
    traced_inventory(WindowsInventoryPrimitive::DescendantFileId, || {
        let mut info = FILE_ID_INFO::default();
        // SAFETY: `info` is writable for its exact size and `handle` remains valid for this identity query.
        #[allow(unsafe_code)]
        let result = unsafe {
            GetFileInformationByHandleEx(
                handle.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        };
        (result != 0)
            .then_some(ObjectIdentity::from_volume_and_file_id(
                info.VolumeSerialNumber,
                info.FileId.Identifier,
            ))
            .ok_or_else(io::Error::last_os_error)
    })
    .map_err(|source| WindowsInventoryError::Io {
        operation: "query retained entry identity",
        path: path.to_path_buf(),
        source,
    })
}

fn require_regular(handle: &OwnedHandle, path: &Path) -> Result<(), WindowsInventoryError> {
    let file_type = traced_inventory(WindowsInventoryPrimitive::DescendantFileType, || {
        // SAFETY: `handle` is a valid retained handle for `GetFileType`.
        #[allow(unsafe_code)]
        let value = unsafe { GetFileType(handle.as_raw_handle()) };
        Ok(value)
    })
    .map_err(|source| WindowsInventoryError::Io {
        operation: "query retained entry type",
        path: path.to_path_buf(),
        source,
    })?;
    (file_type == FILE_TYPE_DISK)
        .then_some(())
        .ok_or_else(|| WindowsInventoryError::NotRegular {
            path: path.to_path_buf(),
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsFileMetadata {
    identity: ObjectIdentity,
    size: u64,
    last_write_time: u64,
}

fn file_metadata(
    handle: &OwnedHandle,
    path: &Path,
) -> Result<WindowsFileMetadata, WindowsInventoryError> {
    let identity = file_id(handle, path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `info` is writable for its exact size and `handle` remains valid for the metadata query.
    #[allow(unsafe_code)]
    let result = unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) };
    if result == 0 {
        return Err(WindowsInventoryError::Io {
            operation: "query retained entry metadata",
            path: path.to_path_buf(),
            source: io::Error::last_os_error(),
        });
    }
    Ok(WindowsFileMetadata {
        identity,
        size: ((info.nFileSizeHigh as u64) << 32) | u64::from(info.nFileSizeLow),
        last_write_time: ((info.ftLastWriteTime.dwHighDateTime as u64) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime),
    })
}

fn reopen_verified_route(
    root: &JournalRoot,
    entry: &WindowsInventoryEntry,
    leaf_access: u32,
) -> Result<OwnedHandle, WindowsInventoryError> {
    let retained_root = borrow_admitted_root(
        root,
        WindowsInventoryPrimitive::BorrowAdmittedRootForRelativeOpen,
    )?;
    let mut parent = retained_root.as_raw_handle();
    let mut owned_parent = None;
    for (index, component) in entry.route.iter().enumerate() {
        test_hook_witness_progress("before-route-component");
        let leaf = index + 1 == entry.route.len();
        let access = if leaf {
            leaf_access
        } else {
            FILE_READ_ATTRIBUTES | FILE_TRAVERSE
        };
        let handle = open_relative(parent, &component.name, access, false, &entry.relative_path)?;
        let attributes = attribute_tag(&handle, &entry.relative_path)?;
        require_no_reparse(attributes, &entry.relative_path)?;
        let identity = file_id(&handle, &entry.relative_path)?;
        test_hook_witness_progress("after-route-component-identity");
        if identity != component.identity {
            test_hook_witness_progress("route-component-identity-mismatch");
            return Err(WindowsInventoryError::IdentityChanged {
                path: entry.relative_path.clone(),
            });
        }
        match component.kind {
            JournalEntryKind::Directory => {
                if !is_directory(attributes) {
                    return Err(WindowsInventoryError::NotDirectory {
                        path: entry.relative_path.clone(),
                    });
                }
            }
            JournalEntryKind::RegularFile => {
                if is_directory(attributes) {
                    return Err(WindowsInventoryError::NotRegular {
                        path: entry.relative_path.clone(),
                    });
                }
                require_regular(&handle, &entry.relative_path)?;
            }
            _ => {
                return Err(WindowsInventoryError::IdentityChanged {
                    path: entry.relative_path.clone(),
                });
            }
        }
        parent = handle.as_raw_handle();
        owned_parent = Some(handle);
    }
    owned_parent.ok_or_else(|| WindowsInventoryError::IdentityChanged {
        path: entry.relative_path.clone(),
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

fn require_no_reparse(
    attributes: FILE_ATTRIBUTE_TAG_INFO,
    path: &Path,
) -> Result<(), WindowsInventoryError> {
    (!is_reparse_point(attributes))
        .then_some(())
        .ok_or_else(|| WindowsInventoryError::ReparsePoint {
            path: path.to_path_buf(),
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TempDir;

    fn budget() -> InventoryBudget {
        InventoryBudget::new(32, 8, 255, 1024, 1024)
    }

    fn fixture() -> (TempDir, JournalRoot) {
        let temporary = TempDir::new();
        let root_path = temporary.path().join("journal");
        fs::create_dir_all(root_path.join("one/two")).unwrap();
        fs::write(root_path.join("root.txt"), b"root").unwrap();
        fs::write(root_path.join("one/two/leaf.txt"), b"leaf").unwrap();
        let root = JournalRoot::open(&root_path).unwrap();
        (temporary, root)
    }

    #[test]
    fn complete_inventory_and_checked_read_use_retained_relative_handles() {
        let (_temporary, root) = fixture();
        let inventory = enumerate_windows_inventory(&root, budget()).unwrap();
        let paths = inventory
            .entries()
            .iter()
            .map(|entry| entry.relative_path().to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("one"),
                PathBuf::from("one/two"),
                PathBuf::from("one/two/leaf.txt"),
                PathBuf::from("root.txt"),
            ]
        );
        let leaf = inventory
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("one/two/leaf.txt"))
            .unwrap();
        assert_eq!(
            read_windows_inventory_file(&root, leaf, budget()).unwrap(),
            b"leaf"
        );
        let directory = inventory
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("one"))
            .unwrap();
        assert!(matches!(
            read_windows_inventory_file(&root, directory, budget()),
            Err(WindowsInventoryError::NotRegular { .. })
        ));
    }

    #[test]
    fn every_budget_dimension_refuses_whole_operation() {
        let (_temporary, root) = fixture();
        for budget in [
            InventoryBudget::new(1, 8, 255, 1024, 1024),
            InventoryBudget::new(32, 1, 255, 1024, 1024),
            InventoryBudget::new(32, 8, 3, 1024, 1024),
            InventoryBudget::new(32, 8, 255, 6, 1024),
        ] {
            assert!(matches!(
                enumerate_windows_inventory(&root, budget),
                Err(WindowsInventoryError::BudgetExceeded { .. })
            ));
        }
        let inventory = enumerate_windows_inventory(&root, budget()).unwrap();
        let leaf = inventory
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("one/two/leaf.txt"))
            .unwrap();
        assert!(matches!(
            read_windows_inventory_file(&root, leaf, InventoryBudget::new(32, 8, 255, 1024, 3)),
            Err(WindowsInventoryError::BudgetExceeded {
                limit: InventoryBudgetLimit::CheckedReadBytes
            })
        ));

        let root_file = inventory
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("root.txt"))
            .unwrap();
        let mut session = WindowsCheckedReadSession::new(InventoryBudget::new(32, 8, 255, 1024, 7));
        assert_eq!(session.read(&root, root_file).unwrap(), b"root");
        assert!(matches!(
            session.read(&root, leaf),
            Err(WindowsInventoryError::BudgetExceeded {
                limit: InventoryBudgetLimit::CheckedReadBytes
            })
        ));
    }

    #[test]
    fn barriers_refuse_add_remove_cross_subtree_and_aba_mutations() {
        for mutation in [
            Box::new(|path: &Path| {
                fs::write(path.join("added"), b"new").unwrap();
            }) as Box<dyn FnOnce(&Path)>,
            Box::new(|path: &Path| {
                fs::remove_file(path.join("root.txt")).unwrap();
            }),
            Box::new(|path: &Path| {
                fs::write(path.join("one/two/other"), b"new").unwrap();
            }),
            Box::new(|path: &Path| {
                let leaf = path.join("root.txt");
                fs::remove_file(&leaf).unwrap();
                fs::write(leaf, b"root").unwrap();
            }),
        ] {
            let (_temporary, root) = fixture();
            let path = root.canonical_path().to_path_buf();
            let (result, trace) = trace_inventory_scenario(
                Some((
                    WindowsInventoryPrimitive::WatchArm,
                    Box::new(move || mutation(&path)),
                )),
                None,
                || enumerate_windows_inventory(&root, budget()),
            );
            assert!(trace.barrier_fired);
            assert!(
                trace
                    .successful
                    .contains(&WindowsInventoryPrimitive::WatchArm)
            );
            assert!(matches!(
                result,
                Err(WindowsInventoryError::NamespaceChanged { .. })
            ));
        }
    }

    #[test]
    fn named_witness_failures_are_refused_without_fallback() {
        let (_temporary, root) = fixture();
        let (result, consumed) = run_with_windows_inventory_fault(
            WindowsInventoryPrimitive::WatchArm,
            1,
            windows_sys::Win32::Foundation::ERROR_INVALID_FUNCTION as i32,
            || enumerate_windows_inventory(&root, budget()),
        );
        assert!(consumed);
        assert!(matches!(
            result,
            Err(WindowsInventoryError::Unsupported { .. })
        ));

        let (_temporary, root) = fixture();
        let (result, consumed) = run_with_windows_inventory_fault(
            WindowsInventoryPrimitive::WitnessCheck,
            1,
            ERROR_NOTIFY_ENUM_DIR as i32,
            || enumerate_windows_inventory(&root, budget()),
        );
        assert!(consumed);
        assert!(matches!(
            result,
            Err(WindowsInventoryError::NamespaceChanged { .. })
        ));

        let (_temporary, root) = fixture();
        let (result, consumed) = run_with_windows_inventory_fault(
            WindowsInventoryPrimitive::WatchArm,
            1,
            ERROR_NOTIFY_ENUM_DIR as i32,
            || enumerate_windows_inventory(&root, budget()),
        );
        assert!(consumed);
        assert!(matches!(
            result,
            Err(WindowsInventoryError::NamespaceChanged { .. })
        ));
    }

    #[test]
    fn operation_error_cancels_and_drains_the_witness_before_returning() {
        let (_temporary, root) = fixture();
        let (result, trace) = trace_inventory_scenario(
            None,
            Some(InventoryInjectedFault {
                primitive: WindowsInventoryPrimitive::DirectoryList,
                ordinal: 1,
                raw_error: 5,
            }),
            || enumerate_windows_inventory(&root, budget()),
        );
        assert!(trace.fault_consumed);
        assert!(matches!(result, Err(WindowsInventoryError::Io { .. })));
        let cancel = trace
            .successful
            .iter()
            .position(|primitive| *primitive == WindowsInventoryPrimitive::WitnessCancelIoEx)
            .expect("operation error explicitly cancels the witness");
        let drain = trace
            .successful
            .iter()
            .position(|primitive| *primitive == WindowsInventoryPrimitive::WitnessDrainCompleted)
            .expect("operation error explicitly drains the witness");
        assert!(cancel < drain);
    }

    #[test]
    fn malformed_names_and_reparse_attributes_are_refused_by_adapters() {
        assert!(matches!(
            checked_name(OsStr::new("bad:name"), Path::new(""), "", budget()),
            Err(WindowsInventoryError::InvalidName { .. })
        ));
        let attributes = FILE_ATTRIBUTE_TAG_INFO {
            FileAttributes: FILE_ATTRIBUTE_REPARSE_POINT,
            ReparseTag: 0,
        };
        assert!(matches!(
            require_no_reparse(attributes, Path::new("junction")),
            Err(WindowsInventoryError::ReparsePoint { .. })
        ));

        let mut entries = Vec::new();
        assert!(matches!(
            parse_directory_buffer(&[], Path::new(""), &mut entries),
            Err(WindowsInventoryError::Io {
                operation: "parse retained directory listing",
                ..
            })
        ));
    }

    #[test]
    fn short_final_directory_record_is_decoded_without_struct_read() {
        let header_bytes = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
        let mut record = vec![0; header_bytes + size_of::<u16>()];
        record[offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength)
            ..offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength) + size_of::<u32>()]
            .copy_from_slice(&(size_of::<u16>() as u32).to_le_bytes());
        record[offset_of!(FILE_ID_EXTD_DIR_INFO, FileId)
            ..offset_of!(FILE_ID_EXTD_DIR_INFO, FileId) + 16]
            .copy_from_slice(&[0x5A; 16]);
        record[header_bytes..].copy_from_slice(&('x' as u16).to_le_bytes());

        let mut entries = Vec::new();
        parse_directory_buffer(&record, Path::new(""), &mut entries).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, OsString::from("x"));
        assert_eq!(entries[0].file_id, [0x5A; 16]);
    }

    #[test]
    fn directory_protocol_dot_entries_are_omitted_without_widening_name_admission() {
        fn record(name: &[u16], file_id: u8, next_entry_offset: u32) -> Vec<u8> {
            let header_bytes = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
            let mut record = vec![0; header_bytes + std::mem::size_of_val(name)];
            record[offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset)
                ..offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset) + size_of::<u32>()]
                .copy_from_slice(&next_entry_offset.to_le_bytes());
            record[offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength)
                ..offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength) + size_of::<u32>()]
                .copy_from_slice(&(std::mem::size_of_val(name) as u32).to_le_bytes());
            record[offset_of!(FILE_ID_EXTD_DIR_INFO, FileId)
                ..offset_of!(FILE_ID_EXTD_DIR_INFO, FileId) + 16]
                .copy_from_slice(&[file_id; 16]);
            for (offset, code_unit) in name.iter().enumerate() {
                let start = header_bytes + offset * size_of::<u16>();
                record[start..start + size_of::<u16>()].copy_from_slice(&code_unit.to_le_bytes());
            }
            record
        }

        let header_bytes = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
        let dot = record(&['.' as u16], 0x11, (header_bytes + 2) as u32);
        let dotdot = record(&['.' as u16, '.' as u16], 0x22, (header_bytes + 4) as u32);
        let ordinary = record(&['o' as u16, 'k' as u16], 0x5A, 0);
        let buffer = [dot, dotdot, ordinary].concat();

        let mut entries = Vec::new();
        parse_directory_buffer(&buffer, Path::new(""), &mut entries).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, OsString::from("ok"));
        assert_eq!(entries[0].file_id, [0x5A; 16]);

        let mut leading_dot = Vec::new();
        parse_directory_buffer(
            &record(&['.' as u16, 'x' as u16], 0x33, 0),
            Path::new(""),
            &mut leading_dot,
        )
        .unwrap();
        assert_eq!(leading_dot[0].name, OsString::from(".x"));
        checked_name(&leading_dot[0].name, Path::new(""), "", budget()).unwrap();

        let mut trailing_dot = Vec::new();
        parse_directory_buffer(
            &record(&['x' as u16, '.' as u16], 0x44, 0),
            Path::new(""),
            &mut trailing_dot,
        )
        .unwrap();
        assert!(matches!(
            checked_name(&trailing_dot[0].name, Path::new(""), "", budget()),
            Err(WindowsInventoryError::InvalidName { .. })
        ));

        let mut malformed_utf16 = Vec::new();
        assert!(matches!(
            parse_directory_buffer(
                &record(&[0xD800], 0x55, 0),
                Path::new(""),
                &mut malformed_utf16,
            ),
            Err(WindowsInventoryError::InvalidName { .. })
        ));
    }

    #[test]
    fn member_limit_counts_the_complete_slash_joined_name() {
        let (_, parent_member) =
            checked_name(OsStr::new("one"), Path::new(""), "", budget()).unwrap();
        assert!(matches!(
            checked_name(
                OsStr::new("two"),
                Path::new("one"),
                &parent_member,
                InventoryBudget::new(32, 8, 6, 1024, 1024),
            ),
            Err(WindowsInventoryError::BudgetExceeded {
                limit: InventoryBudgetLimit::MemberUtf8Bytes
            })
        ));
    }

    #[test]
    fn reparse_descendant_is_refused_when_symlink_creation_is_available() {
        let (temporary, root) = fixture();
        let target = temporary.path().join("outside");
        fs::create_dir(&target).unwrap();
        let link = root.canonical_path().join("junction");
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            eprintln!(
                "skipping reparse descendant fixture: symlink creation unavailable (no Developer Mode / elevated privilege)"
            );
            return;
        }
        assert!(matches!(
            enumerate_windows_inventory(&root, budget()),
            Err(WindowsInventoryError::ReparsePoint { .. })
        ));
    }

    #[test]
    fn retained_root_survives_rename_and_route_revalidation_refuses_substitution() {
        let (temporary, root) = fixture();
        let original = root.canonical_path().to_path_buf();
        let before = enumerate_windows_inventory(&root, budget()).unwrap();
        let leaf = before
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("one/two/leaf.txt"))
            .unwrap()
            .clone();

        let moved = temporary.path().join("journal-moved");
        fs::rename(&original, &moved).unwrap();
        let after_rename = enumerate_windows_inventory(&root, budget()).unwrap();
        assert_eq!(
            after_rename
                .entries()
                .iter()
                .map(|entry| entry.relative_path())
                .collect::<Vec<_>>(),
            before
                .entries()
                .iter()
                .map(|entry| entry.relative_path())
                .collect::<Vec<_>>()
        );

        let replaced_directory = moved.join("one");
        let moved_directory = moved.join("one-original");
        let callback_directory = replaced_directory.clone();
        let callback_moved = moved_directory.clone();
        let (result, trace) = trace_inventory_scenario(
            Some((
                WindowsInventoryPrimitive::BeforeDescendantOpen,
                Box::new(move || {
                    fs::rename(&callback_directory, &callback_moved).unwrap();
                    fs::create_dir_all(callback_directory.join("two")).unwrap();
                    fs::write(callback_directory.join("two/leaf.txt"), b"replacement").unwrap();
                }),
            )),
            None,
            || read_windows_inventory_file(&root, &leaf, budget()),
        );
        assert!(trace.barrier_fired);
        assert!(matches!(
            result,
            Err(WindowsInventoryError::IdentityChanged { .. })
        ));
    }

    #[test]
    fn descendant_listing_open_rechecks_the_verified_identity() {
        let (temporary, root) = fixture();
        let original = root.canonical_path().join("one");
        let moved = temporary.path().join("one-original");
        let callback_original = original.clone();
        let callback_moved = moved.clone();
        let (result, trace) = trace_inventory_scenario(
            Some((
                WindowsInventoryPrimitive::BeforeDescendantListingOpen,
                Box::new(move || {
                    fs::rename(&callback_original, &callback_moved)
                        .expect("move verified directory before listing open");
                    fs::create_dir(&callback_original)
                        .expect("create replacement directory before listing open");
                }),
            )),
            None,
            || enumerate_windows_inventory(&root, budget()),
        );
        assert!(trace.barrier_fired);
        assert!(
            trace
                .successful
                .contains(&WindowsInventoryPrimitive::DescendantListingOpen)
        );
        assert!(
            trace
                .attempted
                .contains(&WindowsInventoryPrimitive::DescendantListingIdentityRecheck)
        );
        assert!(
            !trace
                .successful
                .contains(&WindowsInventoryPrimitive::DescendantListingIdentityRecheck)
        );
        assert!(matches!(
            result,
            Err(WindowsInventoryError::IdentityChanged { .. })
        ));
    }

    #[test]
    fn real_ntfs_and_refs_in_place_mutation_refuses_checked_read_without_stale_bytes() {
        for variable in ["JOURNAL_WIN_CI_NTFS_ROOT", "SOLSTONE_JOURNAL_WIN_REFS_ROOT"] {
            let Ok(parent) = std::env::var(variable) else {
                continue;
            };
            let temporary = tempfile::Builder::new()
                .prefix("solstone-journal-in-place-mutation-")
                .tempdir_in(parent)
                .unwrap_or_else(|error| panic!("create {variable} mutation fixture: {error}"));
            let root_path = temporary.path().join("journal");
            fs::create_dir(&root_path)
                .unwrap_or_else(|error| panic!("create {variable} journal fixture: {error}"));
            let file_path = root_path.join("leaf.txt");
            fs::write(&file_path, b"leaf")
                .unwrap_or_else(|error| panic!("write {variable} observed file: {error}"));
            let root = JournalRoot::open(&root_path)
                .unwrap_or_else(|error| panic!("admit {variable} fixture: {error}"));
            let inventory = enumerate_windows_inventory(&root, budget())
                .unwrap_or_else(|error| panic!("inventory {variable} fixture: {error}"));
            let leaf = inventory
                .entries()
                .iter()
                .find(|entry| entry.relative_path() == Path::new("leaf.txt"))
                .expect("fixture inventory contains observed file")
                .clone();
            let (result, trace) = trace_inventory_scenario(
                Some((
                    WindowsInventoryPrimitive::DescendantFileId,
                    Box::new(move || {
                        fs::write(&file_path, b"muta")
                            .expect("mutate observed file in place without rename");
                    }),
                )),
                None,
                || read_windows_inventory_file(&root, &leaf, budget()),
            );
            assert!(trace.barrier_fired, "{variable} mutation barrier fired");
            assert!(matches!(
                result,
                Err(WindowsInventoryError::IdentityChanged { .. })
                    | Err(WindowsInventoryError::NamespaceChanged { .. })
            ));
        }
    }

    #[test]
    fn real_ntfs_and_refs_witness_mutation_controls_skip_without_environment() {
        for variable in ["JOURNAL_WIN_CI_NTFS_ROOT", "SOLSTONE_JOURNAL_WIN_REFS_ROOT"] {
            let Ok(parent) = std::env::var(variable) else {
                continue;
            };
            let temporary = tempfile::Builder::new()
                .prefix("solstone-journal-witness-")
                .tempdir_in(parent)
                .unwrap_or_else(|error| panic!("create {variable} witness fixture: {error}"));
            let root_path = temporary.path().join("journal");
            fs::create_dir(&root_path)
                .unwrap_or_else(|error| panic!("create {variable} journal fixture: {error}"));
            let root = JournalRoot::open(&root_path)
                .unwrap_or_else(|error| panic!("admit {variable} fixture: {error}"));
            let mut witness = NamespaceWitness::arm(&root)
                .unwrap_or_else(|error| panic!("arm {variable} witness: {error}"));
            let changed = root_path.join("concurrent-add");
            let writer = std::thread::spawn(move || fs::write(changed, b"change"));
            let notification = witness.wait_for_notification();
            writer
                .join()
                .expect("witness writer thread")
                .unwrap_or_else(|error| panic!("mutate {variable} fixture: {error}"));
            match notification {
                Ok(bytes) => assert!(bytes > 0, "{variable} mutation must not overflow"),
                Err(WindowsInventoryError::NamespaceChanged { .. }) => {
                    panic!("{variable} one-entry mutation unexpectedly overflowed")
                }
                Err(error) => panic!("wait for {variable} witness: {error}"),
            }
            assert!(matches!(
                witness.check(),
                Err(WindowsInventoryError::NamespaceChanged { .. })
            ));
        }
    }

    #[test]
    fn real_ntfs_and_refs_witness_overflow_controls_skip_without_environment() {
        for variable in ["JOURNAL_WIN_CI_NTFS_ROOT", "SOLSTONE_JOURNAL_WIN_REFS_ROOT"] {
            let Ok(parent) = std::env::var(variable) else {
                continue;
            };
            let temporary = tempfile::Builder::new()
                .prefix("solstone-journal-witness-overflow-")
                .tempdir_in(parent)
                .unwrap_or_else(|error| panic!("create {variable} overflow fixture: {error}"));
            let root_path = temporary.path().join("journal");
            fs::create_dir(&root_path)
                .unwrap_or_else(|error| panic!("create {variable} journal fixture: {error}"));
            let root = JournalRoot::open(&root_path)
                .unwrap_or_else(|error| panic!("admit {variable} fixture: {error}"));
            let mut witness = NamespaceWitness::arm(&root)
                .unwrap_or_else(|error| panic!("arm {variable} witness: {error}"));
            let mut writers = Vec::new();
            for worker in 0..4 {
                let writer_root = root_path.clone();
                writers.push(std::thread::spawn(move || -> io::Result<()> {
                    for entry in 0..512 {
                        fs::write(
                            writer_root.join(format!("overflow-{worker:02}-{entry:04}-payload")),
                            b"change",
                        )?;
                    }
                    Ok(())
                }));
            }
            for writer in writers {
                writer
                    .join()
                    .expect("overflow writer thread")
                    .unwrap_or_else(|error| panic!("mutate {variable} overflow fixture: {error}"));
            }
            match witness.wait_for_notification() {
                Ok(0) | Err(WindowsInventoryError::NamespaceChanged { .. }) => {}
                Ok(bytes) => panic!("{variable} witness did not overflow: {bytes} bytes"),
                Err(error) => panic!("wait for {variable} overflow witness: {error}"),
            }
        }
    }
}
