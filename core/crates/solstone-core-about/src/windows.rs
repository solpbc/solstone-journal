// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(unsafe_code)]

use super::About;
use windows_sys::Win32::System::{
    Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
    Threading::{GetCurrentProcess, IsWow64Process2},
};

pub(super) fn host_about(version: &str) -> About {
    let key: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "CurrentBuildNumber\0".encode_utf16().collect();
    let mut buffer = [0u16; 32];
    let mut length = std::mem::size_of_val(&buffer) as u32;
    // NUL-terminated fixed key/value, valid writable buffer; RegGetValue validates its size.
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut length,
        )
    };
    let os_version = if result == 0 {
        String::from_utf16_lossy(&buffer)
            .trim_end_matches('\0')
            .parse()
            .ok()
            .map(super::windows_version)
            .unwrap_or_default()
    } else {
        String::new()
    };
    let mut process_machine = 0;
    let mut native_machine = 0;
    // Current-process pseudo-handle needs no close. Both output pointers are valid.
    let native = unsafe {
        IsWow64Process2(
            GetCurrentProcess(),
            &mut process_machine,
            &mut native_machine,
        )
    };
    let arch = if native != 0 {
        super::native_windows_arch(process_machine, native_machine)
    } else {
        ""
    };
    About::from_facts(version, None, "windows".into(), os_version, arch.into())
}
