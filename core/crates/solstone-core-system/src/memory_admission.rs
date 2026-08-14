// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Injected memory-admission decisions for local GPU work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

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
        GIB, MIB, MemoryAdmissionCache, resolve_memory_floor_bytes, wait_for_memory_headroom,
    };

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
