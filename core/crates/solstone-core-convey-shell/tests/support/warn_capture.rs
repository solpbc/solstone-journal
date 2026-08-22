// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::{Mutex, Once, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

struct TestLogger;

static LOGGER: TestLogger = TestLogger;
static LOGGER_INIT: Once = Once::new();
static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

impl Log for TestLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            LOGS.get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .expect("warn capture lock")
                .push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

pub fn install_and_clear() {
    LOGGER_INIT.call_once(|| {
        log::set_logger(&LOGGER).expect("warn capture logger installs");
        log::set_max_level(LevelFilter::Warn);
    });
    clear();
}

pub fn clear() {
    LOGS.get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("warn capture lock")
        .clear();
}

pub fn contains(needle: &str) -> bool {
    LOGS.get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("warn capture lock")
        .iter()
        .any(|entry| entry.contains(needle))
}
