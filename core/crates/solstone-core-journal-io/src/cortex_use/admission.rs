// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exclusive admit and complete for Cortex active-use records.

use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(any(test, feature = "test-hooks"))]
use std::time::Duration;

use super::lock::{CortexNamespaceLock, CortexNamespaceLockError, acquire_cortex_namespace_lock};
use super::namespace::CortexNamespaceAuthority;
use super::talent_directory_name;
use crate::errors::FlatDirectoryError;

#[cfg(any(test, feature = "test-hooks"))]
use super::lock::acquire_cortex_namespace_lock_with_test_timing;

#[cfg(unix)]
use nix::fcntl::{OFlag, openat, renameat};
#[cfg(unix)]
use nix::sys::stat::Mode;

#[cfg(unix)]
use crate::JournalEntryKind;
#[cfg(unix)]
use crate::errors::AtomicWriteError;
#[cfg(unix)]
use crate::flat_directory::{
    FlatDirectory, create_or_open_flat_directory_bound, open_flat_directory_bound, stat_entry,
};
#[cfg(unix)]
use crate::observation::FlatDirectoryEntry;
#[cfg(unix)]
use crate::write_bytes_exclusive_bound;

#[cfg(windows)]
use std::io::Write;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::io::{AsHandle, AsRawHandle};

#[cfg(windows)]
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GENERIC_READ, GENERIC_WRITE,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FlushFileBuffers, MoveFileExW, SYNCHRONIZE,
};

#[cfg(windows)]
use crate::windows_identity::file_identity;
#[cfg(windows)]
use crate::windows_ntcreate::nt_create_relative;
#[cfg(windows)]
use crate::windows_sync_dir::{
    WindowsFlatDirectory, create_or_open_windows_flat_directory_bound,
    open_windows_flat_directory_bound, validate_windows_regular_handle,
};

/// Opaque identity of one admitted active-use file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CortexUseFileIdentity {
    #[cfg(unix)]
    unix: (libc::dev_t, libc::ino_t),
    #[cfg(windows)]
    windows: crate::windows_identity::WindowsFileIdentity,
    first_row_len: u64,
    first_row_hash: u64,
}

fn hash_first_row(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// One successfully admitted active use, bound to the created file's identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CortexAdmittedUse {
    use_id: String,
    identity: CortexUseFileIdentity,
}

impl CortexAdmittedUse {
    /// Use id stored as `{use_id}_active.jsonl`.
    pub fn use_id(&self) -> &str {
        &self.use_id
    }

    /// Identity captured from the admitted active file.
    pub fn identity(&self) -> CortexUseFileIdentity {
        self.identity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CortexAdmissionClass {
    AlreadyClaimed,
    EmptyFirstRow,
    TalentDirectoryUnsafe,
    TalentDirectoryIo,
    WriteIo,
    ActiveIdentityChanged,
    CompletionIo,
    LockUnsafe,
    LockIdentityChanged,
    LockBusy,
    LockIo,
}

impl CortexAdmissionClass {
    const fn token(self) -> &'static str {
        match self {
            Self::AlreadyClaimed => "cortex_admission_already_claimed",
            Self::EmptyFirstRow => "cortex_admission_empty_first_row",
            Self::TalentDirectoryUnsafe => "cortex_admission_talent_directory_unsafe",
            Self::TalentDirectoryIo => "cortex_admission_talent_directory_io",
            Self::WriteIo => "cortex_admission_write_io",
            Self::ActiveIdentityChanged => "cortex_admission_active_identity_changed",
            Self::CompletionIo => "cortex_admission_completion_io",
            Self::LockUnsafe => "cortex_admission_lock_unsafe",
            Self::LockIdentityChanged => "cortex_admission_lock_identity_changed",
            Self::LockBusy => "cortex_admission_lock_busy",
            Self::LockIo => "cortex_admission_lock_io",
        }
    }
}

/// Bounded failure while admitting or completing a Cortex active use.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CortexAdmissionError {
    class: CortexAdmissionClass,
}

impl CortexAdmissionError {
    const fn new(class: CortexAdmissionClass) -> Self {
        Self { class }
    }

    fn token(self) -> &'static str {
        self.class.token()
    }

    /// True when admit or complete refused because the use id is already claimed.
    pub const fn is_already_claimed(&self) -> bool {
        matches!(self.class, CortexAdmissionClass::AlreadyClaimed)
    }
}

impl fmt::Display for CortexAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

impl fmt::Debug for CortexAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for CortexAdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Ordered checkpoints for one admit or complete call.
///
/// Injected faults map as: `TalentDirectoryOpen` → `talent_directory_io`,
/// `ActiveObserve` / `ActiveContentRead` → `active_identity_changed`,
/// `DestinationProbe` → `talent_directory_io` (existence-probe I/O, never
/// `already_claimed`), `Write` → `write_io`, `Rename` → `completion_io`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexAdmissionPrimitive {
    /// Create-or-open (admit) or open (complete) of the talent directory.
    TalentDirectoryOpen,
    /// Admit active-name probe, or complete active-file identity observe.
    ActiveObserve,
    /// Complete bounded first-row content read.
    ActiveContentRead,
    /// Completed-name existence probe.
    DestinationProbe,
    /// Admit exclusive create of `{use_id}_active.jsonl`.
    Write,
    /// Complete rename of `{use_id}_active.jsonl` → `{use_id}.jsonl`.
    Rename,
}

