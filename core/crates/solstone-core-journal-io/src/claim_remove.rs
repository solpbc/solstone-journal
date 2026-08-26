// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Identity-safe claimed removal of one observed direct file entry.

use std::ffi::{CString, OsStr};
use std::io;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::ffi::OsStrExt;

use nix::errno::Errno;
use nix::unistd::{UnlinkatFlags, unlinkat};

use crate::entry::sync_dir_bound;
use crate::errors::{
    ClaimDurability, ClaimRemovalError, ClaimRemovalOutcome, ClaimUnchangedReason,
    FlatDirectoryError, IdentityChangeDisposition, NoReplacePrimitive,
};
use crate::flat_directory::{
    FileObservation, FlatDirectory, FlatDirectoryEntry, read_observed_file_unchecked,
    same_entry_metadata, same_observation, stat_entry,
};
use crate::name_admission::{ClaimName, check_portable_component};

/// Ordered checkpoints used by claim/removal fault and pause tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimRemovalPrimitive {
    /// Immediately before the authoritative claim attempt.
    BeforeClaim,
    /// The no-replace original-to-claim rename.
    ClaimRename,
    /// Immediately after a successful claim rename.
    AfterClaim,
    /// Immediately before checking the claimed object.
    BeforeInspection,
    /// Immediately before unlinking a matching claim.
    BeforeUnlink,
    /// The claimed-entry unlink.
    Unlink,
    /// Immediately before attempting restoration.
    BeforeRestore,
    /// The no-replace claim-to-original restore rename.
    RestoreRename,
    /// The directory durability sync.
    DirectorySync,
}

#[cfg(any(test, feature = "test-hooks"))]
struct ClaimTraceState {
    attempted: Vec<ClaimRemovalPrimitive>,
    fault: Option<ClaimFault>,
    fault_consumed: bool,
    barriers: Vec<ClaimBarrier>,
    barriers_fired: usize,
}

#[cfg(any(test, feature = "test-hooks"))]
struct ClaimFault {
    primitive: ClaimRemovalPrimitive,
    ordinal: usize,
    error: Errno,
}

