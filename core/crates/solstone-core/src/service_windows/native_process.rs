// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only retained supervisor observation; never termination authority.

use solstone_core_system::process::ProcessInstance;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

pub(super) struct RetainedProcess {
    handle: OwnedHandle,
}

impl RetainedProcess {
    pub(super) fn open(expected: ProcessInstance) -> io::Result<Self> {
        let birth = expected
            .birth
            .windows_filetime()
            .ok_or_else(|| io::Error::other("native process birth is missing"))?;
        if expected.pid == 0 {
            return Err(io::Error::other("native process PID is invalid"));
        }
        // SAFETY: no inherited handle or process mutation rights are requested.
        #[allow(unsafe_code)]
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                expected.pid,
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OpenProcess returned one newly owned handle.
        #[allow(unsafe_code)]
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut creation: FILETIME = Default::default();
        let mut exit: FILETIME = Default::default();
        let mut kernel: FILETIME = Default::default();
        let mut user: FILETIME = Default::default();
        // SAFETY: handle is retained and all output pointers refer to initialized storage.
        #[allow(unsafe_code)]
        if unsafe {
            GetProcessTimes(
                handle.as_raw_handle(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let actual = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        if actual != birth {
            return Err(io::Error::other("native process birth changed"));
        }
        Ok(Self { handle })
    }

    pub(super) fn exit_code(&self) -> io::Result<Option<u32>> {
        // SAFETY: retained read-only process handle; zero timeout cannot block.
        #[allow(unsafe_code)]
        let wait = unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) };
        match wait {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: process is signaled and output points to initialized storage.
                #[allow(unsafe_code)]
                if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(code))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
}