#[cfg(any(test, feature = "test-hooks"))]
struct CortexAdmissionTraceState {
    attempted: Vec<CortexAdmissionPrimitive>,
    fault: Option<(CortexAdmissionPrimitive, usize)>,
    fault_consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static CORTEX_ADMISSION_TRACE: std::cell::RefCell<Option<CortexAdmissionTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Run an operation with one injected Cortex-admission fault.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_cortex_admission_fault<T>(
    primitive: CortexAdmissionPrimitive,
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    CORTEX_ADMISSION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Cortex admission trace is already active"
        );
        *trace.borrow_mut() = Some(CortexAdmissionTraceState {
            attempted: Vec::new(),
            fault: Some((primitive, ordinal)),
            fault_consumed: false,
        });
    });
    let result = operation();
    let state = CORTEX_ADMISSION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Cortex admission trace remains active")
    });
    (result, state.fault_consumed)
}

fn checkpoint(primitive: CortexAdmissionPrimitive) -> Result<(), CortexAdmissionError> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        checkpoint_traced(primitive)
    }
    #[cfg(not(any(test, feature = "test-hooks")))]
    {
        let _ = primitive;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint_traced(primitive: CortexAdmissionPrimitive) -> Result<(), CortexAdmissionError> {
    let fault = CORTEX_ADMISSION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return false;
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        if state.fault.is_some_and(|(fault_primitive, fault_ordinal)| {
            fault_primitive == primitive && fault_ordinal == ordinal
        }) {
            state.fault = None;
            state.fault_consumed = true;
            true
        } else {
            false
        }
    });
    if fault {
        return Err(CortexAdmissionError::new(match primitive {
            CortexAdmissionPrimitive::TalentDirectoryOpen
            | CortexAdmissionPrimitive::DestinationProbe => CortexAdmissionClass::TalentDirectoryIo,
            CortexAdmissionPrimitive::ActiveObserve
            | CortexAdmissionPrimitive::ActiveContentRead => {
                CortexAdmissionClass::ActiveIdentityChanged
            }
            CortexAdmissionPrimitive::Write => CortexAdmissionClass::WriteIo,
            CortexAdmissionPrimitive::Rename => CortexAdmissionClass::CompletionIo,
        }));
    }
    Ok(())
}

enum LockTiming {
    Default,
    #[cfg(any(test, feature = "test-hooks"))]
    Explicit(Duration, Duration),
}

/// Admit `{use_id}_active.jsonl` exclusively under `authority`'s talents root.
///
/// Unix publishes through [`write_bytes_exclusive_bound`]: the destination name
/// is never visible with partial content. Windows creates the name with
/// `FILE_CREATE` before the write/flush, so a crash or concurrent reader can
/// observe an empty or truncated file under that name.
pub fn admit_active_use(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    first_row: &[u8],
) -> Result<CortexAdmittedUse, CortexAdmissionError> {
    admit_with_timing(
        authority,
        talent_name,
        use_id,
        first_row,
        LockTiming::Default,
    )
}

/// Complete `{use_id}_active.jsonl` to `{use_id}.jsonl` when the active file still
/// has `active_path_identity`.
///
/// Unix renames with descriptor-relative `renameat` after a vacancy probe.
/// Windows uses path-based `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`
/// after handle-based identity and vacancy probes — a weaker guarantee than
/// the Unix bound rename.
pub fn complete_active_use(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    active_path_identity: CortexUseFileIdentity,
) -> Result<(), CortexAdmissionError> {
    complete_with_timing(
        authority,
        talent_name,
        use_id,
        active_path_identity,
        LockTiming::Default,
    )
}

/// Complete `{use_id}_active.jsonl` to `{use_id}.jsonl` without a prior identity.
///
/// Recovery has no admit-time inode: the active leaf must exist as a regular
/// file, and the completed name must be vacant, under the namespace lock.
pub fn recover_active_use(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
) -> Result<(), CortexAdmissionError> {
    recover_with_timing(authority, talent_name, use_id, LockTiming::Default)
}

/// Admit with caller-supplied lock timing.
#[cfg(any(test, feature = "test-hooks"))]
pub fn admit_active_use_with_test_timing(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    first_row: &[u8],
    timeout: Duration,
    poll_interval: Duration,
) -> Result<CortexAdmittedUse, CortexAdmissionError> {
    admit_with_timing(
        authority,
        talent_name,
        use_id,
        first_row,
        LockTiming::Explicit(timeout, poll_interval),
    )
}