#[cfg(any(test, feature = "test-hooks"))]
struct ClaimBarrier {
    primitive: ClaimRemovalPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static CLAIM_TRACE: std::cell::RefCell<Option<ClaimTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Run `op` with one injected errno at an ordinal claim/removal checkpoint.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_claim_removal_fault<T>(
    primitive: ClaimRemovalPrimitive,
    ordinal: usize,
    raw_errno: i32,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    CLAIM_TRACE.with(|trace| {
        assert!(trace.borrow().is_none(), "claim trace is already active");
        *trace.borrow_mut() = Some(ClaimTraceState {
            attempted: Vec::new(),
            fault: Some(ClaimFault {
                primitive,
                ordinal,
                error: Errno::from_raw(raw_errno),
            }),
            fault_consumed: false,
            barriers: Vec::new(),
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = CLAIM_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("claim trace remains active")
    });
    (result, state.fault_consumed)
}

/// Run `op` with one deterministic claim/removal barrier callback.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_claim_removal_barrier<T>(
    primitive: ClaimRemovalPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    CLAIM_TRACE.with(|trace| {
        assert!(trace.borrow().is_none(), "claim trace is already active");
        *trace.borrow_mut() = Some(ClaimTraceState {
            attempted: Vec::new(),
            fault: None,
            fault_consumed: false,
            barriers: vec![ClaimBarrier {
                primitive,
                ordinal,
                callback: Box::new(callback),
            }],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = CLAIM_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("claim trace remains active")
    });
    (result, state.barriers_fired == 1)
}

/// Run `op` with two deterministic claim/removal barrier callbacks.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_two_claim_removal_barriers<T>(
    first_primitive: ClaimRemovalPrimitive,
    first_ordinal: usize,
    first_callback: impl FnOnce() + 'static,
    second_primitive: ClaimRemovalPrimitive,
    second_ordinal: usize,
    second_callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, usize) {
    CLAIM_TRACE.with(|trace| {
        assert!(trace.borrow().is_none(), "claim trace is already active");
        *trace.borrow_mut() = Some(ClaimTraceState {
            attempted: Vec::new(),
            fault: None,
            fault_consumed: false,
            barriers: vec![
                ClaimBarrier {
                    primitive: first_primitive,
                    ordinal: first_ordinal,
                    callback: Box::new(first_callback),
                },
                ClaimBarrier {
                    primitive: second_primitive,
                    ordinal: second_ordinal,
                    callback: Box::new(second_callback),
                },
            ],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = CLAIM_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("claim trace remains active")
    });
    (result, state.barriers_fired)
}

#[cfg(test)]
fn run_with_claim_removal_fault_and_barrier<T>(
    fault_primitive: ClaimRemovalPrimitive,
    fault_ordinal: usize,
    raw_errno: i32,
    barrier_primitive: ClaimRemovalPrimitive,
    barrier_ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool, bool) {
    CLAIM_TRACE.with(|trace| {
        assert!(trace.borrow().is_none(), "claim trace is already active");
        *trace.borrow_mut() = Some(ClaimTraceState {
            attempted: Vec::new(),
            fault: Some(ClaimFault {
                primitive: fault_primitive,
                ordinal: fault_ordinal,
                error: Errno::from_raw(raw_errno),
            }),
            fault_consumed: false,
            barriers: vec![ClaimBarrier {
                primitive: barrier_primitive,
                ordinal: barrier_ordinal,
                callback: Box::new(callback),
            }],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = CLAIM_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("claim trace remains active")
    });
    (result, state.fault_consumed, state.barriers_fired == 1)
}

/// Claim the exact observed file under `claim`, then remove or restore it safely.
pub fn claim_and_remove_observed(
    directory: &FlatDirectory,
    original: &OsStr,
    prior: &FileObservation,
    claim: &ClaimName,
) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
    validate_original_name(original)?;
    if original != prior.entry.name.as_os_str() {
        return Err(ClaimRemovalError::ObservationNameMismatch {
            original: original.to_os_string(),
            observed: prior.entry.name.clone(),
        });
    }

    let original_stat = stat_entry(directory, original).map_err(preflight_error)?;
    let claim_stat = stat_entry(directory, claim.as_os_str()).map_err(preflight_error)?;
    if let (Some(original_stat), Some(claim_stat)) = (&original_stat, &claim_stat)
        && original_stat.device == claim_stat.device
        && original_stat.inode == claim_stat.inode
    {
        return Err(ClaimRemovalError::AliasedClaimName {
            original: original.to_os_string(),
            claim: claim.clone(),
            device: original_stat.device,
            inode: original_stat.inode,
        });
    }
    let Some(original_stat) = original_stat else {
        return Ok(unknown_location());
    };
    if !same_entry_metadata(&original_stat, &prior.entry) {
        return Ok(unknown_location());
    }
    match observation_state(directory, original, prior).map_err(preflight_error)? {
        ObservationState::Matches => {}
        ObservationState::Absent | ObservationState::Different => return Ok(unknown_location()),
    }
    if claim_stat.is_some() {
        return Ok(unchanged(ClaimUnchangedReason::ClaimNameOccupied));
    }

    checkpoint(ClaimRemovalPrimitive::BeforeClaim).map_err(|source| ClaimRemovalError::Io {
        operation: "checkpoint before claim",
        path: directory.diagnostic_entry(claim.as_os_str()),
        source,
    })?;
    let claim_result = checkpoint(ClaimRemovalPrimitive::ClaimRename)
        .and_then(|()| rename_no_replace(directory, original, claim.as_os_str()));
    match claim_result {
        Ok(()) => {
            checkpoint(ClaimRemovalPrimitive::AfterClaim).map_err(|source| {
                ClaimRemovalError::Io {
                    operation: "checkpoint after claim",
                    path: directory.diagnostic_entry(claim.as_os_str()),
                    source,
                }
            })?;
        }
        Err(source) => match classify_rename_error(&source) {
            RenameErrorClass::Occupied => {
                return Ok(unchanged(ClaimUnchangedReason::ClaimNameOccupied));
            }
            RenameErrorClass::Unsupported => {
                return Ok(unchanged(ClaimUnchangedReason::UnsupportedNoReplace {
                    primitive: no_replace_primitive(),
                }));
            }
            RenameErrorClass::SourceAbsent => return Ok(unknown_location()),
            RenameErrorClass::Ambiguous => {
                match reconcile_claim_rename(directory, original, prior, claim)? {
                    ClaimReconciliation::Succeeded => {}
                    ClaimReconciliation::Unchanged => {
                        return Ok(unchanged(
                            ClaimUnchangedReason::RenameNotAppliedAfterReconciliation,
                        ));
                    }
                    ClaimReconciliation::Unknown => return Ok(unknown_location()),
                }
            }
        },
    }

    checkpoint(ClaimRemovalPrimitive::BeforeInspection).map_err(|source| {
        ClaimRemovalError::PostClaimInspection {
            claim: claim.clone(),
            source: FlatDirectoryError::Io {
                operation: "checkpoint before claim inspection",
                path: directory.diagnostic_entry(claim.as_os_str()),
                source,
            },
        }
    })?;
    let claimed = match read_observed_file_unchecked(directory, claim.as_os_str()) {
        Ok(Some(observation)) => ClaimedObject::Observed(observation),
        Ok(None) => return Ok(unknown_location()),
        Err(FlatDirectoryError::NotRegular { .. } | FlatDirectoryError::IdentityChanged { .. }) => {
            match stat_entry(directory, claim.as_os_str()) {
                Ok(Some(entry)) => ClaimedObject::StatOnly(entry),
                Ok(None) => return Ok(unknown_location()),
                Err(source) => {
                    return Err(ClaimRemovalError::PostClaimInspection {
                        claim: claim.clone(),
                        source,
                    });
                }
            }
        }
        Err(source) => {
            return Err(ClaimRemovalError::PostClaimInspection {
                claim: claim.clone(),
                source,
            });
        }
    };
    if matches!(&claimed, ClaimedObject::Observed(observation) if same_observation(observation, prior))
    {
        return unlink_matching_claim(directory, claim);
    }
    restore_changed_claim(directory, original, &claimed, claim)
}

fn validate_original_name(original: &OsStr) -> Result<(), ClaimRemovalError> {
    let text = original
        .to_str()
        .ok_or_else(|| ClaimRemovalError::InvalidOriginalName {
            name: original.to_os_string(),
            reason: crate::name_admission::NameAdmissionReason::NotUtf8,
        })?;
    check_portable_component(text).map_err(|reason| ClaimRemovalError::InvalidOriginalName {
        name: original.to_os_string(),
        reason,
    })
}

fn preflight_error(source: FlatDirectoryError) -> ClaimRemovalError {
    ClaimRemovalError::Preflight { source }
}

enum ObservationState {
    Matches,
    Absent,
    Different,
}

enum ClaimedObject {
    Observed(FileObservation),
    StatOnly(FlatDirectoryEntry),
}

fn observation_state(
    directory: &FlatDirectory,
    name: &OsStr,
    expected: &FileObservation,
) -> Result<ObservationState, FlatDirectoryError> {
    match read_observed_file_unchecked(directory, name) {
        Ok(Some(observed)) if same_observation(&observed, expected) => {
            Ok(ObservationState::Matches)
        }
        Ok(Some(_)) => Ok(ObservationState::Different),
        Ok(None) => Ok(ObservationState::Absent),
        Err(FlatDirectoryError::NotRegular { .. } | FlatDirectoryError::IdentityChanged { .. }) => {
            Ok(ObservationState::Different)
        }
        Err(source) => Err(source),
    }
}

fn claimed_object_state(
    directory: &FlatDirectory,
    name: &OsStr,
    expected: &ClaimedObject,
) -> Result<ObservationState, FlatDirectoryError> {
    match expected {
        ClaimedObject::Observed(observation) => observation_state(directory, name, observation),
        ClaimedObject::StatOnly(expected) => match stat_entry(directory, name)? {
            Some(observed) if same_entry_metadata(&observed, expected) => {
                Ok(ObservationState::Matches)
            }
            Some(_) => Ok(ObservationState::Different),
            None => Ok(ObservationState::Absent),
        },
    }
}

enum ClaimReconciliation {
    Succeeded,
    Unchanged,
    Unknown,
}

fn reconcile_claim_rename(
    directory: &FlatDirectory,
    original: &OsStr,
    prior: &FileObservation,
    claim: &ClaimName,
) -> Result<ClaimReconciliation, ClaimRemovalError> {
    let original_state = observation_state(directory, original, prior).map_err(|source| {
        ClaimRemovalError::Reconciliation {
            original: original.to_os_string(),
            claim: claim.clone(),
            source,
        }
    })?;
    let claim_state = observation_state(directory, claim.as_os_str(), prior).map_err(|source| {
        ClaimRemovalError::Reconciliation {
            original: original.to_os_string(),
            claim: claim.clone(),
            source,
        }
    })?;
    match (original_state, claim_state) {
        (ObservationState::Matches, ObservationState::Matches) => {
            Err(ClaimRemovalError::ReconciliationInconclusive {
                original: original.to_os_string(),
                claim: claim.clone(),
            })
        }
        (ObservationState::Matches, _) => Ok(ClaimReconciliation::Unchanged),
        (_, ObservationState::Matches) => Ok(ClaimReconciliation::Succeeded),
        _ => Ok(ClaimReconciliation::Unknown),
    }
}

fn unlink_matching_claim(
    directory: &FlatDirectory,
    claim: &ClaimName,
) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
    checkpoint(ClaimRemovalPrimitive::BeforeUnlink).map_err(|source| {
        ClaimRemovalError::UnlinkFailure {
            claim: claim.clone(),
            source,
        }
    })?;
    checkpoint(ClaimRemovalPrimitive::Unlink)
        .and_then(|()| {
            unlinkat(directory, claim.as_os_str(), UnlinkatFlags::NoRemoveDir).map_err(errno_io)
        })
        .map_err(|source| ClaimRemovalError::UnlinkFailure {
            claim: claim.clone(),
            source,
        })?;
    match sync_directory(directory) {
        ClaimDurability::Synced => Ok(ClaimRemovalOutcome::Removed),
        ClaimDurability::Uncertain => Ok(ClaimRemovalOutcome::RemovedDurabilityUncertain),
        ClaimDurability::NotEstablished => {
            unreachable!("sync always establishes or loses durability")
        }
    }
}

fn restore_changed_claim(
    directory: &FlatDirectory,
    original: &OsStr,
    claimed: &ClaimedObject,
    claim: &ClaimName,
) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
    checkpoint(ClaimRemovalPrimitive::BeforeRestore).map_err(|source| {
        ClaimRemovalError::RestoreFailure {
            claim: claim.clone(),
            source,
        }
    })?;
    let restore_result = checkpoint(ClaimRemovalPrimitive::RestoreRename)
        .and_then(|()| rename_no_replace(directory, claim.as_os_str(), original));
    match restore_result {
        Ok(()) => Ok(identity_changed(
            IdentityChangeDisposition::Restored,
            sync_directory(directory),
        )),
        Err(source) => match classify_rename_error(&source) {
            RenameErrorClass::Occupied => Ok(identity_changed(
                IdentityChangeDisposition::RetainedClaim {
                    claim: claim.clone(),
                },
                sync_directory(directory),
            )),
            RenameErrorClass::SourceAbsent => Ok(unknown_location()),
            RenameErrorClass::Unsupported => Err(ClaimRemovalError::RestoreFailure {
                claim: claim.clone(),
                source,
            }),
            RenameErrorClass::Ambiguous => {
                reconcile_restore_rename(directory, original, claimed, claim)
            }
        },
    }
}

fn reconcile_restore_rename(
    directory: &FlatDirectory,
    original: &OsStr,
    claimed: &ClaimedObject,
    claim: &ClaimName,
) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
    let original_state = claimed_object_state(directory, original, claimed).map_err(|source| {
        ClaimRemovalError::Reconciliation {
            original: original.to_os_string(),
            claim: claim.clone(),
            source,
        }
    })?;
    let claim_state =
        claimed_object_state(directory, claim.as_os_str(), claimed).map_err(|source| {
            ClaimRemovalError::Reconciliation {
                original: original.to_os_string(),
                claim: claim.clone(),
                source,
            }
        })?;
    match (original_state, claim_state) {
        (ObservationState::Matches, ObservationState::Matches) => {
            Err(ClaimRemovalError::ReconciliationInconclusive {
                original: original.to_os_string(),
                claim: claim.clone(),
            })
        }
        (ObservationState::Matches, _) => Ok(identity_changed(
            IdentityChangeDisposition::Restored,
            sync_directory(directory),
        )),
        (_, ObservationState::Matches) => Ok(identity_changed(
            IdentityChangeDisposition::RetainedClaim {
                claim: claim.clone(),
            },
            sync_directory(directory),
        )),
        _ => Ok(unknown_location()),
    }
}

fn sync_directory(directory: &FlatDirectory) -> ClaimDurability {
    if checkpoint(ClaimRemovalPrimitive::DirectorySync).is_err() {
        return ClaimDurability::Uncertain;
    }
    match sync_dir_bound(directory) {
        Ok(()) => ClaimDurability::Synced,
        Err(_) => ClaimDurability::Uncertain,
    }
}

fn unchanged(reason: ClaimUnchangedReason) -> ClaimRemovalOutcome {
    ClaimRemovalOutcome::Unchanged { reason }
}

fn unknown_location() -> ClaimRemovalOutcome {
    identity_changed(
        IdentityChangeDisposition::UnknownLocation,
        ClaimDurability::NotEstablished,
    )
}

fn identity_changed(
    disposition: IdentityChangeDisposition,
    durability: ClaimDurability,
) -> ClaimRemovalOutcome {
    ClaimRemovalOutcome::IdentityChanged {
        disposition,
        durability,
    }
}

enum RenameErrorClass {
    Occupied,
    Unsupported,
    SourceAbsent,
    Ambiguous,
}

fn classify_rename_error(error: &io::Error) -> RenameErrorClass {
    let Some(raw) = error.raw_os_error() else {
        return RenameErrorClass::Ambiguous;
    };
    if raw == libc::EEXIST {
        RenameErrorClass::Occupied
    } else if raw == libc::ENOENT {
        RenameErrorClass::SourceAbsent
    } else if is_unsupported_no_replace_errno(raw) {
        RenameErrorClass::Unsupported
    } else {
        RenameErrorClass::Ambiguous
    }
}

fn is_unsupported_no_replace_errno(raw: i32) -> bool {
    // Both names are validated single components below the same retained
    // directory, so EINVAL here is attributable to the no-replace flag or its
    // unavailable implementation rather than an invalid pathname shape.
    raw == libc::ENOSYS || raw == libc::EOPNOTSUPP || raw == libc::ENOTSUP || raw == libc::EINVAL
}

fn no_replace_primitive() -> NoReplacePrimitive {
    #[cfg(target_os = "linux")]
    {
        NoReplacePrimitive::LinuxRenameAt2
    }
    #[cfg(target_os = "macos")]
    {
        NoReplacePrimitive::MacosRenameAtxNp
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        NoReplacePrimitive::UnsupportedUnix
    }
}

fn rename_no_replace(directory: &FlatDirectory, from: &OsStr, to: &OsStr) -> Result<(), io::Error> {
    let from = CString::new(from.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry name contains an interior NUL",
        )
    })?;
    let to = CString::new(to.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry name contains an interior NUL",
        )
    })?;
    let result = platform_rename_no_replace(directory.as_fd().as_raw_fd(), &from, &to)?;
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn platform_rename_no_replace(
    directory: i32,
    from: &CString,
    to: &CString,
) -> Result<i32, io::Error> {
    // `libc` exposes these Linux UAPI constants for both glibc and musl targets.
    unsafe {
        Ok(libc::syscall(
            libc::SYS_renameat2,
            directory,
            from.as_ptr(),
            directory,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        ) as i32)
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn platform_rename_no_replace(
    directory: i32,
    from: &CString,
    to: &CString,
) -> Result<i32, io::Error> {
    // libc 0.2.189 exposes the descriptor-relative Darwin variant directly.
    unsafe {
        Ok(libc::renameatx_np(
            directory,
            from.as_ptr(),
            directory,
            to.as_ptr(),
            libc::RENAME_EXCL,
        ))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_rename_no_replace(
    _directory: i32,
    _from: &CString,
    _to: &CString,
) -> Result<i32, io::Error> {
    Err(io::Error::from_raw_os_error(libc::ENOSYS))
}

fn errno_io(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint(primitive: ClaimRemovalPrimitive) -> Result<(), io::Error> {
    let (fault, barrier) = CLAIM_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (None, None);
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        let inject = state
            .fault
            .as_ref()
            .is_some_and(|fault| fault.primitive == primitive && fault.ordinal == ordinal);
        if inject {
            let fault = state.fault.take().expect("matching fault is present");
            state.fault_consumed = true;
            (Some(fault.error), None)
        } else {
            let barrier = state
                .barriers
                .iter()
                .position(|barrier| barrier.primitive == primitive && barrier.ordinal == ordinal);
            if let Some(index) = barrier {
                let barrier = state.barriers.remove(index);
                state.barriers_fired += 1;
                (None, Some(barrier.callback))
            } else {
                (None, None)
            }
        }
    });
    if let Some(error) = fault {
        return Err(errno_io(error));
    }
    if let Some(callback) = barrier {
        callback();
    }
    pause_at(primitive);
    Ok(())
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn checkpoint(_primitive: ClaimRemovalPrimitive) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
fn pause_at(primitive: ClaimRemovalPrimitive) {
    let step = match primitive {
        ClaimRemovalPrimitive::BeforeClaim => "claim-before-claim",
        ClaimRemovalPrimitive::ClaimRename => "claim-rename",
        ClaimRemovalPrimitive::AfterClaim => "claim-after-claim",
        ClaimRemovalPrimitive::BeforeInspection => "claim-before-inspection",
        ClaimRemovalPrimitive::BeforeUnlink => "claim-before-unlink",
        ClaimRemovalPrimitive::Unlink => "claim-unlink",
        ClaimRemovalPrimitive::BeforeRestore => "claim-before-restore",
        ClaimRemovalPrimitive::RestoreRename => "claim-restore-rename",
        ClaimRemovalPrimitive::DirectorySync => "claim-directory-sync",
    };
    if std::env::var("JOURNAL_IO_TEST_PAUSE_AT").ok().as_deref() == Some(step) {
        if let Ok(marker) = std::env::var("JOURNAL_IO_TEST_MARKER") {
            let _ = std::fs::write(marker, step);
        }
        loop {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::FileTypeExt;
    use std::path::Path;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    use super::*;
    use crate::journal_root::JournalRoot;
    use crate::test_support::TempDir;

    fn setup() -> (TempDir, JournalRoot, FlatDirectory, ClaimName) {
        let temporary = TempDir::new();
        fs::create_dir(temporary.path().join("flat")).unwrap();
        let root = JournalRoot::open(temporary.path()).unwrap();
        let directory = FlatDirectory::open(&root, Path::new("flat")).unwrap();
        let claim = ClaimName::parse("!solstone-claim-00000001-0000000000000001").unwrap();
        (temporary, root, directory, claim)
    }

    #[test]
    fn removes_the_exact_observed_entry() {
        let (temporary, _root, directory, claim) = setup();
        fs::write(temporary.path().join("flat/entry"), b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        assert_eq!(
            claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim).unwrap(),
            ClaimRemovalOutcome::Removed
        );
        assert!(!temporary.path().join("flat/entry").exists());
        assert!(!temporary.path().join(claim.as_str()).exists());
    }

    #[test]
    fn source_disappearance_is_unknown_not_absent() {
        let (temporary, _root, directory, claim) = setup();
        fs::write(temporary.path().join("flat/entry"), b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        fs::remove_file(temporary.path().join("flat/entry")).unwrap();
        assert_eq!(
            claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim).unwrap(),
            unknown_location()
        );
    }

    #[test]
    fn occupied_claim_is_unchanged() {
        let (temporary, _root, directory, claim) = setup();
        fs::write(temporary.path().join("flat/entry"), b"observed").unwrap();
        fs::write(
            temporary.path().join("flat").join(claim.as_str()),
            b"occupied",
        )
        .unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        assert_eq!(
            claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim).unwrap(),
            unchanged(ClaimUnchangedReason::ClaimNameOccupied)
        );
        assert!(temporary.path().join("flat/entry").exists());
    }

    #[test]
    fn aliased_claim_is_a_caller_contract_error() {
        let (temporary, _root, directory, claim) = setup();
        let entry = temporary.path().join("flat/entry");
        fs::write(&entry, b"observed").unwrap();
        fs::hard_link(&entry, temporary.path().join("flat").join(claim.as_str())).unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        assert!(matches!(
            claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
            Err(ClaimRemovalError::AliasedClaimName { .. })
        ));
    }

    #[test]
    fn injected_ambiguous_claim_error_is_reconciled_without_assuming_success() {
        let (temporary, _root, directory, claim) = setup();
        fs::write(temporary.path().join("flat/entry"), b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        let (result, consumed) =
            run_with_claim_removal_fault(ClaimRemovalPrimitive::ClaimRename, 1, libc::EIO, || {
                claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim)
            });
        assert!(consumed);
        assert_eq!(
            result.unwrap(),
            unchanged(ClaimUnchangedReason::RenameNotAppliedAfterReconciliation)
        );
        assert!(temporary.path().join("flat/entry").exists());
    }

    #[test]
    fn injected_ambiguous_claim_error_reconciles_a_rename_that_already_succeeded() {
        let (temporary, _root, directory, claim) = setup();
        let original = temporary.path().join("flat/entry");
        let claim_path = temporary.path().join("flat").join(claim.as_str());
        fs::write(&original, b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        let (result, fault_consumed, barrier_fired) = run_with_claim_removal_fault_and_barrier(
            ClaimRemovalPrimitive::ClaimRename,
            1,
            libc::EIO,
            ClaimRemovalPrimitive::BeforeClaim,
            1,
            move || fs::rename(original, claim_path).unwrap(),
            || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
        );
        assert!(fault_consumed);
        assert!(barrier_fired);
        assert_eq!(result.unwrap(), ClaimRemovalOutcome::Removed);
    }

    #[test]
    fn removed_but_unsynced_is_never_reported_as_removed() {
        let (temporary, _root, directory, claim) = setup();
        fs::write(temporary.path().join("flat/entry"), b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        let (result, consumed) = run_with_claim_removal_fault(
            ClaimRemovalPrimitive::DirectorySync,
            1,
            libc::EIO,
            || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
        );
        assert!(consumed);
        assert_eq!(
            result.unwrap(),
            ClaimRemovalOutcome::RemovedDurabilityUncertain
        );
        assert!(!temporary.path().join("flat/entry").exists());
        assert!(!temporary.path().join("flat").join(claim.as_str()).exists());
    }

    #[test]
    fn claimed_name_disappearance_is_unknown_not_an_inspection_error() {
        let (temporary, _root, directory, claim) = setup();
        fs::write(temporary.path().join("flat/entry"), b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        let claim_path = temporary.path().join("flat").join(claim.as_str());
        let (result, fired) = run_with_claim_removal_barrier(
            ClaimRemovalPrimitive::AfterClaim,
            1,
            move || fs::remove_file(claim_path).unwrap(),
            || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
        );
        assert!(fired);
        assert_eq!(result.unwrap(), unknown_location());
    }

    #[test]
    fn restore_source_disappearance_is_unknown_not_an_error() {
        let (temporary, _root, directory, claim) = setup();
        fs::write(temporary.path().join("flat/entry"), b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        let claim_path = temporary.path().join("flat").join(claim.as_str());
        let changed_claim = claim_path.clone();
        let (result, fired) = run_with_two_claim_removal_barriers(
            ClaimRemovalPrimitive::BeforeInspection,
            1,
            move || fs::write(changed_claim, b"changed").unwrap(),
            ClaimRemovalPrimitive::BeforeRestore,
            1,
            move || fs::remove_file(claim_path).unwrap(),
            || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
        );
        assert_eq!(fired, 2);
        assert_eq!(result.unwrap(), unknown_location());
    }

    #[test]
    fn non_regular_claimed_entries_are_restored_without_unlinking() {
        let (temporary, _root, directory, claim) = setup();
        let original = temporary.path().join("flat/entry");
        let claim_path = temporary.path().join("flat").join(claim.as_str());
        fs::write(&original, b"observed").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        let fifo_path = claim_path.clone();
        let (result, fired) = run_with_claim_removal_barrier(
            ClaimRemovalPrimitive::BeforeInspection,
            1,
            move || {
                fs::remove_file(&fifo_path).unwrap();
                mkfifo(&fifo_path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
            },
            || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
        );
        assert!(fired);
        assert_eq!(
            result.unwrap(),
            identity_changed(IdentityChangeDisposition::Restored, ClaimDurability::Synced)
        );
        assert!(
            fs::symlink_metadata(&original)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        assert!(!claim_path.exists());

        fs::remove_file(&original).unwrap();
        fs::write(&original, b"observed-again").unwrap();
        let prior = crate::flat_directory::read_observed_file(&directory, OsStr::new("entry"))
            .unwrap()
            .unwrap();
        let directory_path = claim_path.clone();
        let (result, fired) = run_with_claim_removal_barrier(
            ClaimRemovalPrimitive::BeforeInspection,
            1,
            move || {
                fs::remove_file(&directory_path).unwrap();
                fs::create_dir(&directory_path).unwrap();
            },
            || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
        );
        assert!(fired);
        assert_eq!(
            result.unwrap(),
            identity_changed(IdentityChangeDisposition::Restored, ClaimDurability::Synced)
        );
        assert!(
            fs::symlink_metadata(&original)
                .unwrap()
                .file_type()
                .is_dir()
        );
        assert!(!claim_path.exists());
    }
}
