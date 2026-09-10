// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Injected memory-admission decisions for local GPU work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Read currently available physical memory for local-work admission.
///
/// An unavailable or inconsistent reading remains unknown to the caller's
/// policy. macOS counts free and inactive pages because both are immediately
/// available to a new allocation without swapping.
#[cfg(windows)]
pub fn available_physical_bytes() -> Option<u64> {
    windows_available_physical_bytes()
}

#[cfg(target_os = "macos")]
pub fn available_physical_bytes() -> Option<u64> {
    macos_available_physical_bytes()
}

#[cfg(target_os = "linux")]
pub fn available_physical_bytes() -> Option<u64> {
    linux_available_physical_bytes()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn available_physical_bytes() -> Option<u64> {
    None
}

/// Read currently available physical memory for Windows CPU admission.
/// An unavailable or inconsistent reading remains unknown to the caller's policy.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn windows_available_physical_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: status is initialized, correctly sized, and exclusively borrowed
    // for the synchronous call. The API retains no pointer after returning.
    let succeeded = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    validated_windows_available_bytes(succeeded, status.ullTotalPhys, status.ullAvailPhys)
}

/// Read free plus inactive pages from Darwin's host VM statistics.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[allow(deprecated)]
fn macos_available_physical_bytes() -> Option<u64> {
    let mut statistics = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: the output buffer is correctly sized for HOST_VM_INFO64, count
    // describes that buffer, and both pointers remain valid for the call.
    let result = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            statistics.as_mut_ptr().cast(),
            &mut count,
        )
    };
    // New SDKs can define vm_statistics64 with fields that an older running
    // kernel does not return. Only the first three natural_t fields are needed
    // here (free, active, inactive), so accept that stable prefix rather than
    // requiring the SDK's full current structure size.
    const REQUIRED_FIELD_COUNT: libc::mach_msg_type_number_t = 3;
    if result != libc::KERN_SUCCESS || count < REQUIRED_FIELD_COUNT {
        return None;
    }
    // SAFETY: the buffer was zero-initialized, and the returned prefix includes
    // every field read below.
    let statistics = unsafe { statistics.assume_init() };
    // SAFETY: vm_page_size is initialized by the Darwin runtime before main.
    let page_size = unsafe { libc::vm_page_size } as u64;
    validated_macos_available_bytes(
        page_size,
        u64::from(statistics.free_count),
        u64::from(statistics.inactive_count),
    )
}

#[cfg(any(target_os = "macos", test))]
fn validated_macos_available_bytes(page_size: u64, free: u64, inactive: u64) -> Option<u64> {
    (page_size > 0)
        .then(|| free.checked_add(inactive)?.checked_mul(page_size))
        .flatten()
}

#[cfg(target_os = "linux")]
fn linux_available_physical_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_linux_available_bytes(&meminfo)
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_available_bytes(meminfo: &str) -> Option<u64> {
    let available = linux_meminfo_value_kib(meminfo, "MemAvailable")?;
    let total = linux_meminfo_value_kib(meminfo, "MemTotal")?;
    if available == 0 || total == 0 || available > total {
        return None;
    }
    available.checked_mul(1024)
}

#[cfg(any(target_os = "linux", test))]
fn linux_meminfo_value_kib(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let (found_key, value) = line.split_once(':')?;
        if found_key != key {
            return None;
        }
        let mut parts = value.split_whitespace();
        let kib = parts.next()?.parse().ok()?;
        matches!(parts.next(), Some("kB")).then_some(kib)
    })
}

#[cfg(any(windows, test))]
fn validated_windows_available_bytes(succeeded: bool, total: u64, available: u64) -> Option<u64> {
    (succeeded && total > 0 && available <= total).then_some(available)
}

#[derive(Debug, Default)]
pub struct MemoryAdmissionCache {
    resolved_floor_bytes: Option<u64>,
    unified_memory: Option<bool>,
}

static WARNED_UNRELIABLE_MEMORY: AtomicBool = AtomicBool::new(false);