/// Complete with caller-supplied lock timing.
#[cfg(any(test, feature = "test-hooks"))]
pub fn complete_active_use_with_test_timing(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    active_path_identity: CortexUseFileIdentity,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(), CortexAdmissionError> {
    complete_with_timing(
        authority,
        talent_name,
        use_id,
        active_path_identity,
        LockTiming::Explicit(timeout, poll_interval),
    )
}

/// Recover with caller-supplied lock timing.
#[cfg(any(test, feature = "test-hooks"))]
pub fn recover_active_use_with_test_timing(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(), CortexAdmissionError> {
    recover_with_timing(
        authority,
        talent_name,
        use_id,
        LockTiming::Explicit(timeout, poll_interval),
    )
}

fn admit_with_timing(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    first_row: &[u8],
    timing: LockTiming,
) -> Result<CortexAdmittedUse, CortexAdmissionError> {
    if first_row.is_empty() {
        return Err(CortexAdmissionError::new(
            CortexAdmissionClass::EmptyFirstRow,
        ));
    }
    let _lock = acquire_lock(authority, timing)?;
    let projected = talent_directory_name(talent_name);
    checkpoint(CortexAdmissionPrimitive::TalentDirectoryOpen)?;
    let talent = create_talent_directory(authority, &projected)?;
    let active = active_leaf(use_id);
    let completed = completed_leaf(use_id);
    checkpoint(CortexAdmissionPrimitive::ActiveObserve)?;
    if probe_exists(&talent, OsStr::new(&active))? {
        return Err(CortexAdmissionError::new(
            CortexAdmissionClass::AlreadyClaimed,
        ));
    }
    checkpoint(CortexAdmissionPrimitive::DestinationProbe)?;
    if probe_exists(&talent, OsStr::new(&completed))? {
        return Err(CortexAdmissionError::new(
            CortexAdmissionClass::AlreadyClaimed,
        ));
    }
    checkpoint(CortexAdmissionPrimitive::Write)?;
    let identity = exclusive_write(&talent, OsStr::new(&active), first_row)?;
    Ok(CortexAdmittedUse {
        use_id: use_id.to_owned(),
        identity,
    })
}

fn complete_with_timing(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    active_path_identity: CortexUseFileIdentity,
    timing: LockTiming,
) -> Result<(), CortexAdmissionError> {
    let _lock = acquire_lock(authority, timing)?;
    let projected = talent_directory_name(talent_name);
    checkpoint(CortexAdmissionPrimitive::TalentDirectoryOpen)?;
    let talent = open_talent_directory(authority, &projected)?;
    let active = active_leaf(use_id);
    let completed = completed_leaf(use_id);
    checkpoint(CortexAdmissionPrimitive::ActiveObserve)?;
    observe_active(&talent, OsStr::new(&active), active_path_identity)?;
    checkpoint(CortexAdmissionPrimitive::ActiveContentRead)?;
    verify_active_content(
        &talent,
        OsStr::new(&active),
        active_path_identity.first_row_len,
        active_path_identity.first_row_hash,
    )?;
    checkpoint(CortexAdmissionPrimitive::DestinationProbe)?;
    if probe_exists(&talent, OsStr::new(&completed))? {
        return Err(CortexAdmissionError::new(
            CortexAdmissionClass::AlreadyClaimed,
        ));
    }
    checkpoint(CortexAdmissionPrimitive::Rename)?;
    rename_active_to_completed(&talent, OsStr::new(&active), OsStr::new(&completed))
}

fn recover_with_timing(
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    use_id: &str,
    timing: LockTiming,
) -> Result<(), CortexAdmissionError> {
    let _lock = acquire_lock(authority, timing)?;
    let projected = talent_directory_name(talent_name);
    checkpoint(CortexAdmissionPrimitive::TalentDirectoryOpen)?;
    let talent = open_talent_directory(authority, &projected)?;
    let active = active_leaf(use_id);
    let completed = completed_leaf(use_id);
    checkpoint(CortexAdmissionPrimitive::ActiveObserve)?;
    observe_active_present(&talent, OsStr::new(&active))?;
    checkpoint(CortexAdmissionPrimitive::DestinationProbe)?;
    if probe_exists(&talent, OsStr::new(&completed))? {
        return Err(CortexAdmissionError::new(
            CortexAdmissionClass::AlreadyClaimed,
        ));
    }
    checkpoint(CortexAdmissionPrimitive::Rename)?;
    rename_active_to_completed(&talent, OsStr::new(&active), OsStr::new(&completed))
}

fn acquire_lock(
    authority: &CortexNamespaceAuthority,
    timing: LockTiming,
) -> Result<CortexNamespaceLock, CortexAdmissionError> {
    let result = match timing {
        LockTiming::Default => acquire_cortex_namespace_lock(authority),
        #[cfg(any(test, feature = "test-hooks"))]
        LockTiming::Explicit(timeout, poll_interval) => {
            acquire_cortex_namespace_lock_with_test_timing(authority, timeout, poll_interval)
        }
    };
    result.map_err(map_lock_error)
}

fn map_lock_error(error: CortexNamespaceLockError) -> CortexAdmissionError {
    let token = error.to_string();
    let suffix = token
        .strip_prefix("cortex_namespace_lock_")
        .expect("CortexNamespaceLockError Display is cortex_namespace_lock_{class}");
    let class = match suffix {
        "unsafe" => CortexAdmissionClass::LockUnsafe,
        "identity_changed" => CortexAdmissionClass::LockIdentityChanged,
        "busy" => CortexAdmissionClass::LockBusy,
        "io" => CortexAdmissionClass::LockIo,
        _ => panic!("unknown CortexNamespaceLockError class {suffix}"),
    };
    CortexAdmissionError::new(class)
}

fn map_talent_directory_error(error: FlatDirectoryError) -> CortexAdmissionError {
    let class = match error {
        FlatDirectoryError::InvalidRelativePath { .. }
        | FlatDirectoryError::InvalidName { .. }
        | FlatDirectoryError::NotDirectory { .. }
        | FlatDirectoryError::SymlinkRefused { .. }
        | FlatDirectoryError::NotRegular { .. }
        | FlatDirectoryError::SizeLimitExceeded { .. } => {
            CortexAdmissionClass::TalentDirectoryUnsafe
        }
        FlatDirectoryError::IdentityChanged { .. }
        | FlatDirectoryError::EnumerationChanged { .. } => CortexAdmissionClass::TalentDirectoryIo,
        FlatDirectoryError::Io { source, .. } => match source.kind() {
            io::ErrorKind::AlreadyExists
            | io::ErrorKind::NotADirectory
            | io::ErrorKind::IsADirectory => CortexAdmissionClass::TalentDirectoryUnsafe,
            _ => CortexAdmissionClass::TalentDirectoryIo,
        },
    };
    CortexAdmissionError::new(class)
}

fn active_leaf(use_id: &str) -> String {
    format!("{use_id}_active.jsonl")
}

fn completed_leaf(use_id: &str) -> String {
    format!("{use_id}.jsonl")
}

fn row_bytes(first_row: &[u8]) -> Vec<u8> {
    let mut contents = Vec::with_capacity(first_row.len() + 1);
    contents.extend_from_slice(first_row);
    contents.push(b'\n');
    contents
}

#[cfg(unix)]
fn create_talent_directory(
    authority: &CortexNamespaceAuthority,
    projected: &str,
) -> Result<FlatDirectory, CortexAdmissionError> {
    create_or_open_flat_directory_bound(
        authority.talents(),
        OsStr::new(projected),
        0o700,
        authority.talents().diagnostic_path(),
    )
    .map_err(map_talent_directory_error)
}

#[cfg(unix)]
fn open_talent_directory(
    authority: &CortexNamespaceAuthority,
    projected: &str,
) -> Result<FlatDirectory, CortexAdmissionError> {
    match open_flat_directory_bound(
        authority.talents(),
        OsStr::new(projected),
        authority.talents().diagnostic_path(),
    ) {
        Ok(Some(directory)) => Ok(directory),
        Ok(None) => Err(CortexAdmissionError::new(
            CortexAdmissionClass::TalentDirectoryIo,
        )),
        Err(error) => Err(map_talent_directory_error(error)),
    }
}

#[cfg(unix)]
fn probe_exists(talent: &FlatDirectory, name: &OsStr) -> Result<bool, CortexAdmissionError> {
    match stat_entry(talent, name) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(_) => Err(CortexAdmissionError::new(
            CortexAdmissionClass::TalentDirectoryIo,
        )),
    }
}

#[cfg(unix)]
fn exclusive_write(
    talent: &FlatDirectory,
    name: &OsStr,
    first_row: &[u8],
) -> Result<CortexUseFileIdentity, CortexAdmissionError> {
    match write_bytes_exclusive_bound(talent, name, &row_bytes(first_row), 0o600) {
        Ok(()) => {}
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            return Err(CortexAdmissionError::new(
                CortexAdmissionClass::AlreadyClaimed,
            ));
        }
        Err(_) => {
            return Err(CortexAdmissionError::new(CortexAdmissionClass::WriteIo));
        }
    }
    match stat_entry(talent, name) {
        Ok(Some(entry)) if entry.kind == JournalEntryKind::RegularFile => {
            Ok(CortexUseFileIdentity {
                unix: identity_from_entry(&entry),
                first_row_len: first_row.len() as u64,
                first_row_hash: hash_first_row(first_row),
            })
        }
        _ => Err(CortexAdmissionError::new(CortexAdmissionClass::WriteIo)),
    }
}

