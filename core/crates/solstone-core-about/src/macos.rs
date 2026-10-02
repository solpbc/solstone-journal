// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(unsafe_code)]

use super::About;
use std::ffi::CStr;

fn sysctl(name: &CStr, bytes: &mut [u8]) -> bool {
    let mut length = bytes.len();
    // Both pointers are valid for the stated capacities; there is no write value.
    unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        ) == 0
    }
}

pub(super) fn host_about(version: &str) -> About {
    let mut product = [0u8; 128];
    let os_version = if sysctl(c"kern.osproductversion", &mut product) {
        String::from_utf8_lossy(&product)
            .trim_end_matches('\0')
            .to_owned()
    } else {
        String::new()
    };
    let mut translated_bytes = [0u8; 4];
    let translated = if sysctl(c"sysctl.proc_translated", &mut translated_bytes) {
        Some(i32::from_ne_bytes(translated_bytes) == 1)
    } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
        // Intel kernels do not expose the Rosetta flag.
        Some(false)
    } else {
        None
    };
    let mut machine_bytes = [0u8; 128];
    let machine = sysctl(c"hw.machine", &mut machine_bytes)
        .then(|| CStr::from_bytes_until_nul(&machine_bytes).ok())
        .flatten()
        .and_then(|value| value.to_str().ok());
    let arch = super::native_macos_arch(translated, machine);
    let build = std::env::current_exe().ok().and_then(|exe| {
        exe.ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "Contents"))
            .and_then(|contents| plist::Value::from_file(contents.join("Info.plist")).ok())
            .and_then(|value| {
                let dictionary = value.as_dictionary()?;
                if dictionary.get("CFBundleIdentifier")?.as_string()? != "app.solstone.journal" {
                    return None;
                }
                dictionary
                    .get("CFBundleVersion")?
                    .as_string()
                    .map(str::to_owned)
            })
            .filter(|value| {
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || byte == b'.')
            })
    });
    About::from_facts(version, build, "macos".into(), os_version, arch.into())
}
