// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_journal_io::{JsonWriteOptions, write_json};
use solstone_core_segment::StreamRecord;

/// Read a stream's registry state (`streams/<name>.json`). This crate does
/// not expose a bare reader beyond its bind/advance/resolve flow, so prune's
/// tail-repair reads and writes this file directly, exactly as it does for
/// the per-segment marker in `marker.rs`.
pub fn read_stream_state(journal: &Path, name: &str) -> Option<StreamRecord> {
    let bytes = fs::read(state_path(journal, name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Repair a stream's tail while preserving identity metadata and the
/// monotonic `seq` -- never regressing it below what is already recorded.
pub fn repair_stream_state_tail(
    journal: &Path,
    name: &str,
    last_day: Option<&str>,
    last_segment: Option<&str>,
    max_seq: u64,
) -> StreamRecord {
    let mut state = read_stream_state(journal, name).unwrap_or_else(|| StreamRecord {
        name: name.to_owned(),
        kind: "unknown".to_owned(),
        host: None,
        platform: None,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
        last_day: None,
        last_segment: None,
        seq: 0,
        did: None,
        source: None,
    });
    state.last_day = last_day.map(ToOwned::to_owned);
    state.last_segment = last_segment.map(ToOwned::to_owned);
    state.seq = state.seq.max(max_seq);
    let _ = write_json(
        state_path(journal, name),
        &state,
        JsonWriteOptions::default(),
    );
    state
}

fn state_path(journal: &Path, name: &str) -> std::path::PathBuf {
    journal.join("streams").join(format!("{name}.json"))
}