#[cfg(unix)]
fn observe_active(
    talent: &FlatDirectory,
    name: &OsStr,
    expected: CortexUseFileIdentity,
) -> Result<(), CortexAdmissionError> {
    match stat_entry(talent, name) {
        Ok(Some(entry))
            if entry.kind == JournalEntryKind::RegularFile
                && identity_from_entry(&entry) == expected.unix =>
        {
            Ok(())
        }
        _ => Err(CortexAdmissionError::new(
            CortexAdmissionClass::ActiveIdentityChanged,
        )),
    }
}

#[cfg(unix)]
fn observe_active_present(
    talent: &FlatDirectory,
    name: &OsStr,
) -> Result<(), CortexAdmissionError> {
    match stat_entry(talent, name) {
        Ok(Some(entry)) if entry.kind == JournalEntryKind::RegularFile => Ok(()),
        _ => Err(CortexAdmissionError::new(
            CortexAdmissionClass::ActiveIdentityChanged,
        )),
    }
}

#[cfg(unix)]
fn rename_active_to_completed(
    talent: &FlatDirectory,
    active: &OsStr,
    completed: &OsStr,
) -> Result<(), CortexAdmissionError> {
    renameat(talent, active, talent, completed)
        .map_err(|_| CortexAdmissionError::new(CortexAdmissionClass::CompletionIo))
}

