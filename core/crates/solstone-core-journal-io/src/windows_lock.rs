// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows whole-file advisory lock guard.

use std::fmt;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, RawHandle};

use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
use windows_sys::Win32::Storage::FileSystem::{
    LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
};
use windows_sys::Win32::System::IO::OVERLAPPED;

const WHOLE_FILE_LOW: u32 = u32::MAX;
const WHOLE_FILE_HIGH: u32 = u32::MAX;

/// A retained handle that owns one whole-file byte-range advisory lock.
pub(crate) struct WindowsLockGuard {
    file: File,
    locked_handle: Option<RawHandle>,
    overlapped: OVERLAPPED,
}

// SAFETY: LockFileEx is always called with LOCKFILE_FAIL_IMMEDIATELY, so this guard
// never has an asynchronous completion or event registration in flight. The guard owns
// its File and zeroed OVERLAPPED exclusively, and File itself is Send.
#[allow(unsafe_code)]
unsafe impl Send for WindowsLockGuard {}

impl fmt::Debug for WindowsLockGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsLockGuard")
            .field("file", &self.file)
            .field("locked_handle", &self.locked_handle)
            .finish_non_exhaustive()
    }
}

impl WindowsLockGuard {
    pub(crate) fn file(&self) -> &File {
        &self.file
    }
}

impl Drop for WindowsLockGuard {
    fn drop(&mut self) {
        let Some(locked_handle) = self.locked_handle else {
            return;
        };
        // SAFETY: this guard owns the file handle and retains the exact whole-file range
        // supplied to LockFileEx. Test-only replacement handles remain live for the guard's
        // lifetime. Drop cannot report a failure; closing the handle remains the kernel fallback.
        #[allow(unsafe_code)]
        let result = unsafe {
            UnlockFileEx(
                locked_handle,
                0,
                WHOLE_FILE_LOW,
                WHOLE_FILE_HIGH,
                &mut self.overlapped,
            )
        };
        #[cfg(any(test, feature = "test-hooks"))]
        let error = if result == 0 {
            io::Error::last_os_error().raw_os_error()
        } else {
            None
        };
        #[cfg(not(any(test, feature = "test-hooks")))]
        let error = None;
        record_unlock_file_ex_observation(locked_handle, result != 0, error);
    }
}

pub(crate) fn try_lock_exclusive(file: File) -> Result<WindowsLockGuard, (File, io::Error)> {
    try_lock(file, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)
}

/// Exclusive whole-file lock whose drop only closes the handle.
pub(crate) struct WindowsLockHeld {
    #[allow(dead_code)]
    file: File,
}

impl fmt::Debug for WindowsLockHeld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsLockHeld")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

pub(crate) fn try_lock_exclusive_held(file: File) -> Result<WindowsLockHeld, (File, io::Error)> {
    let mut overlapped = OVERLAPPED::default();
    let handle = lock_handle(file.as_raw_handle());
    let genuine = handle.is_some();
    let result = match handle {
        Some(handle) => {
            record_lock_handle(handle);
            // SAFETY: `handle` is either the owned file handle or a test-only borrowed
            // replacement. The zeroed OVERLAPPED describes the whole-file range for this
            // synchronous `LOCKFILE_FAIL_IMMEDIATELY` call.
            #[allow(unsafe_code)]
            unsafe {
                LockFileEx(
                    handle,
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    WHOLE_FILE_LOW,
                    WHOLE_FILE_HIGH,
                    &mut overlapped,
                )
            }
        }
        None => 1,
    };
    if result != 0 {
        record_last_lock_file_ex_genuine(genuine);
        Ok(WindowsLockHeld { file })
    } else {
        Err((file, io::Error::last_os_error()))
    }
}

pub(crate) fn try_lock_shared(file: File) -> Result<WindowsLockGuard, (File, io::Error)> {
    try_lock(file, LOCKFILE_FAIL_IMMEDIATELY)
}

pub(crate) fn is_contention(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32)
}

