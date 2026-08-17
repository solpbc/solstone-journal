// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

/// A segment's `stream.json` chain marker.
///
/// This deliberately duplicates the shape `solstone-core-segment` writes
/// during normal advancement (it does not expose a public rewrite of an
/// existing marker's predecessor pointers -- that crate's public surface is
/// scoped to appending new segments, not repairing survivors after a
/// deletion elsewhere). Path resolution for segments still goes through
/// `solstone_core_segment::SegmentDir`; only the marker's own read/write is
/// local to prune, since prune repairs markers segment-writing never revisits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMarker {
    pub stream: String,
    #[serde(default)]
    pub prev_day: Option<String>,
    #[serde(default)]
    pub prev_segment: Option<String>,
    pub seq: u64,
}

/// Read `stream.json` from a segment directory. Both a missing file and an
/// unreadable/malformed one collapse to `None`, matching the Python reader
/// this ports (`solstone/think/streams.py::read_segment_stream`) -- prune
/// only ever needs "is this marker usable," not why it is not.
pub fn read_segment_marker(segment_dir: &Path) -> Option<StreamMarker> {
    let bytes = fs::read(segment_dir.join("stream.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Rewrite `stream.json`, preserving `stream`/`seq` and only ever called with
/// a freshly-read marker whose predecessor pointers have been repaired.
pub fn write_segment_marker(
    segment_dir: &Path,
    marker: &StreamMarker,
) -> Result<(), solstone_core_journal_io::AtomicWriteError> {
    write_json(
        segment_dir.join("stream.json"),
        marker,
        JsonWriteOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;

    fn root(name: &str) -> std::path::PathBuf {
        reserve_temp_path(&format!("observer-prune-marker-{name}"))
    }

    #[test]
    fn missing_and_malformed_markers_both_read_as_none() {
        let dir = root("missing");
        fs::create_dir_all(&dir).expect("dir");
        assert_eq!(read_segment_marker(&dir), None);
        fs::write(dir.join("stream.json"), b"{not json").expect("write");
        assert_eq!(read_segment_marker(&dir), None);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn round_trips_a_marker() {
        let dir = root("roundtrip");
        fs::create_dir_all(&dir).expect("dir");
        let marker = StreamMarker {
            stream: "workstation".to_owned(),
            prev_day: Some("20260101".to_owned()),
            prev_segment: Some("090000_300".to_owned()),
            seq: 3,
        };
        write_segment_marker(&dir, &marker).expect("write");
        assert_eq!(read_segment_marker(&dir), Some(marker));
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
