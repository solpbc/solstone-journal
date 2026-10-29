// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A Mac that wakes itself while asleep (maintenance, Power Nap) runs with the
//! CPU on, graphics off and its fans held quiet. Model work waits for a full
//! wake instead of heating the machine inside those wakes.

const CAPABILITY_CPU: u64 = 0x01;
const CAPABILITY_GRAPHICS: u64 = 0x02;

fn is_dark_wake(capabilities: u64) -> bool {
    capabilities & CAPABILITY_CPU != 0 && capabilities & CAPABILITY_GRAPHICS == 0
}

/// Unknown is never a dark wake, so a failed read keeps today's behaviour.
pub(super) fn in_dark_wake() -> bool {
    system_capabilities().is_some_and(is_dark_wake)
}

#[cfg(target_os = "macos")]
fn system_capabilities() -> Option<u64> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static CACHE: Mutex<Option<(Instant, Option<u64>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().expect("wake capability cache");
    if let Some((read_at, value)) = *cache
        && read_at.elapsed() < Duration::from_secs(1)
    {
        return value;
    }
    let value = read_root_domain_capabilities();
    *cache = Some((Instant::now(), value));
    value
}

#[cfg(not(target_os = "macos"))]
fn system_capabilities() -> Option<u64> {
    None
}

/// The kernel publishes the current wake's capabilities on IOPMrootDomain as
/// "System Capabilities" when each wake or capability change completes.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn read_root_domain_capabilities() -> Option<u64> {
    use std::ffi::{c_char, c_void};

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            value: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFGetTypeID(value: *const c_void) -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFNumberGetValue(number: *const c_void, kind: isize, value: *mut c_void) -> u8;
        fn CFRelease(value: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;

    // SAFETY: each Create/Get result is checked for null and released exactly
    // once; IOServiceGetMatchingService consumes the matching dictionary; the
    // number is written into a local of the requested 64-bit width.
    unsafe {
        let matching = IOServiceMatching(c"IOPMrootDomain".as_ptr());
        if matching.is_null() {
            return None;
        }
        let root = IOServiceGetMatchingService(0, matching);
        if root == 0 {
            return None;
        }
        let key =
            CFStringCreateWithCString(std::ptr::null(), c"System Capabilities".as_ptr(), UTF8);
        if key.is_null() {
            IOObjectRelease(root);
            return None;
        }
        let property = IORegistryEntryCreateCFProperty(root, key, std::ptr::null(), 0);
        CFRelease(key);
        IOObjectRelease(root);
        if property.is_null() {
            return None;
        }
        let mut value: i64 = 0;
        let read = CFGetTypeID(property) == CFNumberGetTypeID()
            && CFNumberGetValue(property, SINT64, (&raw mut value).cast()) != 0;
        CFRelease(property);
        read.then_some(value as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cpu_without_graphics_is_a_dark_wake() {
        assert!(is_dark_wake(CAPABILITY_CPU));
        assert!(is_dark_wake(CAPABILITY_CPU | 0x08));
        assert!(!is_dark_wake(0x0f));
        assert!(!is_dark_wake(CAPABILITY_CPU | CAPABILITY_GRAPHICS));
        assert!(!is_dark_wake(0));
    }
}
