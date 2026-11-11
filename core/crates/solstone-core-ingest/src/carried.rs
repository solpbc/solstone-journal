// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a device's latest accepted upload carried, read from that segment's
//! own custody receipts.
//!
//! A device's own capture uploads under no source name (a computer's screen
//! and audio) or as `mobile-segment` (a phone's). The ledger keys its accept
//! time by that name, so it cannot say which kinds arrived. The receipts the
//! journal already keeps for the segment can: they list every file the upload
//! wrote. This reads them; it records nothing.

use std::collections::BTreeSet;
use std::path::Path;

use solstone_core_callosum::DeviceIngestEvent;
use solstone_core_segment::{SegmentDir, visible_stream_names};

use crate::listing::segment_events;

/// A kind of capture an upload can carry inside one segment.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CarriedKind {
    Audio,
    Screen,
    Location,
}

impl CarriedKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Screen => "screen",
            Self::Location => "location",
        }
    }
}

/// The kinds of capture this device's upload into `day`/`segment` under
/// `source` carried, in a fixed order.
///
/// Any read failure, a missing segment, or a receipt that is not this
/// device's returns an empty set: the caller then shows only what the ledger
/// already says, never a kind it did not see.
pub fn carried_kinds(
    journal_root: &Path,
    cid: &str,
    source: &str,
    day: &str,
    segment: &str,
) -> BTreeSet<CarriedKind> {
    let Ok(streams) = visible_stream_names(journal_root, cid, source) else {
        return BTreeSet::new();
    };
    let mut kinds = BTreeSet::new();
    for stream in streams {
        let Ok(dir) = SegmentDir::resolve(journal_root, day, segment, &stream) else {
            continue;
        };
        if !dir.path().is_dir() {
            continue;
        }
        let Ok(events) =
            segment_events(journal_root, dir.path(), day, &stream, segment, cid, source)
        else {
            continue;
        };
        kinds.extend(events.iter().flat_map(event_kinds));
    }
    kinds
}

fn event_kinds(event: &DeviceIngestEvent) -> impl Iterator<Item = CarriedKind> + '_ {
    event
        .files
        .iter()
        .filter_map(|file| file_kind(&file.submitted))
}

/// The kind of capture a submitted file name holds, when the name says so.
fn file_kind(name: &str) -> Option<CarriedKind> {
    let lowered = name.to_ascii_lowercase();
    let (stem, extension) = lowered.rsplit_once('.')?;
    match extension {
        "flac" | "opus" | "ogg" | "m4a" | "mp3" | "wav" => Some(CarriedKind::Audio),
        "webm" | "mp4" | "mov" => Some(CarriedKind::Screen),
        "jsonl" if stem == "location" || stem.ends_with("_location") => Some(CarriedKind::Location),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_name_their_kind() {
        // What the computers' and the phone's own capture uploads carry.
        for name in [
            "120000_300_audio.m4a",
            "audio.flac",
            "mic_audio.flac",
            "audio.m4a",
        ] {
            assert_eq!(file_kind(name), Some(CarriedKind::Audio), "{name}");
        }
        for name in [
            "120000_300_display_1_screen.mp4",
            "left_DP-1_screen.webm",
            "display_1_screen.mp4",
            "screen.mp4",
        ] {
            assert_eq!(file_kind(name), Some(CarriedKind::Screen), "{name}");
        }
        assert_eq!(file_kind("location.jsonl"), Some(CarriedKind::Location));
        // Other line files are not location, and an unknown file is no kind.
        for name in [
            "120000_300_protected_video.jsonl",
            "120000_300_abc_desktop_activity.jsonl",
            "browser_pages.jsonl",
            "metadata.jsonl",
            "mic.pcm",
            "noextension",
        ] {
            assert_eq!(file_kind(name), None, "{name}");
        }
    }
}
