// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};
use std::thread::{ThreadId, current};

use log::{Level, LevelFilter, Log, Metadata, Record};

static INIT: Once = Once::new();
static RECORDS: Mutex<Vec<(ThreadId, Level, String, String)>> = Mutex::new(Vec::new());
static PROCESS_RECORDS: Mutex<Vec<(Level, String, String)>> = Mutex::new(Vec::new());
static CAPTURE_LEVEL: AtomicUsize = AtomicUsize::new(LevelFilter::Info as usize);

struct TestLogger;

impl Log for TestLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let max_lvl = CAPTURE_LEVEL.load(Ordering::Relaxed);
        metadata.level() as usize <= max_lvl
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let target = record.target().to_string();
            let msg = record.args().to_string();
            if let Ok(mut records) = PROCESS_RECORDS.lock() {
                records.push((record.level(), target.clone(), msg.clone()));
            }
            if let Ok(mut records) = RECORDS.lock() {
                records.push((current().id(), record.level(), target, msg));
            }
        }
    }

    fn flush(&self) {}
}

static LOGGER: TestLogger = TestLogger;

static PROCESS_CAPTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Held for the whole of a test that clears or reads the process-wide
/// capture, or changes its level, so another such test cannot clear it
/// partway through. Tests that read only their own thread's records need not
/// hold it.
pub(crate) async fn process_capture() -> tokio::sync::MutexGuard<'static, ()> {
    PROCESS_CAPTURE.lock().await
}

pub(crate) fn init() {
    INIT.call_once(|| {
        let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Info));
    });
}

pub(crate) fn set_capture_level(filter: LevelFilter) {
    init();
    CAPTURE_LEVEL.store(filter as usize, Ordering::Relaxed);
    log::set_max_level(filter);
}

pub(crate) fn clear() {
    init();
    if let Ok(mut records) = PROCESS_RECORDS.lock() {
        records.clear();
    }
}

pub(crate) fn records() -> Vec<(Level, String, String)> {
    init();
    if let Ok(records) = PROCESS_RECORDS.lock() {
        records.clone()
    } else {
        Vec::new()
    }
}

pub(crate) fn clear_current_thread() {
    init();
    let thread_id = current().id();
    if let Ok(mut records) = RECORDS.lock() {
        records.retain(|(id, _, _, _)| *id != thread_id);
    }
}

pub(crate) fn records_for_current_thread() -> Vec<(Level, String, String)> {
    init();
    let thread_id = current().id();
    if let Ok(records) = RECORDS.lock() {
        records
            .iter()
            .filter(|(id, _, _, _)| *id == thread_id)
            .map(|(_, level, target, msg)| (*level, target.clone(), msg.clone()))
            .collect()
    } else {
        Vec::new()
    }
}