#[cfg(unix)]
fn identity_from_entry(entry: &FlatDirectoryEntry) -> (libc::dev_t, libc::ino_t) {
    (entry.device as libc::dev_t, entry.inode as libc::ino_t)
}

#[cfg(unix)]
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

#[cfg(unix)]
fn verify_active_content(
    talent: &FlatDirectory,
    name: &OsStr,
    expected_len: u64,
    expected_hash: u64,
) -> Result<(), CortexAdmissionError> {
    let changed = CortexAdmissionError::new(CortexAdmissionClass::ActiveIdentityChanged);
    let descriptor = match openat(talent, name, FILE_FLAGS, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(_) => return Err(changed),
    };
    let mut file = File::from(descriptor);
    let mut buf = vec![0_u8; expected_len as usize];
    if file.seek(SeekFrom::Start(0)).is_err() || file.read_exact(&mut buf).is_err() {
        return Err(changed);
    }
    (hash_first_row(&buf) == expected_hash)
        .then_some(())
        .ok_or(changed)
}

#[cfg(windows)]
fn create_talent_directory(
    authority: &CortexNamespaceAuthority,
    projected: &str,
) -> Result<WindowsFlatDirectory, CortexAdmissionError> {
    create_or_open_windows_flat_directory_bound(
        authority.talents(),
        OsStr::new(projected),
        authority.talents().diagnostic_path(),
    )
    .map_err(map_talent_directory_error)
}

#[cfg(windows)]
fn open_talent_directory(
    authority: &CortexNamespaceAuthority,
    projected: &str,
) -> Result<WindowsFlatDirectory, CortexAdmissionError> {
    match open_windows_flat_directory_bound(
        authority.talents(),
        OsStr::new(projected),
        authority.talents().diagnostic_path(),
    ) {
        Ok(Some(directory)) => Ok(directory),
        Ok(None) => Err(CortexAdmissionError::new(
            CortexAdmissionClass::TalentDirectoryIo,
        )),
        Err(error) => Err(map_talent_directory_error(error)),
    }
}

#[cfg(windows)]
fn probe_exists(talent: &WindowsFlatDirectory, name: &OsStr) -> Result<bool, CortexAdmissionError> {
    match nt_create_relative(
        talent.as_handle().as_raw_handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            Ok(false)
        }
        Err(_) => Err(CortexAdmissionError::new(
            CortexAdmissionClass::TalentDirectoryIo,
        )),
    }
}

#[cfg(windows)]
fn exclusive_write(
    talent: &WindowsFlatDirectory,
    name: &OsStr,
    first_row: &[u8],
) -> Result<CortexUseFileIdentity, CortexAdmissionError> {
    let handle = match nt_create_relative(
        talent.as_handle().as_raw_handle(),
        name,
        GENERIC_WRITE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_EXISTS as i32) => {
            return Err(CortexAdmissionError::new(
                CortexAdmissionClass::AlreadyClaimed,
            ));
        }
        Err(_) => return Err(CortexAdmissionError::new(CortexAdmissionClass::WriteIo)),
    };
    let mut file = File::from(handle);
    let path = talent.diagnostic_entry_path(name);
    if validate_windows_regular_handle(file.as_raw_handle(), &path).is_err() {
        return Err(CortexAdmissionError::new(CortexAdmissionClass::WriteIo));
    }
    if file.write_all(&row_bytes(first_row)).is_err() {
        return Err(CortexAdmissionError::new(CortexAdmissionClass::WriteIo));
    }
    // SAFETY: `file` is a live write handle for the duration of this synchronous flush.
    #[allow(unsafe_code)]
    let flushed = unsafe { FlushFileBuffers(file.as_raw_handle()) };
    if flushed == 0 {
        return Err(CortexAdmissionError::new(CortexAdmissionClass::WriteIo));
    }
    let identity = file_identity(file.as_raw_handle())
        .map_err(|_| CortexAdmissionError::new(CortexAdmissionClass::WriteIo))?;
    Ok(CortexUseFileIdentity {
        windows: identity,
        first_row_len: first_row.len() as u64,
        first_row_hash: hash_first_row(first_row),
    })
}