fn try_lock(file: File, flags: u32) -> Result<WindowsLockGuard, (File, io::Error)> {
    let mut overlapped = OVERLAPPED::default();
    let handle = lock_handle(file.as_raw_handle());
    let genuine = handle.is_some();
    let result = match handle {
        Some(handle) => {
            record_lock_handle(handle);
            // SAFETY: `handle` is either the owned file handle or a test-only borrowed
            // replacement. The zeroed OVERLAPPED describes the retained whole-file range.
            #[allow(unsafe_code)]
            unsafe {
                LockFileEx(
                    handle,
                    flags,
                    0,
                    WHOLE_FILE_LOW,
                    WHOLE_FILE_HIGH,
                    &mut overlapped,
                )
            }
        }
        None => 1,
    };
    if result != 0 {
        record_last_lock_file_ex_genuine(genuine);
        Ok(WindowsLockGuard {
            file,
            locked_handle: handle,
            overlapped,
        })
    } else {
        Err((file, io::Error::last_os_error()))
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn with_lock_file_ex_trace<T>(operation: impl FnOnce() -> T) -> (T, Vec<RawHandle>) {
    LOCK_FILE_EX_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "LockFileEx trace is already active"
        );
        *trace.borrow_mut() = Some(Vec::new());
    });
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            LOCK_FILE_EX_TRACE.with(|trace| {
                trace.borrow_mut().take();
            });
        }
    }
    let restore = Restore;
    let result = operation();
    let trace = LOCK_FILE_EX_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("LockFileEx trace remains active")
    });
    drop(restore);
    (result, trace)
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy)]
pub enum WindowsLockFileExSubstitution {
    Skip,
    ReplaceHandle(RawHandle),
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsUnlockFileExObservation {
    pub handle: RawHandle,
    pub length_low: u32,
    pub length_high: u32,
    pub succeeded: bool,
    pub error: Option<i32>,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy)]
