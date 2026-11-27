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
use solstone_core_format::folder::{FOLDER_ACTIVITY_FILE, FOLDER_SOURCE, is_folder_source_name};
use solstone_core_format::obsidian::OBSIDIAN_SOURCE;
use solstone_core_format::screenshots::SCREENSHOTS_SOURCE;
use solstone_core_journal_io::{PathOrDay, Segment, iter_segments};

use crate::content_name::is_reserved_name;
use crate::segment_dir::is_safe_stream_component;
use crate::stream_record::{read_typed_stream_record, stream_record_path};

/// Syncing sources whose historical (`day < today`) arrivals schedule in Band 4
/// (`WorkBand::Import`) and settle immediately without writing an import
/// manifest (`imports/*/imported.json`).
pub const SYNCING_SOURCES: &[&str] = &[
    CALENDAR_SOURCE,
    FOLDER_SOURCE,
    OBSIDIAN_SOURCE,
    SCREENSHOTS_SOURCE,
    "granola",
    "email",
    "photos",
];

/// Whether `source` names a syncing source that may deliver historical days.
#[must_use]
pub fn is_syncing_source_name(source: &str) -> bool {
    SYNCING_SOURCES.contains(&source) || is_folder_source_name(source)
}

/// Whether `stream` belongs to a syncing source (bound via `streams/<stream>.json`,
/// `photos`, `import.email`, or a Granola source stream `import.granola.<id>`).
#[must_use]
pub fn is_syncing_source_stream(journal: &Path, stream: &str) -> bool {
    if !is_safe_stream_component(stream) {
        return false;
    }
    if stream.starts_with("import.granola.")
        || stream == PHOTOS_SOURCE
        || stream == EMAIL_SYNC_STREAM
    {
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
/// backfill stream (`import.*` or a syncing source), whenever it arrived. Days
/// holding no segments (or holding day-level live artifacts alone) return
/// `false`.
#[must_use]
pub fn day_holds_only_backfill_streams(journal: &Path, day: &str) -> bool {
    backfill_only_streams(journal, day).is_some()
}

/// Whether `stream` on `day` arrived as historical backfill rather than live
/// capture as the day happened.
///
/// File imports (`import.*` excluding syncing sources and `import.archive`) are
/// always history. Syncing sources (including `photos`, `import.email`,
/// `import.granola.*`, and bound syncing streams) are history on days before
/// their stream record's creation day, and live capture on or after it.
#[must_use]
pub fn stream_arrived_as_history_on_day(journal: &Path, stream: &str, day: &str) -> bool {
    if !is_backfill_stream(journal, stream) {
        return false;
    }
    if !is_syncing_source_stream(journal, stream) {
        return true;
    }
    match read_typed_stream_record(&stream_record_path(journal, stream)) {
        Ok(Some(record)) if record.created_at > 0 => {
            i64::try_from(record.created_at.saturating_mul(1_000))
                .ok()
                .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                .is_none_or(|began| {
                    day < solstone_core_journal_config::owner_day(journal, began).as_str()
                })
        }
        _ => true,
    }
}

/// Whether `day` holds at least one live (non-tombstoned, non-body-only)
/// segment that arrived as historical backfill, even if the day also holds
/// live capture.
#[must_use]
pub fn day_holds_historical_backfill(journal: &Path, day: &str) -> bool {
    let Ok(segments) = iter_segments(journal, PathOrDay::Day(day)) else {
        return false;
    };
    let mut checked = std::collections::BTreeSet::new();
    for segment in segments.iter().filter(|segment| {
        !segment.path().join("tombstone.json").is_file()
            && !crate::is_body_only_segment(segment)
            && !crate::is_transactions_segment(segment)
            && !is_activity_only_folder_segment(segment)
            && !is_calendar_segment(journal, segment)
    }) {
        let Some(name) = segment.stream().directory().and_then(|s| s.to_str()) else {
            continue;
        };
        if !checked.insert(name.to_owned()) {
            continue;
        }
        if stream_arrived_as_history_on_day(journal, name, day) {
            return true;
        }
    }
    false
}

/// Whether everything `day` holds arrived as history: every segment sits in a
/// backfill stream, and none of them was captured as the day happened.
///
/// A file import (`import.*`) is history whenever it arrives: what it holds
/// was made somewhere else and brought in later. A syncing source (a bound
/// syncing stream, `photos`, `import.granola.*`, and `import.email`) keeps
/// delivering after it is connected, so a day on or after the day its stream
/// record was created is a day the owner captured through it, as much a day
/// of the journal as any other. Every syncing source writes that record with
/// its first segment; one whose record is missing or unreadable gives no
/// start to compare with, and its days read as history.
#[must_use]
pub fn day_is_backfill_only(journal: &Path, day: &str) -> bool {
    let Some(streams) = backfill_only_streams(journal, day) else {
        return false;
    };
    streams
        .iter()
        .all(|stream| stream_arrived_as_history_on_day(journal, stream, day))
}

/// The stream the email source syncs into. It is named like a file import
/// and carries no source binding, so its name is all that tells it apart.
const EMAIL_SYNC_STREAM: &str = "import.email";

/// The streams of a day whose every live segment sits in a backfill stream,
/// or `None` when the day holds no segment or holds a captured one.
fn backfill_only_streams(journal: &Path, day: &str) -> Option<std::collections::BTreeSet<String>> {
    let segments = iter_segments(journal, PathOrDay::Day(day)).ok()?;
    let mut streams = std::collections::BTreeSet::new();
    for segment in segments
        .iter()
        .filter(|segment| !segment.path().join("tombstone.json").is_file())
    {
        let name = segment.stream().directory()?.to_str()?;
        // One read of a stream's record, however many segments it holds.
        if streams.contains(name) {
            continue;
        }
        if !is_backfill_stream(journal, name) {
            return None;
        }
        streams.insert(name.to_owned());
    }
    (!streams.is_empty()).then_some(streams)
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

/// Syncing source name for synced photo-library copies.
pub const PHOTOS_SOURCE: &str = "photos";

/// Whether `stream` is bound to the email source.
pub fn is_email_stream(journal: &Path, stream: &str) -> bool {
    is_safe_stream_component(stream)
        && matches!(
            read_typed_stream_record(&stream_record_path(journal, stream)),
            Ok(Some(record)) if record.source.as_deref() == Some("email")
        )
}

/// Whether `stream` is bound to the syncing photos source (photo library copies).
pub fn is_photos_stream(journal: &Path, stream: &str) -> bool {
    is_safe_stream_component(stream)
        && matches!(
            read_typed_stream_record(&stream_record_path(journal, stream)),
            Ok(Some(record)) if record.source.as_deref() == Some(PHOTOS_SOURCE)
        )
}

/// Whether `stream` is bound to a watched-folder source (`folder` or `folder_<12 hex>`).
pub fn is_folder_stream(journal: &Path, stream: &str) -> bool {
    is_safe_stream_component(stream)
        && matches!(
            read_typed_stream_record(&stream_record_path(journal, stream)),
            Ok(Some(record)) if record.source.as_deref().is_some_and(is_folder_source_name)
        )
}

/// Whether `dir` is an activity-only watched-folder segment: it carries
/// `talents/folder_activity.json`, has no `talents/sense.json`, and holds no
/// top-level content files outside journal bookkeeping sidecars and subdirs.
///
/// Activity-only folder segments are searchable immediately under `screen_text`
/// and carry any owner-chosen facet for access control, but owe zero model
/// passes and must not wake daily facet talents.
#[must_use]
pub fn is_activity_only_folder_dir(dir: &Path) -> bool {
    let talents = dir.join("talents");
    if !talents.join(FOLDER_ACTIVITY_FILE).is_file() || talents.join("sense.json").is_file() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            return false;
        };
        if ft.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return false;
        };
        if is_reserved_name(&name) || name.ends_with(".lock") {
            continue;
        }
        return false;
    }
    true
}