#[cfg(windows)]
fn observe_active(
    talent: &WindowsFlatDirectory,
    name: &OsStr,
    expected: CortexUseFileIdentity,
) -> Result<(), CortexAdmissionError> {
    let handle = match nt_create_relative(
        talent.as_handle().as_raw_handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(_) => {
            return Err(CortexAdmissionError::new(
                CortexAdmissionClass::ActiveIdentityChanged,
            ));
        }
    };
    let path = talent.diagnostic_entry_path(name);
    let identity = match validate_windows_regular_handle(handle.as_raw_handle(), &path) {
        Ok(identity) => identity,
        Err(_) => {
            return Err(CortexAdmissionError::new(
                CortexAdmissionClass::ActiveIdentityChanged,
            ));
        }
    };
    (identity == expected.windows)
        .then_some(())
        .ok_or(CortexAdmissionError::new(
            CortexAdmissionClass::ActiveIdentityChanged,
        ))
}

#[cfg(windows)]
fn observe_active_present(
    talent: &WindowsFlatDirectory,
    name: &OsStr,
) -> Result<(), CortexAdmissionError> {
    let handle = match nt_create_relative(
        talent.as_handle().as_raw_handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(_) => {
            return Err(CortexAdmissionError::new(
                CortexAdmissionClass::ActiveIdentityChanged,
            ));
        }
    };
    let path = talent.diagnostic_entry_path(name);
    validate_windows_regular_handle(handle.as_raw_handle(), &path)
        .map(|_| ())
        .map_err(|_| CortexAdmissionError::new(CortexAdmissionClass::ActiveIdentityChanged))
}

#[cfg(windows)]
fn verify_active_content(
    talent: &WindowsFlatDirectory,
    name: &OsStr,
    expected_len: u64,
    expected_hash: u64,
) -> Result<(), CortexAdmissionError> {
    let changed = CortexAdmissionError::new(CortexAdmissionClass::ActiveIdentityChanged);
    let handle = match nt_create_relative(
        talent.as_handle().as_raw_handle(),
        name,
        GENERIC_READ | FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(_) => return Err(changed),
    };
    let path = talent.diagnostic_entry_path(name);
    if validate_windows_regular_handle(handle.as_raw_handle(), &path).is_err() {
        return Err(changed);
    }
    let mut file = File::from(handle);
    let mut buf = vec![0_u8; expected_len as usize];
    if file.seek(SeekFrom::Start(0)).is_err() || file.read_exact(&mut buf).is_err() {
        return Err(changed);
    }
    (hash_first_row(&buf) == expected_hash)
        .then_some(())
        .ok_or(changed)
}

#[cfg(windows)]
fn rename_active_to_completed(
    talent: &WindowsFlatDirectory,
    active: &OsStr,
    completed: &OsStr,
) -> Result<(), CortexAdmissionError> {
    let source = wide_path(&talent.diagnostic_entry_path(active));
    let destination = wide_path(&talent.diagnostic_entry_path(completed));
    // SAFETY: both buffers are NUL-terminated and live for this synchronous call.
    #[allow(unsafe_code)]
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) };
    (result != 0).then_some(()).ok_or(CortexAdmissionError::new(
        CortexAdmissionClass::CompletionIo,
    ))
}

