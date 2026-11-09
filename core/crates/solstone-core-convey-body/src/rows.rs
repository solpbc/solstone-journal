// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A body window's rows, as the Body app reads them.

use std::collections::BTreeMap;
use std::path::Path;

use solstone_core_format::content::body_records::{
    BODY_RECORD_STREAMS, BODY_RECORDS_FILE, BodyRow, decode_rows, one_copy_per_row,
};

use crate::listing::{is_deleted, window_names};

/// Every read of a window's file goes through here, so a test can say what a
/// request read.
pub(crate) fn read_window_text(path: &Path) -> std::io::Result<String> {
    #[cfg(test)]
    window_read_probe::record();
    std::fs::read_to_string(path)
}

#[cfg(test)]
pub(crate) mod window_read_probe {
    use std::cell::Cell;

    thread_local!(static COUNT: Cell<u64> = const { Cell::new(0) });

    pub(crate) fn record() {
        COUNT.with(|count| count.set(count.get() + 1));
    }

    /// The window-file reads this thread has made since the last reset.
    pub(crate) fn reads() -> u64 {
        COUNT.with(Cell::get)
    }

    pub(crate) fn reset() {
        COUNT.with(|count| count.set(0));
    }
}

/// A stream's rows on a day, each row once, with how many windows couldn't be
/// read. A window deleted or holding only moved rows has none.
pub(crate) fn stream_rows(root: &Path, day: &str, stream: &str) -> (Vec<(String, Vec<BodyRow>)>, u64) {
    let mut windows = Vec::new();
    let mut unreadable = 0;
    for name in window_names(root, day, stream) {
        if is_deleted(root, day, stream, &name) {
            continue;
        }
        let path = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(&name)
            .join(BODY_RECORDS_FILE);
        match read_window_text(&path) {
            Ok(text) => match decode_rows(&text) {
                Ok(rows) => windows.push((name, rows)),
                Err(_) => unreadable += 1,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => unreadable += 1,
        }
    }
    (one_copy_per_row(windows), unreadable)
}

/// A day's rows by source: Apple Health and Oura, each row once.
pub(crate) fn day_rows(root: &Path, day: &str) -> (BTreeMap<String, Vec<BodyRow>>, u64) {
    let mut by_source = BTreeMap::<String, Vec<BodyRow>>::new();
    let mut unreadable = 0;
    for stream in BODY_RECORD_STREAMS {
        let (windows, failed) = stream_rows(root, day, stream);
        unreadable += failed;
        for (_, rows) in windows {
            for row in rows {
                by_source.entry(row.source.clone()).or_default().push(row);
            }
        }
    }
    (by_source, unreadable)
}
