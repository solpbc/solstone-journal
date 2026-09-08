// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Advisory duplicate suppression for the heartbeat command's numeric PID file.
//! This cannot establish heartbeat identity or authorize process control.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

pub(crate) fn recorded_pid_may_be_running(pid: u32) -> io::Result<bool> {
    if pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid heartbeat PID",
        ));
    }
    // SAFETY: requests only observation rights; the returned handle is not inheritable.
    #[allow(unsafe_code)]
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        let error = io::Error::last_os_error();
        return match error.raw_os_error().map(|code| code as u32) {
            // As with Unix EPERM, suppress duplicates without claiming verified identity.
            Some(ERROR_ACCESS_DENIED) => Ok(true),
            Some(ERROR_INVALID_PARAMETER) => Ok(false),
            _ => Err(error),
        };
    }
    // SAFETY: OpenProcess transferred this newly owned, non-null handle.
    #[allow(unsafe_code)]
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the owned process handle remains open throughout this zero-timeout wait.
    #[allow(unsafe_code)]
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_TIMEOUT => Ok(true),
        WAIT_OBJECT_0 => Ok(false),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        value => Err(io::Error::other(format!(
            "unexpected process wait result {value}"
        ))),
    }
}
