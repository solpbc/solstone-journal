// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Streams bound to a linked device's calendar.
//!
//! A calendar stream holds notes about the owner's calendar: when an event was
//! added, moved or removed. They are not something the owner did, so nothing
//! that reads segments as the owner's activity counts them. A stream is a
//! calendar stream by the source it is bound to, and by nothing else.

use std::path::Path;

use solstone_core_format::calendar::CALENDAR_SOURCE;
use solstone_core_journal_io::{PathOrDay, Segment, iter_segments};

use crate::segment_dir::is_safe_stream_component;
use crate::stream_record::{read_typed_stream_record, stream_record_path};

/// Whether `stream` is bound to the calendar source. A stream whose record is
/// absent or unreadable is not one, so it keeps counting as it always has.
pub fn is_calendar_stream(journal: &Path, stream: &str) -> bool {
    is_safe_stream_component(stream)
        && matches!(
            read_typed_stream_record(&stream_record_path(journal, stream)),
            Ok(Some(record)) if record.source.as_deref() == Some(CALENDAR_SOURCE)
        )
}

/// Whether a listed segment sits in a calendar stream.
pub fn is_calendar_segment(journal: &Path, segment: &Segment) -> bool {
    segment
        .stream()
        .directory()
        .and_then(|name| name.to_str())
        .is_some_and(|name| is_calendar_stream(journal, name))
}

/// Whether every segment of `day` sits in a calendar stream, and it has at
/// least one. Such a day holds nothing the owner did.
pub fn day_holds_only_calendar(journal: &Path, day: &str) -> bool {
    iter_segments(journal, PathOrDay::Day(day)).is_ok_and(|segments| {
        !segments.is_empty()
            && segments
                .iter()
                .all(|segment| is_calendar_segment(journal, segment))
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TempDir;

    fn stream(journal: &Path, name: &str, source: Option<&str>) {
        fs::create_dir_all(journal.join("streams")).unwrap();
        let mut record = serde_json::json!({
            "name": name, "kind": "observer", "host": null, "platform": null,
            "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
            "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        });
        if let Some(source) = source {
            record["source"] = source.into();
        }
        fs::write(
            journal.join("streams").join(format!("{name}.json")),
            record.to_string(),
        )
        .unwrap();
    }

    fn segment(journal: &Path, day: &str, stream: &str, key: &str) {
        let dir = journal.join("chronicle").join(day).join(stream).join(key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("note.jsonl"), "{}\n").unwrap();
    }

    #[test]
    fn a_stream_is_a_calendar_stream_by_its_bound_source_only() {
        let journal = TempDir::new();
        let root = journal.path();
        stream(root, "phone_calendar", Some("calendar"));
        stream(root, "phone", Some("audio"));
        // A name that says calendar is not enough.
        stream(root, "calendar", Some("screen"));
        stream(root, "legacy", None);
        assert!(is_calendar_stream(root, "phone_calendar"));
        assert!(!is_calendar_stream(root, "phone"));
        assert!(!is_calendar_stream(root, "calendar"));
        assert!(!is_calendar_stream(root, "legacy"));
        assert!(!is_calendar_stream(root, "absent"));
        assert!(!is_calendar_stream(root, "../streams/phone_calendar"));
        fs::write(root.join("streams/broken.json"), "{").unwrap();
        assert!(!is_calendar_stream(root, "broken"));
    }

    #[test]
    fn a_day_holds_only_calendar_when_every_segment_is_a_calendar_segment() {
        let journal = TempDir::new();
        let root = journal.path();
        stream(root, "phone_calendar", Some("calendar"));
        stream(root, "phone", Some("audio"));
        segment(root, "20261019", "phone_calendar", "094000_300");
        segment(root, "20261019", "phone_calendar", "101000_300");
        segment(root, "20261020", "phone_calendar", "163000_300");
        segment(root, "20261020", "phone", "090000_300");
        assert!(day_holds_only_calendar(root, "20261019"));
        assert!(!day_holds_only_calendar(root, "20261020"));
        // A day with no segments at all keeps counting as it always has.
        fs::create_dir_all(root.join("chronicle/20261021/health")).unwrap();
        assert!(!day_holds_only_calendar(root, "20261021"));
        assert!(!day_holds_only_calendar(root, "20261022"));
    }
}