struct LockFileExSubstitutionState {
    ordinal: usize,
    seen: usize,
    consumed: bool,
    substitution: WindowsLockFileExSubstitution,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static LOCK_FILE_EX_SUBSTITUTION: std::cell::RefCell<Option<LockFileExSubstitutionState>> = const {
        std::cell::RefCell::new(None)
    };
    static FORCE_POST_LOCK_IDENTITY_MISMATCH: std::cell::RefCell<Option<(usize, usize, bool)>> = const {
        std::cell::RefCell::new(None)
    };
    static LOCK_FILE_EX_TRACE: std::cell::RefCell<Option<Vec<RawHandle>>> = const {
        std::cell::RefCell::new(None)
    };
    static UNLOCK_FILE_EX_OBSERVATIONS: std::cell::RefCell<Option<Vec<WindowsUnlockFileExObservation>>> = const {
        std::cell::RefCell::new(None)
    };
    static LAST_LOCK_FILE_EX_WAS_GENUINE: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
fn lock_handle(handle: RawHandle) -> Option<RawHandle> {
    LOCK_FILE_EX_SUBSTITUTION.with(|substitution| {
        let mut substitution = substitution.borrow_mut();
        let Some(state) = substitution.as_mut() else {
            return Some(handle);
        };
        state.seen += 1;
        if state.seen != state.ordinal {
            return Some(handle);
        }
        state.consumed = true;
        match state.substitution {
            WindowsLockFileExSubstitution::Skip => None,
            WindowsLockFileExSubstitution::ReplaceHandle(handle) => Some(handle),
        }
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn lock_handle(handle: RawHandle) -> Option<RawHandle> {
    Some(handle)
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_lock_handle(handle: RawHandle) {
    LOCK_FILE_EX_TRACE.with(|trace| {
        if let Some(trace) = trace.borrow_mut().as_mut() {
            trace.push(handle);
        }
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_unlock_file_ex_observation(handle: RawHandle, succeeded: bool, error: Option<i32>) {
    UNLOCK_FILE_EX_OBSERVATIONS.with(|observations| {
        if let Some(observations) = observations.borrow_mut().as_mut() {
            observations.push(WindowsUnlockFileExObservation {
                handle,
                length_low: WHOLE_FILE_LOW,
                length_high: WHOLE_FILE_HIGH,
                succeeded,
                error,
            });
        }
    });
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn record_lock_handle(_handle: RawHandle) {}

#[cfg(not(any(test, feature = "test-hooks")))]
fn record_unlock_file_ex_observation(_handle: RawHandle, _succeeded: bool, _error: Option<i32>) {}

#[cfg(any(test, feature = "test-hooks"))]
fn record_last_lock_file_ex_genuine(genuine: bool) {
    LAST_LOCK_FILE_EX_WAS_GENUINE.with(|last| last.set(genuine));
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn record_last_lock_file_ex_genuine(_genuine: bool) {}

#[cfg(any(test, feature = "test-hooks"))]
fn last_lock_file_ex_was_genuine() -> bool {
    LAST_LOCK_FILE_EX_WAS_GENUINE.with(std::cell::Cell::get)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn with_lock_file_ex_substitution<T>(
    ordinal: usize,
    substitution: WindowsLockFileExSubstitution,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    LOCK_FILE_EX_SUBSTITUTION.with(|state| {
        assert!(
            state.borrow().is_none(),
            "LockFileEx substitution is already active"
        );
        *state.borrow_mut() = Some(LockFileExSubstitutionState {
            ordinal,
            seen: 0,
            consumed: false,
            substitution,
        });
    });
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            LOCK_FILE_EX_SUBSTITUTION.with(|state| {
                state.borrow_mut().take();
            });
        }
    }
    let restore = Restore;
    let result = operation();
    let state = LOCK_FILE_EX_SUBSTITUTION.with(|state| {
        state
            .borrow_mut()
            .take()
            .expect("LockFileEx substitution remains active")
    });
    drop(restore);
    (result, state.consumed)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn with_unlock_file_ex_observation<T>(
    operation: impl FnOnce() -> T,
) -> (T, Vec<WindowsUnlockFileExObservation>) {
    UNLOCK_FILE_EX_OBSERVATIONS.with(|observations| {
        assert!(
            observations.borrow().is_none(),
            "UnlockFileEx observation is already active"
        );
        *observations.borrow_mut() = Some(Vec::new());
    });
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            UNLOCK_FILE_EX_OBSERVATIONS.with(|observations| {
                observations.borrow_mut().take();
            });
        }
    }
    let restore = Restore;
    let result = operation();
    let observations = UNLOCK_FILE_EX_OBSERVATIONS.with(|observations| {
        observations
            .borrow_mut()
            .take()
            .expect("UnlockFileEx observation remains active")
    });
    drop(restore);
    (result, observations)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn with_forced_post_lock_identity_mismatch<T>(
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    FORCE_POST_LOCK_IDENTITY_MISMATCH.with(|state| {
        assert!(
            state.borrow().is_none(),
            "post-lock identity substitution is already active"
        );
        *state.borrow_mut() = Some((ordinal, 0, false));
    });
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            FORCE_POST_LOCK_IDENTITY_MISMATCH.with(|state| {
                state.borrow_mut().take();
            });
        }
    }
    let restore = Restore;
    let result = operation();
    let state = FORCE_POST_LOCK_IDENTITY_MISMATCH.with(|state| {
        state
            .borrow_mut()
            .take()
            .expect("post-lock identity substitution remains active")
    });
    drop(restore);
    (result, state.2)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn force_post_lock_identity_mismatch() -> bool {
    FORCE_POST_LOCK_IDENTITY_MISMATCH.with(|state| {
        let mut state = state.borrow_mut();
        let Some((ordinal, seen, consumed)) = state.as_mut() else {
            return false;
        };
        *seen += 1;
        if *seen == *ordinal && last_lock_file_ex_was_genuine() {
            *consumed = true;
            true
        } else {
            false
        }
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
pub(crate) fn force_post_lock_identity_mismatch() -> bool {
    false
}

#[cfg(feature = "test-hooks")]
pub fn run_with_windows_lock_file_ex_trace<T>(
    operation: impl FnOnce() -> T,
) -> (T, Vec<RawHandle>) {
    with_lock_file_ex_trace(operation)
}

#[cfg(feature = "test-hooks")]
pub fn run_with_windows_lock_file_ex_substitution<T>(
    ordinal: usize,
    substitution: WindowsLockFileExSubstitution,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    with_lock_file_ex_substitution(ordinal, substitution, operation)
}

#[cfg(feature = "test-hooks")]
pub fn run_with_windows_unlock_file_ex_observation<T>(
    operation: impl FnOnce() -> T,
) -> (T, Vec<WindowsUnlockFileExObservation>) {
    with_unlock_file_ex_observation(operation)
}

#[cfg(feature = "test-hooks")]
pub fn run_with_forced_post_lock_identity_mismatch<T>(
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    with_forced_post_lock_identity_mismatch(ordinal, operation)
}
