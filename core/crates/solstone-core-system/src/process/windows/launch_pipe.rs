// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native access and peer identity for the one-transaction launch pipe.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Arc;
use std::time::Instant;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::Pipes::*;
use windows_sys::Win32::System::Threading::*;

use super::identity::{SystemWindowsProcessApi, WindowsProcessProbe, sample_windows_process_with};
use crate::process::ProcessInstance;

pub(super) const CLIENT_ACCESS: u32 =
    FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
pub(super) const PIPE_PREFIX: &str = r"\\.\pipe\solstone-launch-";

fn wide(value: &str) -> io::Result<Vec<u16>> {
    if value.contains('\0') {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}

struct LocalAllocation(*mut std::ffi::c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: both conversion APIs return LocalAlloc-owned buffers.
        #[allow(unsafe_code)]
        unsafe {
            LocalFree(self.0)
        };
    }
}

fn owner_sid() -> io::Result<String> {
    let mut token = std::ptr::null_mut();
    // SAFETY: valid process pseudo-handle and writable output.
    #[allow(unsafe_code)]
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful OpenProcessToken transferred unique ownership.
    #[allow(unsafe_code)]
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut required = 0;
    // SAFETY: documented token-information sizing call.
    #[allow(unsafe_code)]
    let first = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut required,
        )
    };
    if first != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || required < std::mem::size_of::<TOKEN_USER>() as u32
    {
        return Err(io::Error::other("cannot size launch owner token"));
    }
    // Pointer-aligned storage for TOKEN_USER and its SID.
    let mut buffer = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: buffer is aligned and has at least the requested capacity.
    #[allow(unsafe_code)]
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful query initialized TOKEN_USER and its SID in this live buffer.
    #[allow(unsafe_code)]
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = std::ptr::null_mut();
    // SAFETY: SID remains valid through conversion and text is writable.
    #[allow(unsafe_code)]
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let allocation = LocalAllocation(text.cast());
    // A Windows textual SID is bounded by its 15 subauthorities; refuse missing
    // termination instead of scanning unbounded memory.
    // SAFETY: converter returns a NUL-terminated UTF-16 string. Stop at its NUL.
    #[allow(unsafe_code)]
    let result = unsafe {
        let length = (0..184)
            .find(|index| *text.add(*index) == 0)
            .ok_or_else(|| io::Error::other("launch owner SID is not terminated"))?;
        String::from_utf16(std::slice::from_raw_parts(text, length))
            .map_err(|_| io::Error::other("launch owner SID is not UTF-16"))
    };
    drop(allocation);
    result
}

pub(super) fn create(name: &str) -> io::Result<Arc<OwnedHandle>> {
    let name = wide(name)?;
    let sddl = wide(&format!("D:P(A;;0x{CLIENT_ACCESS:08x};;;{})", owner_sid()?))?;
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: bounded NUL-terminated SDDL, writable LocalAlloc output pointer.
    #[allow(unsafe_code)]
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = LocalAllocation(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: descriptor lives through creation; output is uniquely owned.
    // One local-only first instance, byte framing, no inheritable authority.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            32768,
            32768,
            0,
            &attributes,
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful CreateNamedPipeW returned a unique handle.
    #[allow(unsafe_code)]
    Ok(Arc::new(unsafe { OwnedHandle::from_raw_handle(raw) }))
}

pub(super) fn open(name: &str, deadline: Instant) -> io::Result<Arc<OwnedHandle>> {
    let name = wide(name)?;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        // SAFETY: bounded terminated name; the client cannot impersonate with
        // broader rights or create another pipe instance through this handle.
        #[allow(unsafe_code)]
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                CLIENT_ACCESS,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if raw != INVALID_HANDLE_VALUE {
            // SAFETY: successful CreateFileW returned a unique handle.
            #[allow(unsafe_code)]
            return Ok(Arc::new(unsafe { OwnedHandle::from_raw_handle(raw) }));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) {
            return Err(error);
        }
        let milliseconds = remaining.as_millis().clamp(1, u128::from(u32::MAX - 1)) as u32;
        // SAFETY: stable name and bounded wait; the original deadline is never reset.
        #[allow(unsafe_code)]
        if unsafe { WaitNamedPipeW(name.as_ptr(), milliseconds) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
}

pub(super) fn peer(pipe: &OwnedHandle, server: bool) -> io::Result<ProcessInstance> {
    let mut pid = 0;
    // SAFETY: connected pipe remains borrowed; PID output is writable.
    #[allow(unsafe_code)]
    let result = unsafe {
        if server {
            GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid)
        } else {
            GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid)
        }
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    match sample_windows_process_with(&SystemWindowsProcessApi, pid) {
        WindowsProcessProbe::Live(instance) => Ok(instance),
        _ => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "launch pipe peer is not verifiably live",
        )),
    }
}

pub(super) fn disconnect(pipe: &OwnedHandle) -> io::Result<()> {
    // SAFETY: caller has settled all I/O for this connection before reuse.
    #[allow(unsafe_code)]
    if unsafe { DisconnectNamedPipe(pipe.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn require_direct_parent(parent: ProcessInstance) -> io::Result<()> {
    // SAFETY: process snapshot uses no supplied memory and returns owned handle.
    #[allow(unsafe_code)]
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: snapshot creation returned unique ownership.
    #[allow(unsafe_code)]
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = PROCESSENTRY32W::default();
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: snapshot is live and entry has documented size and writable storage.
    #[allow(unsafe_code)]
    let mut next = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) };
    while next != 0 {
        if entry.th32ProcessID == std::process::id() {
            return if entry.th32ParentProcessID == parent.pid
                && matches!(sample_windows_process_with(&SystemWindowsProcessApi, parent.pid), WindowsProcessProbe::Live(actual) if actual == parent)
            {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "launch server is not the exact direct parent",
                ))
            };
        }
        // SAFETY: same live snapshot and writable entry for iteration.
        #[allow(unsafe_code)]
        {
            next = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) };
        }
    }
    Err(io::Error::other(
        "current process is absent from the parent snapshot",
    ))
}
