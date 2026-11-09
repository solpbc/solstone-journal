// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the journal holds, as directory listings.
//!
//! Nothing here opens a window: a day's body sources and how many windows it
//! holds come from the names in its folders.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use solstone_core_format::content::body_records::BODY_RECORD_STREAMS;
use solstone_core_format::content::strava_body::STRAVA_STREAM;

/// Every stream body data lands in, with its source key.
pub(crate) fn body_streams() -> [(&'static str, &'static str); 3] {
    [
        (BODY_RECORD_STREAMS[0], "apple_health"),
        (BODY_RECORD_STREAMS[1], "oura"),
        (STRAVA_STREAM, "strava"),
    ]
}

/// `HHMMSS_<length>`, as window folders are named.
fn is_window_name(name: &str) -> bool {
    name.split_once('_')
        .is_some_and(|(clock, rest)| {
            clock.len() == 6
                && clock.bytes().all(|byte| byte.is_ascii_digit())
                && !rest.is_empty()
        })
}

/// The window folders a stream holds on a day, by name. A deleted window keeps
/// a tombstone folder, so it's left out by name where a caller needs that.
pub(crate) fn window_names(root: &Path, day: &str, stream: &str) -> Vec<String> {
    let dir = root.join("chronicle").join(day).join(stream);
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            is_window_name(&name).then_some(name)
        })
        .collect()
}

fn holds_a_live_window(root: &Path, day: &str, stream: &str) -> bool {
    window_names(root, day, stream)
        .iter()
        .any(|name| !is_deleted(root, day, stream, name))
}

pub(crate) fn is_deleted(root: &Path, day: &str, stream: &str, name: &str) -> bool {
    root.join("chronicle")
        .join(day)
        .join(stream)
        .join(name)
        .join("tombstone.json")
        .exists()
}

/// Every day that holds a window, and the sources holding one that day.
pub(crate) fn body_days(root: &Path) -> Result<BTreeMap<String, BTreeSet<&'static str>>, String> {
    let days = solstone_core_journal_io::day_dirs(root).map_err(|error| error.to_string())?;
    let mut by_day = BTreeMap::new();
    for day in days.into_keys() {
        let sources = body_streams()
            .iter()
            .filter(|(stream, _)| holds_a_live_window(root, &day, stream))
            .map(|(_, source)| *source)
            .collect::<BTreeSet<_>>();
        if !sources.is_empty() {
            by_day.insert(day, sources);
        }
    }
    Ok(by_day)
}

/// Live windows a day holds, across every body stream.
pub(crate) fn window_count(root: &Path, day: &str) -> u64 {
    body_streams()
        .iter()
        .map(|(stream, _)| {
            window_names(root, day, stream)
                .iter()
                .filter(|name| !is_deleted(root, day, stream, name))
                .count() as u64
        })
        .sum()
}