#[cfg(windows)]
fn wide_path(path: &std::path::Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ffi::OsStr;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::*;
    use crate::cortex_use::create_or_admit_cortex_namespace;
    use crate::journal_root::JournalRoot;

    const ZERO: Duration = Duration::ZERO;
    const TALENT: &str = "conversation";
    const USE: &str = "one";

    fn temp() -> tempfile::TempDir {
        #[cfg(unix)]
        {
            tempfile::tempdir_in("/var/tmp").unwrap()
        }
        #[cfg(windows)]
        {
            tempfile::TempDir::new().unwrap()
        }
    }

    fn authority(root: &Path) -> CortexNamespaceAuthority {
        create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap()).unwrap()
    }

    fn first_row() -> Vec<u8> {
        br#"{"name":"conversation","use_id":"one"}"#.to_vec()
    }

    fn active_path(root: &Path) -> PathBuf {
        root.join("talents")
            .join(talent_directory_name(TALENT))
            .join(active_leaf(USE))
    }

    fn completed_path(root: &Path) -> PathBuf {
        root.join("talents")
            .join(talent_directory_name(TALENT))
            .join(completed_leaf(USE))
    }

    fn talent_dir(root: &Path) -> PathBuf {
        root.join("talents").join(talent_directory_name(TALENT))
    }

    fn expect_token(error: CortexAdmissionError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    #[test]
    fn completed_name_collision_is_already_claimed() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        assert_eq!(admitted.use_id(), USE);
        fs::rename(active_path(root), completed_path(root)).unwrap();
        let error =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap_err();
        expect_token(error, "cortex_admission_already_claimed");
        assert!(error.is_already_claimed());
    }

    #[test]
    fn recover_active_use_renames_an_orphan_and_refuses_an_occupied_destination() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
            .unwrap();
        recover_active_use_with_test_timing(&authority, TALENT, USE, ZERO, ZERO).unwrap();
        assert!(!active_path(root).exists());
        assert_eq!(fs::read(completed_path(root)).unwrap(), {
            let mut expected = first_row();
            expected.push(b'\n');
            expected
        });
        fs::write(active_path(root), b"replacement\n").unwrap();
        let before_active = fs::read(active_path(root)).unwrap();
        let before_completed = fs::read(completed_path(root)).unwrap();
        let error =
            recover_active_use_with_test_timing(&authority, TALENT, USE, ZERO, ZERO).unwrap_err();
        assert!(error.is_already_claimed());
        assert_eq!(fs::read(active_path(root)).unwrap(), before_active);
        assert_eq!(fs::read(completed_path(root)).unwrap(), before_completed);
    }

    #[test]
    fn admit_writes_the_first_row_and_newline() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
            .unwrap();
        let mut expected = first_row();
        expected.push(b'\n');
        assert_eq!(fs::read(active_path(root)).unwrap(), expected);
    }

    #[test]
    fn destination_probe_fault_is_talent_directory_io_and_does_not_write() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let (result, consumed) =
            run_with_cortex_admission_fault(CortexAdmissionPrimitive::DestinationProbe, 1, || {
                admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
            });
        assert!(consumed);
        expect_token(result.unwrap_err(), "cortex_admission_talent_directory_io");
        assert!(!active_path(root).exists());
    }

    #[test]
    fn stale_identity_refuses_complete() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        let path = active_path(root);
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement\n").unwrap();
        let error = complete_active_use_with_test_timing(
            &authority,
            TALENT,
            USE,
            admitted.identity(),
            ZERO,
            ZERO,
        )
        .unwrap_err();
        expect_token(error, "cortex_admission_active_identity_changed");
        assert!(path.exists());
        assert!(!completed_path(root).exists());
    }

    #[test]
    fn renamed_replacement_different_inode_refuses_complete() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        let path = active_path(root);
        let staging = talent_dir(root).join("replacement_staging");
        fs::write(&staging, b"other-first-row\n").unwrap();
        fs::rename(&staging, &path).unwrap();
        let error = complete_active_use_with_test_timing(
            &authority,
            TALENT,
            USE,
            admitted.identity(),
            ZERO,
            ZERO,
        )
        .unwrap_err();
        expect_token(error, "cortex_admission_active_identity_changed");
        assert!(path.exists());
        assert!(!completed_path(root).exists());
    }

    #[test]
    fn overwritten_same_inode_content_refuses_complete() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        let path = active_path(root);
        let mut replacement = first_row();
        replacement[2] = b'X';
        replacement.push(b'\n');
        fs::write(&path, &replacement).unwrap();
        let error = complete_active_use_with_test_timing(
            &authority,
            TALENT,
            USE,
            admitted.identity(),
            ZERO,
            ZERO,
        )
        .unwrap_err();
        expect_token(error, "cortex_admission_active_identity_changed");
        assert!(path.exists());
        assert!(!completed_path(root).exists());
    }

    #[test]
    fn appended_progress_events_still_complete() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(active_path(root))
            .unwrap()
            .write_all(b"{\"event\":\"progress\"}\n{\"event\":\"tail\"}\n")
            .unwrap();
        complete_active_use_with_test_timing(
            &authority,
            TALENT,
            USE,
            admitted.identity(),
            ZERO,
            ZERO,
        )
        .unwrap();
        assert!(!active_path(root).exists());
        assert!(completed_path(root).exists());
    }

    #[test]
    fn complete_active_content_read_fault_is_identity_changed() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        let before = fs::read(active_path(root)).unwrap();
        let (result, consumed) =
            run_with_cortex_admission_fault(CortexAdmissionPrimitive::ActiveContentRead, 1, || {
                complete_active_use_with_test_timing(
                    &authority,
                    TALENT,
                    USE,
                    admitted.identity(),
                    ZERO,
                    ZERO,
                )
            });
        assert!(consumed);
        expect_token(
            result.unwrap_err(),
            "cortex_admission_active_identity_changed",
        );
        assert_eq!(fs::read(active_path(root)).unwrap(), before);
        assert!(!completed_path(root).exists());
    }

    #[test]
    fn empty_first_row_is_refused() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let error = admit_active_use_with_test_timing(&authority, TALENT, USE, &[], ZERO, ZERO)
            .unwrap_err();
        expect_token(error, "cortex_admission_empty_first_row");
        assert!(!active_path(root).exists());
        assert!(!talent_dir(root).exists());
    }

    #[test]
    fn empty_first_row_wins_over_already_claimed_without_writes() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
            .unwrap();
        fs::rename(active_path(root), completed_path(root)).unwrap();
        let error = admit_active_use_with_test_timing(&authority, TALENT, USE, &[], ZERO, ZERO)
            .unwrap_err();
        expect_token(error, "cortex_admission_empty_first_row");
        assert!(completed_path(root).exists());
        assert!(!active_path(root).exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_active_leaf_refuses_verify_active_content() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        let path = active_path(root);
        let target = talent_dir(root).join("symlink_target");
        fs::write(&target, fs::read(&path).unwrap()).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let talent = open_talent_directory(&authority, &talent_directory_name(TALENT)).unwrap();
        let identity = admitted.identity();
        let error = verify_active_content(
            &talent,
            OsStr::new(&active_leaf(USE)),
            identity.first_row_len,
            identity.first_row_hash,
        )
        .unwrap_err();
        expect_token(error, "cortex_admission_active_identity_changed");
    }

    #[cfg(windows)]
    #[test]
    fn symlink_active_leaf_refuses_verify_active_content() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        let path = active_path(root);
        let target = talent_dir(root).join("symlink_target");
        fs::write(&target, fs::read(&path).unwrap()).unwrap();
        fs::remove_file(&path).unwrap();
        if std::os::windows::fs::symlink_file(&target, &path).is_err() {
            eprintln!(
                "skipping symlink active-leaf fixture: symlink creation unavailable (no Developer Mode / elevated privilege)"
            );
            return;
        }
        let talent = open_talent_directory(&authority, &talent_directory_name(TALENT)).unwrap();
        let identity = admitted.identity();
        let error = verify_active_content(
            &talent,
            OsStr::new(&active_leaf(USE)),
            identity.first_row_len,
            identity.first_row_hash,
        )
        .unwrap_err();
        expect_token(error, "cortex_admission_active_identity_changed");
    }

    #[test]
    fn admit_does_not_trigger_active_content_read_checkpoint() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let (result, consumed) =
            run_with_cortex_admission_fault(CortexAdmissionPrimitive::ActiveContentRead, 1, || {
                admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
            });
        assert!(!consumed);
        result.unwrap();
        assert!(active_path(root).exists());
    }

    #[test]
    fn rename_fault_leaves_the_active_file_unchanged() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let admitted =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap();
        let before = fs::read(active_path(root)).unwrap();
        let (result, consumed) =
            run_with_cortex_admission_fault(CortexAdmissionPrimitive::Rename, 1, || {
                complete_active_use_with_test_timing(
                    &authority,
                    TALENT,
                    USE,
                    admitted.identity(),
                    ZERO,
                    ZERO,
                )
            });
        assert!(consumed);
        expect_token(result.unwrap_err(), "cortex_admission_completion_io");
        assert_eq!(fs::read(active_path(root)).unwrap(), before);
        assert!(!completed_path(root).exists());
    }

    #[test]
    fn held_lock_is_busy_then_succeeds_after_drop() {
        let temporary = temp();
        let root = temporary.path();
        let authority = authority(root);
        let held = acquire_cortex_namespace_lock_with_test_timing(&authority, ZERO, ZERO).unwrap();
        let error =
            admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
                .unwrap_err();
        expect_token(error, "cortex_admission_lock_busy");
        assert!(!active_path(root).exists());
        drop(held);
        admit_active_use_with_test_timing(&authority, TALENT, USE, &first_row(), ZERO, ZERO)
            .unwrap();
        assert!(active_path(root).exists());
    }

    #[test]
    fn no_external_production_callers() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let needles = [
            "admit_active_use",
            "complete_active_use",
            "build_recovery_catalog",
            "read_cortex_use_completed_request",
        ];
        let mut hits = Vec::new();
        for crate_dir in fs::read_dir(&crates).unwrap() {
            let crate_dir = crate_dir.unwrap().path();
            for tree in ["src", "tests"] {
                let root = crate_dir.join(tree);
                if root.is_dir() {
                    collect_hits(&root, &needles, &mut hits);
                }
            }
        }
        assert!(hits.is_empty(), "external callers of F4/F5 APIs: {hits:?}");
    }

    fn is_cortex_storage(path: &Path) -> bool {
        let parts: Vec<_> = path.iter().rev().take(3).collect();
        parts.len() == 3
            && parts[0] == "storage.rs"
            && parts[1] == "src"
            && parts[2] == "solstone-core-cortex"
    }

    fn collect_hits(dir: &Path, needles: &[&str], hits: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "cortex_use") {
                    continue;
                }
                collect_hits(&path, needles, hits);
                continue;
            }
            if is_cortex_storage(&path) {
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for needle in needles {
                if text.contains(needle) {
                    hits.push(format!("{}:{needle}", path.display()));
                }
            }
        }
    }

    #[test]
    fn mapper_does_not_leak_failure_details() {
        let sentinel = "fixture-sentinel-must-not-leak";
        let path = PathBuf::from(sentinel);
        for error in [
            FlatDirectoryError::NotDirectory { path: path.clone() },
            FlatDirectoryError::Io {
                operation: sentinel,
                path,
                source: io::Error::other(sentinel),
            },
        ] {
            let rendered = map_talent_directory_error(error).to_string();
            assert!(!rendered.contains(sentinel));
            assert!(!format!("{rendered:?}").contains(sentinel));
        }
    }
}