pub fn resolve_memory_floor_bytes(
    cache: &mut MemoryAdmissionCache,
    config: &serde_json::Value,
    platform: &str,
    arch: &str,
    unified_memory: impl FnOnce() -> bool,
    total_bytes: impl FnOnce() -> Option<u64>,
    stt_floor_bytes: Option<u64>,
) -> u64 {
    if let Some(value) = cache.resolved_floor_bytes {
        return value;
    }
    let explicit = config
        .get("memory")
        .and_then(serde_json::Value::as_object)
        .and_then(|memory| memory.get("floor_mib"))
        .and_then(serde_json::Value::as_u64);
    let floor = if let Some(mib) = explicit {
        mib.saturating_mul(MIB)
    } else {
        let unified = *cache.unified_memory.get_or_insert_with(|| {
            platform.eq_ignore_ascii_case("darwin") && arch.eq_ignore_ascii_case("arm64")
                || unified_memory()
        });
        if !unified {
            0
        } else {
            // A gate floor below the STT local floor would admit transcribe jobs that then silently downgrade off the local backend.
            let lower = stt_floor_bytes.unwrap_or(2 * GIB).saturating_add(GIB);
            total_bytes()
                .map(|total| percentage_floor(total).max(lower).min(12 * GIB))
                .unwrap_or(lower)
        }
    };
    cache.resolved_floor_bytes = Some(floor);
    floor
}

fn percentage_floor(total: u64) -> u64 {
    (0.06_f64 * total as f64) as u64
}

