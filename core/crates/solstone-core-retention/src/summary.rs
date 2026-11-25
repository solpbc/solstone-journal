// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only storage accounting for the Settings surface and shared custody helpers.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use solstone_core_format::audio_archive::{
    AUDIO_ARCHIVE_PLACEMENT_FILENAME, AudioArchivePlacement, is_audio_archive_transcript_name,
};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

use crate::{
    ContentName, MediaClassifier, content::JournalMedia, original_deletion, original_trash,
};

/// Import streams whose segments never carry raw audio/video originals.
const NON_MEDIA_IMPORT_STREAMS: &[&str] = &[
    "import.apple_health",
    "import.calendar",
    "import.chatgpt",
    "import.claude",
    "import.document",
    "import.evernote",
    "import.garmin",
    "import.gemini",
    "import.granola",
    "import.obsidian",
    "import.oura",
    "import.strava",
];

/// Whether `stream` is a known import stream that never creates raw media files.
#[must_use]
pub fn is_non_media_import_stream(stream: &str) -> bool {
    NON_MEDIA_IMPORT_STREAMS.contains(&stream)
}

/// Collect journal-relative segment paths belonging to imports that explicitly
/// opted out of keeping originals (`keep_original_at_import == false`).
///
/// Strictly skips hidden/staging directories under `imports/` (including `.uploads/`).
#[must_use]
pub fn import_segments_without_originals(journal_root: &Path) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let imports_dir = journal_root.join("imports");
    let Ok(entries) = fs::read_dir(imports_dir) else {
        return result;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str.starts_with('.') {
            continue;
        }
        let dir = entry.path();
        let keep_choice = fs::read(dir.join("import.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|meta| meta.get("keep_original_at_import")?.as_bool());
        if keep_choice != Some(false) {
            continue;
        }
        collect_import_segment_rels(&dir, &mut result);
    }
    result
}

fn collect_import_segment_rels(import_dir: &Path, out: &mut BTreeSet<String>) {
    if let Some(record) = fs::read(import_dir.join("imported.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        && let Some(segments) = record.get("segments").and_then(Value::as_array)
    {
        for segment in segments {
            if let (Some(day), Some(stream), Some(key)) = (
                segment.get("day").and_then(Value::as_str),
                segment.get("stream").and_then(Value::as_str),
                segment.get("segment").and_then(Value::as_str),
            ) {
                out.insert(crate::layout::segment_rel(day, stream, key));
            }
        }
    }
    if let Some(record) = fs::read(import_dir.join("audio-import.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        && let Some(segments) = record.get("created_segments").and_then(Value::as_array)
    {
        for segment in segments {
            if let (Some(day), Some(stream), Some(key)) = (
                segment.get("day").and_then(Value::as_str),
                segment.get("stream").and_then(Value::as_str),
                segment.get("key").and_then(Value::as_str),
            ) {
                out.insert(crate::layout::segment_rel(day, stream, key));
            }
        }
    }
}

/// Collect raw media filenames explicitly expected by a segment's manifests
/// (`placement.json` or `ingest.json`).
#[must_use]
pub fn manifest_expected_raw_names(segment_dir: &Path) -> BTreeSet<String> {
    let mut expected = BTreeSet::new();
    let placement_path = segment_dir.join(AUDIO_ARCHIVE_PLACEMENT_FILENAME);
    if let Ok(bytes) = fs::read(&placement_path)
        && let Ok(placement) = serde_json::from_slice::<AudioArchivePlacement>(&bytes)
    {
        for part in placement.parts {
            expected.insert(part.stored_name);
        }
    }
    let ingest_path = segment_dir.join("ingest.json");
    if let Ok(bytes) = fs::read(&ingest_path)
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && let Some(files) = value.get("files").and_then(Value::as_object)
    {
        for name in files.keys() {
            if ContentName::new(name).is_some_and(|valid| JournalMedia.is_owner_media(&valid)) {
                expected.insert(name.clone());
            }
        }
    }
    expected
}

/// Read the `raw` filename declared in a transcript sidecar's first JSONL header, if any.
#[must_use]
pub fn sidecar_declared_raw_name(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    for line in bytes.split(|&b| b == b'\n') {
        let trimmed = std::str::from_utf8(line).ok()?.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed).ok()?;
        return value
            .get("raw")
            .and_then(Value::as_str)
            .and_then(ContentName::new)
            .filter(|name| JournalMedia.is_owner_media(name))
            .map(ContentName::into_string);
    }
    None
}

/// Whether `path` is a Granola text-note `audio.jsonl` (which never had raw audio).
#[must_use]
pub fn is_granola_only_jsonl(path: &Path) -> bool {
    solstone_core_processing_record::jsonl_has_row_with_key(
        path,
        solstone_core_format::content::granola::HEADER_KEY,
    ) && !solstone_core_processing_record::jsonl_has_row_with_key(
        path,
        solstone_core_processing_record::vocab::AUDIO_TRANSCRIPT_ROW_KEY,
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageSummary {
    pub raw_media_bytes: u64,
    pub derived_bytes: u64,
    pub total_segments: u64,
    pub segments_with_raw: u64,
    pub segments_purged: u64,
    pub segments_missing_unrecorded: u64,
    pub segments_damaged_zero_byte: u64,
}

impl StorageSummary {
    pub fn raw_media_human(self) -> String {
        human_bytes(self.raw_media_bytes)
    }

    pub fn derived_human(self) -> String {
        human_bytes(self.derived_bytes)
    }
}

pub fn compute_storage_summary(journal_root: &Path) -> io::Result<StorageSummary> {
    let chronicle = journal_root.join("chronicle");
    let days = match fs::read_dir(chronicle) {
        Ok(days) => days,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(StorageSummary::default());
        }
        Err(error) => return Err(error),
    };
    let import_no_media = import_segments_without_originals(journal_root);
    let trashed_segments = original_trash::segments_with_originals_in_trash(journal_root);
    let mut summary = StorageSummary::default();
    for day in days {
        let day = day?;
        if !day.file_type()?.is_dir() {
            continue;
        }
        let Some(day_name) = day.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let segments = iter_segments(journal_root, PathOrDay::Directory(&day.path()))
            .map_err(io::Error::other)?;
        for segment in segments {
            summary.total_segments = summary.total_segments.saturating_add(1);
            let facts = segment_facts(segment.path(), true)?;
            summary.raw_media_bytes = summary.raw_media_bytes.saturating_add(facts.raw_bytes);
            summary.derived_bytes = summary.derived_bytes.saturating_add(facts.derived_bytes);
            if facts.has_zero_byte_raw {
                summary.segments_damaged_zero_byte =
                    summary.segments_damaged_zero_byte.saturating_add(1);
            }
            if facts.has_non_zero_raw {
                summary.segments_with_raw = summary.segments_with_raw.saturating_add(1);
                continue;
            }
            if facts.has_zero_byte_raw {
                // Only 0-byte raw files exist: accounted under damaged_zero_byte, never purged or with_raw.
                continue;
            }
            let recorded = original_deletion::recorded_original_deletions(segment.path());
            let expected_manifests = manifest_expected_raw_names(segment.path());
            let expects_raw =
                facts.has_media_sidecar || !expected_manifests.is_empty() || !recorded.is_empty();
            if !expects_raw {
                continue;
            }
            let stream_name = segment
                .record_identity()
                .ok()
                .map(|identity| identity.stream.to_owned())
                .unwrap_or_default();
            let seg_name = segment
                .name()
                .to_str()
                .map(str::to_owned)
                .unwrap_or_default();
            let seg_rel = crate::layout::segment_rel(&day_name, &stream_name, &seg_name);
            let import_without_media =
                is_non_media_import_stream(&stream_name) || import_no_media.contains(&seg_rel);
            if !recorded.is_empty() || import_without_media {
                summary.segments_purged = summary.segments_purged.saturating_add(1);
            } else if trashed_segments.contains(&seg_rel) {
                // Recoverable in Trash: neither purged nor missing-unrecorded.
            } else {
                // Unrecorded missing original (including owner-acknowledged ones,
                // which silence integrity alerts without turning loss into purged).
                summary.segments_missing_unrecorded =
                    summary.segments_missing_unrecorded.saturating_add(1);
            }
        }
    }
    Ok(summary)
}

#[derive(Default)]
struct SegmentFacts {
    raw_bytes: u64,
    derived_bytes: u64,
    has_non_zero_raw: bool,
    has_zero_byte_raw: bool,
    has_media_sidecar: bool,
}

// A single traversal counts both categories and propagates its read errors.
// Symlink entries encountered within a segment are skipped, avoiding recursive
// cycles. Segment discovery retains the shared iter_segments contract.
fn segment_facts(path: &Path, immediate: bool) -> io::Result<SegmentFacts> {
    let mut facts = SegmentFacts::default();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let nested = segment_facts(&entry.path(), false)?;
            facts.derived_bytes = facts.derived_bytes.saturating_add(nested.derived_bytes);
        } else if kind.is_file() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().and_then(ContentName::new) else {
                continue;
            };
            let bytes = entry.metadata()?.len();
            if JournalMedia.is_owner_media(&name) {
                if immediate {
                    facts.raw_bytes = facts.raw_bytes.saturating_add(bytes);
                    if bytes == 0 {
                        facts.has_zero_byte_raw = true;
                    } else {
                        facts.has_non_zero_raw = true;
                    }
                }
            } else {
                facts.derived_bytes = facts.derived_bytes.saturating_add(bytes);
            }
            if immediate {
                let name_str = name.as_str();
                let is_media_jsonl = (name_str == "audio.jsonl"
                    && !is_granola_only_jsonl(&entry.path()))
                    || name_str == "screen.jsonl"
                    || name_str.ends_with("_screen.jsonl")
                    || is_audio_archive_transcript_name(name_str)
                    || solstone_core_processing_record::is_transcribe_output(&entry.path());
                facts.has_media_sidecar |= is_media_jsonl;
            }
        }
    }
    Ok(facts)
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "KiB";
    for candidate in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = candidate;
    }
    if value >= 1024.0 && unit == "TiB" {
        format!("{:.1} PiB", value / 1024.0)
    } else {
        format!("{value:.1} {unit}")
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]
mod tests {
    use std::fs;

    use super::{compute_storage_summary, human_bytes};
    use crate::original_deletion::record_original_missing_acknowledged;
    use tempfile::TempDir;

    #[test]
    fn headered_speech_counts_as_purged_only_with_recorded_deletion_otherwise_missing_unrecorded()
    -> std::io::Result<()> {
        let temporary = TempDir::new()?;
        let speech_purged = temporary
            .path()
            .join("chronicle/20261024/phone.audio/090000_60");
        let speech_missing = temporary
            .path()
            .join("chronicle/20261024/phone.audio/093000_60");
        let other = temporary
            .path()
            .join("chronicle/20261024/phone.audio/100000_60");
        fs::create_dir_all(&speech_purged)?;
        fs::create_dir_all(&speech_missing)?;
        fs::create_dir_all(&other)?;
        fs::write(
            speech_purged.join("note-0.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"transcribe\"}}\n",
        )?;
        fs::write(
            speech_purged.join("events.jsonl"),
            "{\"tract\":\"retention\",\"event\":\"original_deleted\",\"ts\":1000,\"name\":\"note-0.flac\",\"class\":\"policy_raw_release\"}\n",
        )?;
        fs::write(
            speech_missing.join("note-0.jsonl"),
            "{\"_solstone_processing\":{\"handler\":\"transcribe\"}}\n",
        )?;
        fs::write(other.join("note-0.jsonl"), "{\"event\":\"accepted\"}\n")?;
        let summary = compute_storage_summary(temporary.path())?;
        assert_eq!(summary.total_segments, 3);
        assert_eq!(summary.segments_purged, 1);
        assert_eq!(summary.segments_missing_unrecorded, 1);
        assert_eq!(summary.segments_with_raw, 0);
        assert_eq!(summary.segments_damaged_zero_byte, 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn recursive_accounting_skips_descendant_symlink_cycles() -> std::io::Result<()> {
        let temporary = TempDir::new()?;
        let segment = temporary.path().join("chronicle/20260810/tmux/090000_300");
        fs::create_dir_all(&segment)?;
        fs::write(segment.join("notes.md"), "abc")?;
        std::os::unix::fs::symlink(&segment, segment.join("cycle"))?;
        let summary = compute_storage_summary(temporary.path())?;
        assert_eq!(summary.derived_bytes, 3);
        assert_eq!(summary.total_segments, 1);
        Ok(())
    }

    #[test]
    fn unavailable_chronicle_is_not_reported_as_empty() -> std::io::Result<()> {
        let temporary = TempDir::new()?;
        assert_eq!(compute_storage_summary(temporary.path())?.total_segments, 0);
        fs::write(temporary.path().join("chronicle"), "not a directory")?;
        assert!(compute_storage_summary(temporary.path()).is_err());
        Ok(())
    }

    #[test]
    fn populated_storage_accounting_distinguishes_purged_missing_acknowledged_and_zero_byte()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let with_raw = temporary.path().join("chronicle/20260810/tmux/090000_300");
        let purged = temporary.path().join("chronicle/20260810/tmux/100000_300");
        let missing = temporary.path().join("chronicle/20260810/tmux/110000_300");
        let acknowledged = temporary.path().join("chronicle/20260810/tmux/120000_300");
        let zero_byte = temporary.path().join("chronicle/20260810/tmux/130000_300");
        let pre_transcribe_missing = temporary.path().join("chronicle/20260810/tmux/140000_300");
        let bare = temporary
            .path()
            .join("chronicle/20260811/screen/090000_120");
        for path in [
            &with_raw,
            &purged,
            &missing,
            &acknowledged,
            &zero_byte,
            &pre_transcribe_missing,
            &bare,
        ] {
            fs::create_dir_all(path)?;
        }
        fs::write(with_raw.join("audio.flac"), vec![0_u8; 4096])?;
        fs::write(with_raw.join("monitor_1_diff.png"), vec![0_u8; 2048])?;
        fs::write(with_raw.join("audio.jsonl"), b"{\"seeded\": true}\n")?;
        fs::write(purged.join("audio.jsonl"), b"{\"seeded\": true}\n")?;
        fs::write(
            purged.join("events.jsonl"),
            b"{\"tract\":\"retention\",\"event\":\"original_deleted\",\"ts\":1000,\"name\":\"audio.flac\",\"class\":\"owner_raw_release\"}\n",
        )?;
        fs::write(missing.join("audio.jsonl"), b"{\"seeded\": true}\n")?;
        fs::write(acknowledged.join("audio.jsonl"), b"{\"seeded\": true}\n")?;
        record_original_missing_acknowledged(&acknowledged, Some("audio.flac"), 2000)
            .map_err(std::io::Error::other)?;
        fs::write(zero_byte.join("audio.flac"), b"")?;
        fs::write(
            pre_transcribe_missing.join("ingest.json"),
            b"{\"schema_version\":1,\"requested_segment\":\"140000_300\",\"files\":{\"audio.flac\":{\"sha256\":\"aaaa\",\"size\":10}}}\n",
        )?;
        fs::write(bare.join("notes.md"), b"seeded\n")?;

        let summary = compute_storage_summary(temporary.path())?;
        assert_eq!(summary.total_segments, 7);
        assert_eq!(summary.segments_with_raw, 1);
        assert_eq!(summary.segments_purged, 1);
        assert_eq!(summary.segments_missing_unrecorded, 3);
        assert_eq!(summary.segments_damaged_zero_byte, 1);
        assert_eq!(summary.raw_media_bytes, 6144);
        assert_eq!(summary.raw_media_human(), "6.0 KiB");
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1024_u64.pow(5)), "1.0 PiB");
        Ok(())
    }

    #[test]
    fn import_without_media_and_trashed_originals_are_not_missing_unrecorded()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let root = temporary.path();
        let import_seg = root.join("chronicle/20261010/import.audio/090000_300");
        fs::create_dir_all(&import_seg)?;
        fs::write(import_seg.join("audio.jsonl"), b"{\"text\":\"hello\"}\n")?;
        let import_dir = root.join("imports/20261010_090000");
        fs::create_dir_all(&import_dir)?;
        fs::write(
            import_dir.join("import.json"),
            b"{\"keep_original_at_import\":false}\n",
        )?;
        fs::write(
            import_dir.join("imported.json"),
            b"{\"source_type\":\"audio\",\"segments\":[{\"day\":\"20261010\",\"stream\":\"import.audio\",\"segment\":\"090000_300\"}]}\n",
        )?;
        // Hidden staging uploads dir must never interfere.
        fs::create_dir_all(root.join("imports/.uploads/abc"))?;
        fs::write(root.join("imports/.uploads/abc/import.json"), b"{bad")?;

        // Segment whose original was moved to recoverable trash.
        let trashed_seg = root.join("chronicle/20261010/field/100000_300");
        fs::create_dir_all(&trashed_seg)?;
        fs::write(trashed_seg.join("audio.jsonl"), b"{\"text\":\"world\"}\n")?;
        fs::write(trashed_seg.join("audio.flac"), b"raw bytes")?;
        crate::original_trash::trash_originals(
            root,
            &crate::Target {
                day: "20261010".to_owned(),
                stream: "field".to_owned(),
                dir: "100000_300".to_owned(),
            },
            vec!["audio.flac".to_owned()],
            solstone_core_removals::Action::OriginalsRelease,
            solstone_core_removals::Surface::Web {
                page: solstone_core_removals::Page::Health,
            },
        )
        .unwrap();

        let summary = compute_storage_summary(root)?;
        assert_eq!(summary.total_segments, 2);
        assert_eq!(summary.segments_purged, 2);
        assert_eq!(summary.segments_missing_unrecorded, 0);
        Ok(())
    }
}
