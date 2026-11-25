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
use solstone_core_format::obsidian::OBSIDIAN_SOURCE;
use solstone_core_format::screenshots::SCREENSHOTS_SOURCE;
use solstone_core_journal_io::{PathOrDay, Segment, iter_segments};

use crate::segment_dir::is_safe_stream_component;
use crate::stream_record::{read_typed_stream_record, stream_record_path};

/// Syncing sources whose historical (`day < today`) arrivals schedule in Band 4
/// (`WorkBand::Import`) and settle immediately without writing an import
/// manifest (`imports/*/imported.json`).
pub const SYNCING_SOURCES: &[&str] = &[
    CALENDAR_SOURCE,
    OBSIDIAN_SOURCE,
    SCREENSHOTS_SOURCE,
    "granola",
    "email",
    "photos",
];

/// Whether `source` names a syncing source that may deliver historical days.
#[must_use]
pub fn is_syncing_source_name(source: &str) -> bool {
    SYNCING_SOURCES.contains(&source)
}

/// Whether `stream` belongs to a syncing source (bound via `streams/<stream>.json`
/// or a Granola source stream `import.granola.<id>`).
#[must_use]
pub fn is_syncing_source_stream(journal: &Path, stream: &str) -> bool {
    if !is_safe_stream_component(stream) {
        return false;
    }
    if stream.starts_with("import.granola.") {
        return true;
    }
    matches!(
        read_typed_stream_record(&stream_record_path(journal, stream)),
        Ok(Some(record)) if record.source.as_deref().is_some_and(is_syncing_source_name)
    )
}

/// Whether `stream` carries file imports or syncing-source history (`import.*`
/// excluding `"import.archive"`, or a bound syncing source stream).
#[must_use]
pub fn is_backfill_stream(journal: &Path, stream: &str) -> bool {
    if !is_safe_stream_component(stream) || stream == "import.archive" {
        return false;
    }
    stream.starts_with("import.") || is_syncing_source_stream(journal, stream)
}

/// Whether `day` holds at least one segment and every segment sits in a
/// backfill stream (`import.*` or a syncing source). Days holding no segments
/// (or holding day-level live artifacts alone) return `false`.
#[must_use]
pub fn day_is_backfill_only(journal: &Path, day: &str) -> bool {
    iter_segments(journal, PathOrDay::Day(day)).is_ok_and(|segments| {
        !segments.is_empty()
            && segments.iter().all(|segment| {
                segment
                    .stream()
                    .directory()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| is_backfill_stream(journal, name))
            })
    })
}

/// Read the realistic setup-completion epoch millisecond timestamp from
/// `config/journal.json` (`setup.completed_at`), normalizing legacy epoch-second
/// floats when `>= 2000-01-01T00:00:00Z` (`946_684_800`).
#[must_use]
pub fn setup_completed_at_ms(journal: &Path) -> Option<i64> {
    let bytes = std::fs::read(journal.join("config/journal.json")).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let raw = value.get("setup")?.get("completed_at")?;
    if let Some(i) = raw
        .as_i64()
        .or_else(|| raw.as_u64().and_then(|u| i64::try_from(u).ok()))
    {
        if i >= 946_684_800_000 {
            return Some(i);
        }
        if i >= 946_684_800 {
            return i.checked_mul(1_000);
        }
        return None;
    }
    let f = raw.as_f64()?;
    if !f.is_finite() {
        return None;
    }
    if f >= 946_684_800_000.0 {
        Some(f as i64)
    } else if f >= 946_684_800.0 {
        Some((f * 1_000.0) as i64)
    } else {
        None
    }
}

/// Whether `stream` is bound to the calendar source. A stream whose record is
/// absent or unreadable is not one, so it keeps counting as it always has.
pub fn is_calendar_stream(journal: &Path, stream: &str) -> bool {
    is_safe_stream_component(stream)
        && matches!(
            read_typed_stream_record(&stream_record_path(journal, stream)),
            Ok(Some(record)) if record.source.as_deref() == Some(CALENDAR_SOURCE)
        )
}

/// Whether `stream` is bound to the Obsidian source: note changes reported
/// from a vault.
pub fn is_obsidian_stream(journal: &Path, stream: &str) -> bool {
    is_safe_stream_component(stream)
        && matches!(
            read_typed_stream_record(&stream_record_path(journal, stream)),
            Ok(Some(record)) if record.source.as_deref() == Some(OBSIDIAN_SOURCE)
        )
}

/// Whether `stream` is bound to the screenshots source: screenshots a device
/// sent, one to a segment.
pub fn is_screenshots_stream(journal: &Path, stream: &str) -> bool {
    is_safe_stream_component(stream)
        && matches!(
            read_typed_stream_record(&stream_record_path(journal, stream)),
            Ok(Some(record)) if record.source.as_deref() == Some(SCREENSHOTS_SOURCE)
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
    fn backfill_streams_unify_imports_and_syncing_sources_while_keeping_archive_out() {
        let journal = TempDir::new();
        let root = journal.path();
        stream(root, "mac_obsidian", Some("obsidian"));
        stream(root, "mac_shots", Some("screenshots"));
        stream(root, "mail_sync", Some("email"));
        stream(root, "photo_sync", Some("photos"));
        stream(root, "desk", Some("screen"));

        assert!(is_backfill_stream(root, "import.text"));
        assert!(is_backfill_stream(root, "import.image"));
        assert!(is_backfill_stream(root, "import.granola.a1b2"));
        assert!(is_backfill_stream(root, "mac_obsidian"));
        assert!(is_backfill_stream(root, "mac_shots"));
        assert!(is_backfill_stream(root, "mail_sync"));
        assert!(is_backfill_stream(root, "photo_sync"));
        assert!(!is_backfill_stream(root, "import.archive"));
        assert!(!is_backfill_stream(root, "desk"));

        fs::create_dir_all(root.join("chronicle/20150412/import.image/080000_0")).unwrap();
        fs::write(
            root.join("chronicle/20150412/import.image/080000_0/image_transcript.md"),
            "photo",
        )
        .unwrap();
        assert!(day_is_backfill_only(root, "20150412"));

        fs::create_dir_all(root.join("chronicle/20150412/desk/090000_300")).unwrap();
        fs::write(
            root.join("chronicle/20150412/desk/090000_300/audio.jsonl"),
            "{}",
        )
        .unwrap();
        assert!(!day_is_backfill_only(root, "20150412"));
    }
}
