// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::{Mutex, Once};
use std::thread::{ThreadId, current};

use log::{Level, LevelFilter, Log, Metadata, Record};

static INIT: Once = Once::new();
static RECORDS: Mutex<Vec<(ThreadId, Level, String)>> = Mutex::new(Vec::new());
static PROCESS_RECORDS: Mutex<Vec<(Level, String)>> = Mutex::new(Vec::new());

struct TestLogger;

impl Log for TestLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            if let Ok(mut records) = PROCESS_RECORDS.lock() {
                records.push((record.level(), record.args().to_string()));
            }
            if let Ok(mut records) = RECORDS.lock() {
                records.push((current().id(), record.level(), record.args().to_string()));
            }
        }
    }

    fn flush(&self) {}
}

static LOGGER: TestLogger = TestLogger;

pub(crate) fn init() {
    INIT.call_once(|| {
        let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Info));
    });
}

pub(crate) fn clear() {
    init();
    if let Ok(mut records) = PROCESS_RECORDS.lock() {
        records.clear();
    }
    if let Ok(mut records) = RECORDS.lock() {
        records.clear();
    }
}

pub(crate) fn records() -> Vec<(Level, String)> {
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
        records.retain(|(id, _, _)| *id != thread_id);
    }
}

pub(crate) fn records_for_current_thread() -> Vec<(Level, String)> {
    init();
    let thread_id = current().id();
    if let Ok(records) = RECORDS.lock() {
        records
            .iter()
            .filter(|(id, _, _)| *id == thread_id)
            .map(|(_, level, msg)| (*level, msg.clone()))
            .collect()
    } else {
        Vec::new()
    }
}