/// Whether a listed segment is an activity-only watched-folder segment.
#[must_use]
pub fn is_activity_only_folder_segment(segment: &Segment) -> bool {
    is_activity_only_folder_dir(segment.path())
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
    #[test]
    fn a_synced_day_is_history_only_before_its_stream_began() {
        let journal = TempDir::new();
        let root = journal.path();
        fs::create_dir_all(root.join("config")).unwrap();
        // Set up on 2026-03-01; the screenshots stream began on 2026-06-01.
        fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"},"setup":{"completed_at":1772366400000}}"#,
        )
        .unwrap();
        stream(root, "shots", Some("screenshots"));
        let record = root.join("streams/shots.json");
        let began = fs::read_to_string(&record)
            .unwrap()
            .replace(r#""created_at":0"#, r#""created_at":1780315200"#);
        fs::write(&record, began).unwrap();
        let shot = |day: &str, stream: &str| {
            let dir = root
                .join("chronicle")
                .join(day)
                .join(stream)
                .join("101500_1");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("screenshot_screen.png"), "png").unwrap();
        };
        shot("20260531", "shots");
        shot("20260601", "shots");
        assert!(day_is_backfill_only(root, "20260531"));
        assert!(!day_is_backfill_only(root, "20260601"));
        // Whenever it arrived, each of those days holds only a syncing stream.
        assert!(day_holds_only_backfill_streams(root, "20260531"));
        assert!(day_holds_only_backfill_streams(root, "20260601"));
        // A file import is history whenever it arrived, record or none.
        shot("20260228", "import.image");
        shot("20260615", "import.image");
        assert!(day_is_backfill_only(root, "20260228"));
        assert!(day_is_backfill_only(root, "20260615"));
        stream(root, "import.image", None);
        assert!(day_is_backfill_only(root, "20260615"));
        // A day a file import and a synced stream both hold is history only
        // if it is before the synced stream began.
        shot("20260615", "shots");
        assert!(!day_is_backfill_only(root, "20260615"));
        shot("20260228", "shots");
        assert!(day_is_backfill_only(root, "20260228"));
        // Synced email lands in a stream named like an import: it began when
        // its record was written, and without one its days are history.
        shot("20260702", "import.email");
        assert!(day_is_backfill_only(root, "20260702"));
        stream(root, "import.email", None);
        let record = root.join("streams/import.email.json");
        let began = fs::read_to_string(&record)
            .unwrap()
            .replace(r#""created_at":0"#, r#""created_at":1780315200"#);
        fs::write(&record, began).unwrap();
        assert!(!day_is_backfill_only(root, "20260702"));
        // import.email is itself a syncing source stream (Band 4 / backfill pause).
        assert!(is_syncing_source_stream(root, "import.email"));
        // Mixed live + historical email day reports day_holds_historical_backfill == true,
        // while pure live or live + live-captured synced day reports false.
        let live_day = root.join("chronicle/20260530/desk/090000_300");
        fs::create_dir_all(&live_day).unwrap();
        fs::write(live_day.join("audio.jsonl"), "{}").unwrap();
        assert!(!day_holds_historical_backfill(root, "20260530"));
        shot("20260530", "import.email");
        assert!(day_holds_historical_backfill(root, "20260530"));
        assert!(!day_holds_only_backfill_streams(root, "20260530"));
        // On/after import.email's created_at (2026-06-01), import.email arrived live, not as history.
        let recent_live = root.join("chronicle/20260702/desk/090000_300");
        fs::create_dir_all(&recent_live).unwrap();
        fs::write(recent_live.join("audio.jsonl"), "{}").unwrap();
        assert!(!day_holds_historical_backfill(root, "20260702"));
    }
}