/// Wait until the injected reading permits work. There is intentionally no
/// timeout, maximum iteration count, or caller policy hidden in this function.
pub fn wait_for_memory_headroom(
    floor: u64,
    should_stop: Option<&dyn Fn() -> bool>,
    read_available_bytes: &dyn Fn() -> Option<u64>,
    sleep: &dyn Fn(Duration),
    warn_unreliable_memory: &dyn Fn(),
) -> Duration {
    if floor == 0 {
        return Duration::ZERO;
    }
    let started = std::time::Instant::now();
    loop {
        if should_stop.is_some_and(|stop| stop()) {
            return started.elapsed();
        }
        let Some(available) = read_available_bytes() else {
            if !WARNED_UNRELIABLE_MEMORY.swap(true, Ordering::AcqRel) {
                warn_unreliable_memory();
            }
            return started.elapsed();
        };
        if available >= floor {
            return started.elapsed();
        }
        sleep(Duration::from_secs(1));
        if should_stop.is_some_and(|stop| stop()) {
            return started.elapsed();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use serde_json::json;

    use super::{
        GIB, MIB, MemoryAdmissionCache, parse_linux_available_bytes, resolve_memory_floor_bytes,
        validated_macos_available_bytes, wait_for_memory_headroom,
    };

    #[test]
    fn macos_available_bytes_are_free_plus_inactive_pages() {
        assert_eq!(
            validated_macos_available_bytes(16_384, 10, 20),
            Some(30 * 16_384)
        );
        assert_eq!(validated_macos_available_bytes(0, 10, 20), None);
        assert_eq!(validated_macos_available_bytes(u64::MAX, 1, 1), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn running_macos_reports_available_physical_memory() {
        assert!(super::available_physical_bytes().is_some());
    }

    #[test]
    fn linux_available_bytes_require_valid_available_and_total() {
        assert_eq!(
            parse_linux_available_bytes("MemTotal: 2048 kB\nMemAvailable: 1024 kB\n"),
            Some(1024 * 1024)
        );
        assert_eq!(
            parse_linux_available_bytes("MemTotal: 1024 kB\nMemAvailable: 2048 kB\n"),
            None
        );
        assert_eq!(
            parse_linux_available_bytes("MemTotal: 1024 kB\nMemAvailable: 0 kB\n"),
            None
        );
    }

    #[test]
    fn windows_memory_reading_preserves_failure_and_zero_headroom() {
        use super::validated_windows_available_bytes as validate;
        assert_eq!(validate(true, 8 * GIB, 4 * GIB), Some(4 * GIB));
        assert_eq!(validate(true, 8 * GIB, 0), Some(0));
        assert_eq!(validate(false, 8 * GIB, 4 * GIB), None);
        assert_eq!(validate(true, 0, 0), None);
        assert_eq!(validate(true, 4 * GIB, 8 * GIB), None);
    }

    #[test]
    fn resolution_cache_wins_before_a_second_config_read() {
        let mut cache = MemoryAdmissionCache::default();
        assert_eq!(
            resolve_memory_floor_bytes(
                &mut cache,
                &json!({"memory":{"floor_mib":7}}),
                "linux",
                "x86_64",
                || false,
                || None,
                None
            ),
            7 * 1024 * 1024
        );
        assert_eq!(
            resolve_memory_floor_bytes(
                &mut cache,
                &json!({"memory":{"floor_mib":9}}),
                "linux",
                "x86_64",
                || false,
                || None,
                None
            ),
            7 * 1024 * 1024
        );
    }

    #[test]
    fn floor_resolution_covers_auto_bounds_and_float_truncation() {
        let resolve = |config, platform, arch, unified, total, stt| {
            resolve_memory_floor_bytes(
                &mut MemoryAdmissionCache::default(),
                &config,
                platform,
                arch,
                || unified,
                || total,
                stt,
            )
        };
        assert_eq!(
            resolve(
                json!({"memory":{"floor_mib":7}}),
                "linux",
                "x86_64",
                false,
                None,
                None
            ),
            7 * MIB
        );
        assert_eq!(
            resolve(
                json!({"memory":{"floor_mib":"bad"}}),
                "Darwin",
                "arm64",
                false,
                None,
                None
            ),
            3 * GIB
        );
        assert_eq!(resolve(json!({}), "linux", "x86_64", false, None, None), 0);
        assert_eq!(
            resolve(json!({}), "Darwin", "arm64", false, None, Some(4 * GIB)),
            5 * GIB
        );
        assert_eq!(
            resolve(
                json!({}),
                "linux",
                "x86_64",
                true,
                Some(10 * GIB),
                Some(4 * GIB)
            ),
            5 * GIB
        );
        let between_total = 100 * GIB;
        assert_eq!(
            resolve(
                json!({}),
                "linux",
                "x86_64",
                true,
                Some(between_total),
                Some(4 * GIB)
            ),
            6 * GIB
        );
        assert_eq!(
            resolve(
                json!({}),
                "linux",
                "x86_64",
                true,
                Some(400 * GIB),
                Some(4 * GIB)
            ),
            12 * GIB
        );
        let total = u64::MAX;
        assert_eq!(
            super::percentage_floor(total),
            (0.06_f64 * total as f64) as u64
        );
        assert_ne!(
            super::percentage_floor(total),
            ((u128::from(total) * 6) / 100) as u64
        );
    }

    #[test]
    fn waiting_has_stop_and_no_stop_blocking_arms_without_expiry() {
        let polls = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_sleep = Arc::clone(&stop);
        let seen = Arc::clone(&polls);
        let _ = wait_for_memory_headroom(
            10,
            Some(&|| stop.load(Ordering::Relaxed)),
            &|| Some(0),
            &|_| {
                seen.fetch_add(1, Ordering::Relaxed);
                stop_for_sleep.store(true, Ordering::Relaxed);
            },
            &|| {},
        );
        assert_eq!(polls.load(Ordering::Relaxed), 1);

        let (sleep_seen, sleep_wait) = mpsc::channel();
        let release = Arc::new(AtomicBool::new(false));
        let release_worker = Arc::clone(&release);
        let read_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            wait_for_memory_headroom(
                10,
                None,
                &|| {
                    read_release
                        .load(Ordering::Acquire)
                        .then_some(10)
                        .or(Some(0))
                },
                &|_| {
                    sleep_seen.send(()).unwrap();
                    while !release_worker.load(Ordering::Acquire) {
                        thread::yield_now();
                    }
                },
                &|| {},
            )
        });
        sleep_wait.recv().unwrap();
        assert!(
            !worker.is_finished(),
            "no stop callback must not introduce an expiry"
        );
        release.store(true, Ordering::Release);
        let _ = worker.join();

        let (sleep_seen, sleep_wait) = mpsc::channel();
        let release = Arc::new(AtomicBool::new(false));
        let release_worker = Arc::clone(&release);
        let read_release = Arc::clone(&release);
        let worker = thread::spawn(move || {
            wait_for_memory_headroom(
                10,
                Some(&|| false),
                &|| {
                    read_release
                        .load(Ordering::Acquire)
                        .then_some(10)
                        .or(Some(0))
                },
                &|_| {
                    sleep_seen.send(()).unwrap();
                    while !release_worker.load(Ordering::Acquire) {
                        thread::yield_now();
                    }
                },
                &|| {},
            )
        });
        sleep_wait.recv().unwrap();
        assert!(
            !worker.is_finished(),
            "a false stop callback must not add an expiry"
        );
        release.store(true, Ordering::Release);
        let _ = worker.join();
    }

    #[test]
    fn unavailable_memory_warns_once_and_returns() {
        let warnings = AtomicUsize::new(0);
        let warn = || {
            warnings.fetch_add(1, Ordering::Relaxed);
        };
        let _ = wait_for_memory_headroom(1, None, &|| None, &|_| {}, &warn);
        let _ = wait_for_memory_headroom(1, None, &|| None, &|_| {}, &warn);
        assert_eq!(warnings.load(Ordering::Relaxed), 1);
    }
}
