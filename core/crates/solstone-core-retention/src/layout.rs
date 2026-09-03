// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Where a segment lives, journal-relative. **The only place this crate builds a
//! chronicle path.**
//!
//! # 🔴 The default stream has no directory
//!
//! A segment in the default stream lives at `chronicle/<day>/<dir>` — the stream
//! component is **absent**, not the literal `_default`. Both reference
//! implementations encode this rule and both express it as a branch, because there is
//! no way to derive it.
//!
//! Every earlier wave of this crate built the path by interpolating four components
//! unconditionally, which is right for a named stream and produces a path that does
//! not exist for the default one. The consequence was not a wrong deletion — an
//! absent path is refused — but a silent one: every default-stream segment would be
//! reported as *not removed, entry missing*, and a sweep would skip the owner's
//! oldest data forever while reporting success. That is why this is one function and
//! `tests/architecture.rs` forbids the interpolation anywhere else.

use std::path::Path;

use solstone_core_journal_io::paths::DEFAULT_STREAM;

/// Whether the journal has a chronicle directory to scan.
pub fn chronicle_root_is_dir(journal: &Path) -> bool {
    journal.join("chronicle").is_dir()
}

/// The directory holding a stream's segments, journal-relative.
pub fn stream_rel(day: &str, stream: &str) -> String {
    if stream == DEFAULT_STREAM {
        format!("chronicle/{day}")
    } else {
        format!("chronicle/{day}/{stream}")
    }
}

/// A segment directory, journal-relative.
///
/// ⛔ `dir` is the directory **name**, never a key parsed out of it.
pub fn segment_rel(day: &str, stream: &str, dir: &str) -> String {
    format!("{}/{}", stream_rel(day, stream), dir)
}

/// A file inside a segment, journal-relative.
pub fn content_rel(day: &str, stream: &str, dir: &str, name: &str) -> String {
    format!("{}/{}", segment_rel(day, stream, dir), name)
}

/// A canonical operational-log leaf inside one Chronicle day health directory.
pub fn oplog_rel(day: &str, leaf: &str) -> String {
    format!("chronicle/{day}/health/{leaf}")
}

/// The stream name for a segment found directly under a day directory.
pub fn default_stream() -> &'static str {
    DEFAULT_STREAM
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code; the crate-level denials exist to constrain the verbs"
)]
mod tests {
    use super::*;

    #[test]
    fn a_named_stream_carries_its_component() {
        assert_eq!(
            segment_rel("20260805", "field.audio", "070000_17"),
            "chronicle/20260805/field.audio/070000_17"
        );
        assert_eq!(
            content_rel("20260805", "field.audio", "070000_17", "audio.flac"),
            "chronicle/20260805/field.audio/070000_17/audio.flac"
        );
    }

    /// 🔴 The rule the earlier waves got wrong.
    #[test]
    fn the_default_stream_has_no_component_at_all() {
        assert_eq!(
            segment_rel("20260805", DEFAULT_STREAM, "070000_17"),
            "chronicle/20260805/070000_17"
        );
        assert_eq!(
            content_rel("20260805", DEFAULT_STREAM, "070000_17", "audio.flac"),
            "chronicle/20260805/070000_17/audio.flac"
        );
        assert_eq!(stream_rel("20260805", DEFAULT_STREAM), "chronicle/20260805");
    }

    /// The literal must never appear as a path component.
    #[test]
    fn the_default_stream_name_never_reaches_a_path() {
        for built in [
            stream_rel("20260805", DEFAULT_STREAM),
            segment_rel("20260805", DEFAULT_STREAM, "070000_17"),
            content_rel("20260805", DEFAULT_STREAM, "070000_17", "audio.flac"),
        ] {
            assert!(
                !built.contains(DEFAULT_STREAM),
                "`{built}` names the default stream, which is not a directory"
            );
        }
    }

    /// A stream whose name merely resembles the default is a real directory.
    #[test]
    fn only_the_exact_default_name_is_elided() {
        for stream in ["_defaults", "default", "_Default", "_default "] {
            let built = segment_rel("20260805", stream, "070000_17");
            assert_eq!(
                built,
                format!("chronicle/20260805/{stream}/070000_17"),
                "`{stream}` is not the default stream"
            );
        }
    }
}
