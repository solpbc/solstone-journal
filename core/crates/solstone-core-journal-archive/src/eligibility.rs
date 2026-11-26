// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared lighter-export eligibility and four-population measurement.
//!
//! Reuses media offload's exact retention eligibility gate
//! (`solstone_core_retention::grouped::inspect_grouped` with
//! `DecisionPurpose::OffloadProposal`) so the `/app/export/` preview,
//! `/app/export/` preparation, and `solstone journal archive export
//! --without-processed-media` never disagree about a single file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Map;
use solstone_core_format::segment::segment_key;
use solstone_core_journal_config::{owner_zone, read_journal_config};
use solstone_core_journal_io::paths::SegmentLayout;
use solstone_core_offload::ledger::catalog_journal;
use solstone_core_retention::content::{
    ClosedHandlerSet, HandlerRegistry, JournalMedia, MediaClassifier,
};
use solstone_core_retention::grouped::{DecisionPurpose, inspect_grouped};
use solstone_core_retention::policy::{Policy, policy_from_journal_config};
use solstone_core_retention::receipt::Target;
use solstone_core_retention::scan::scan_segment_checked;

use crate::{DayWindow, Inventory};

/// Owner-chosen export copy mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportMode {
    /// Complete portable copy of everything currently in the journal.
    #[default]
    Full,
    /// Portable copy omitting only proven-processed audio and video eligible
    /// under offload's grouped retention gate.
    WithoutProcessedMedia,
}

impl ExportMode {
    /// Wire identifier shared by the export API and persisted job metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::WithoutProcessedMedia => "without_processed_media",
        }
    }
}

/// Byte and file counts for one population in the export breakdown.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PopulationBucket {
    pub bytes: u64,
    pub files: u64,
}

/// Backup-only offloaded media counts read from the offload ledger catalog.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OffloadCatalogCounts {
    pub days: u64,
    pub files: u64,
    pub bytes: u64,
    /// True when any offload ledger file or record was unreadable or corrupt.
    pub degraded: bool,
}

/// Four-population accounting for a journal export.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportBreakdown {
    pub full_bytes: u64,
    pub full_files: u64,
    pub light_bytes: u64,
    pub light_files: u64,
    /// Proven-processed media eligible to omit in `WithoutProcessedMedia`.
    pub processed_media_omitted: PopulationBucket,
    /// Owner media still processing, failed, or otherwise held by retention rules.
    pub unprocessed_or_failed_media: PopulationBucket,
    /// Transcripts, notes, photos, non-media imports, kept originals, settings, credentials.
    pub kept_non_media: PopulationBucket,
    /// Older recordings that already live only in backup.
    pub already_offloaded: OffloadCatalogCounts,
    /// True when config or one or more segment scans failed closed (held media instead of omitting).
    pub measurement_degraded: bool,
}

/// Partial progress reported while walking segments during an estimate.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EstimateProgress {
    pub segments_scanned: u64,
    pub segments_total: u64,
    pub files_measured: u64,
    pub files_total: u64,
    pub full_bytes_so_far: u64,
    pub light_bytes_so_far: u64,
    pub processed_media_bytes_so_far: u64,
}

/// Result of evaluating lighter-export eligibility over a frozen inventory.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LighterExportPlan {
    /// Exact slash-separated archive member names omitted in `WithoutProcessedMedia`.
    pub omitted_members: BTreeSet<String>,
    /// Exact four-population accounting for preview, UI, and CLI notices.
    pub breakdown: ExportBreakdown,
}

/// Error returned only when evaluation is cooperatively cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EligibilityError {
    Cancelled,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SegmentId {
    day: String,
    stream: String,
    dir: String,
    layout: SegmentLayout,
}

impl SegmentId {
    fn relative_prefix(&self) -> String {
        match self.layout {
            SegmentLayout::Direct => format!("chronicle/{}/{}", self.day, self.dir),
            SegmentLayout::Named => format!("chronicle/{}/{}/{}", self.day, self.stream, self.dir),
        }
    }
}

/// Parse a chronicle segment member (`chronicle/<day>/<seg>/<file>` or
/// `chronicle/<day>/<stream>/<seg>/<file>`) into its segment identity and leaf name.
fn parse_segment_member(member: &str) -> Option<(SegmentId, &str)> {
    let parts: Vec<&str> = member.split('/').collect();
    match parts.as_slice() {
        ["chronicle", day, dir, leaf]
            if is_eight_digit_day(day)
                && *dir != "health"
                && *dir != "talents"
                && segment_key(dir).is_some()
                && !leaf.is_empty() =>
        {
            Some((
                SegmentId {
                    day: (*day).to_owned(),
                    stream: "_default".to_owned(),
                    dir: (*dir).to_owned(),
                    layout: SegmentLayout::Direct,
                },
                *leaf,
            ))
        }
        ["chronicle", day, stream, dir, leaf]
            if is_eight_digit_day(day)
                && *stream != "health"
                && *stream != "talents"
                && segment_key(dir).is_some()
                && !leaf.is_empty() =>
        {
            Some((
                SegmentId {
                    day: (*day).to_owned(),
                    stream: (*stream).to_owned(),
                    dir: (*dir).to_owned(),
                    layout: SegmentLayout::Named,
                },
                *leaf,
            ))
        }
        _ => None,
    }
}

fn is_eight_digit_day(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit())
}

/// Read the journal's offload catalog counts without missing v2 manifests.
///
/// Any active witness whose exact local member path is still present in
/// `local_members` is excluded from `already_offloaded` because those bytes are
/// still present locally in the journal.
pub fn summarize_offload_catalog(
    journal: &Path,
    local_members: &BTreeSet<&str>,
    day_window: Option<&DayWindow>,
) -> OffloadCatalogCounts {
    let catalog = catalog_journal(journal);
    let mut days = 0u64;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut degraded = catalog.degraded();

    for day in &catalog.days {
        if day_window.is_some_and(|window| !window.contains_day(&day.day)) {
            continue;
        }
        if day.degraded() {
            degraded = true;
        }
        let mut day_has_offloaded = false;
        for seg in &day.segments {
            if seg.degraded() {
                degraded = true;
            }
            for witness in &seg.active {
                let dir_name = witness.segment_dir.as_deref().unwrap_or(&seg.segment);
                let local_path = match witness.layout {
                    Some(SegmentLayout::Direct) => {
                        format!("chronicle/{}/{dir_name}/{}", day.day, witness.file.name)
                    }
                    Some(SegmentLayout::Named) => format!(
                        "chronicle/{}/{}/{dir_name}/{}",
                        day.day, seg.stream, witness.file.name
                    ),
                    None if seg.stream == "_default" => {
                        format!("chronicle/{}/{dir_name}/{}", day.day, witness.file.name)
                    }
                    None => format!(
                        "chronicle/{}/{}/{dir_name}/{}",
                        day.day, seg.stream, witness.file.name
                    ),
                };
                if local_members.contains(local_path.as_str()) {
                    continue;
                }
                day_has_offloaded = true;
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(witness.file.bytes);
            }
        }
        if day_has_offloaded {
            days = days.saturating_add(1);
        }
    }

    OffloadCatalogCounts {
        days,
        files,
        bytes,
        degraded,
    }
}

/// Evaluate offload's grouped retention eligibility over `inventory`.
///
/// Fails closed: if `config/journal.json` is corrupt/unreadable or a segment
/// directory cannot be checked cleanly, `measurement_degraded` is set to `true`
/// and zero files from the unreadable scope are omitted.
pub fn evaluate_lighter_export(
    journal: &Path,
    inventory: &Inventory,
    day_window: Option<&DayWindow>,
    cancel: Option<&AtomicBool>,
    mut on_progress: impl FnMut(EstimateProgress),
) -> Result<LighterExportPlan, EligibilityError> {
    let mut selected_entries = Vec::new();
    let mut local_members = BTreeSet::new();
    let mut full_bytes = 0u64;
    let mut full_files = 0u64;

    for entry in inventory.entries() {
        let name = entry.member_name().as_str();
        if day_window.is_some_and(|window| !window.contains_member(name)) {
            continue;
        }
        selected_entries.push(entry);
        local_members.insert(name);
        full_bytes = full_bytes.saturating_add(entry.size());
        full_files = full_files.saturating_add(1);
    }

    if is_cancelled(cancel) {
        return Err(EligibilityError::Cancelled);
    }

    let already_offloaded = summarize_offload_catalog(journal, &local_members, day_window);

    let (policy, mut measurement_degraded): (Option<Policy>, bool) =
        match read_journal_config(journal) {
            Ok(read) => {
                let map = read.config.unwrap_or_else(Map::new);
                (Some(policy_from_journal_config(&map)), false)
            }
            Err(_) => (None, true),
        };

    // Group chronicle segment files so each segment directory is scanned once.
    let mut segments: BTreeMap<SegmentId, BTreeMap<String, u64>> = BTreeMap::new();
    for entry in &selected_entries {
        if let Some((seg_id, leaf)) = parse_segment_member(entry.member_name().as_str()) {
            segments
                .entry(seg_id)
                .or_default()
                .insert(leaf.to_owned(), entry.size());
        }
    }

    let now = Timestamp::now();
    let today = owner_zone(journal).to_datetime(now).date();
    let segments_total = segments.len() as u64;
    let mut segments_scanned = 0u64;

    let mut omitted_members = BTreeSet::new();
    let mut processed_media_omitted = PopulationBucket::default();
    let mut unprocessed_or_failed_media = PopulationBucket::default();

    // Non-segment files are always kept (transcripts, notes, photos, imports, settings, creds).
    let segment_file_count: u64 = segments.values().map(|files| files.len() as u64).sum();
    let mut files_measured = full_files.saturating_sub(segment_file_count);

    on_progress(EstimateProgress {
        segments_scanned: 0,
        segments_total,
        files_measured,
        files_total: full_files,
        full_bytes_so_far: full_bytes,
        light_bytes_so_far: full_bytes,
        processed_media_bytes_so_far: 0,
    });

    for (seg_id, inv_files) in &segments {
        if is_cancelled(cancel) {
            return Err(EligibilityError::Cancelled);
        }

        let rel_prefix = seg_id.relative_prefix();
        let seg_path = journal.join(&rel_prefix);

        let mut seg_omitted_names = BTreeSet::new();
        let mut seg_held_media_names = BTreeSet::new();

        match (
            &policy,
            scan_segment_checked(&seg_path, &ClosedHandlerSet, &JournalMedia),
        ) {
            (Some(policy), Ok(checked)) => {
                let target = Target {
                    day: seg_id.day.clone(),
                    stream: seg_id.stream.clone(),
                    dir: seg_id.dir.clone(),
                };
                let inspection = inspect_grouped(
                    &checked,
                    &target,
                    seg_id.layout,
                    policy,
                    today,
                    now,
                    DecisionPurpose::OffloadProposal,
                );
                for eligible in inspection.eligible() {
                    if inv_files.get(eligible.name()).copied() == Some(eligible.bytes()) {
                        seg_omitted_names.insert(eligible.name().to_owned());
                    } else {
                        // Size changed between inventory and scan: fail closed for this file.
                        measurement_degraded = true;
                        seg_held_media_names.insert(eligible.name().to_owned());
                    }
                }
                for found in checked.files() {
                    let name = found.name.as_str();
                    if inv_files.contains_key(name)
                        && !seg_omitted_names.contains(name)
                        && ClosedHandlerSet.expected_handler(&found.name).is_some()
                    {
                        seg_held_media_names.insert(name.to_owned());
                    }
                }
            }
            _ => {
                // Config corrupt or segment unreadable: fail closed and keep all owner media.
                measurement_degraded = true;
                for leaf in inv_files.keys() {
                    if let Some(content_name) =
                        solstone_core_retention::content::ContentName::new(leaf)
                        && JournalMedia.is_owner_media(&content_name)
                        && ClosedHandlerSet.expected_handler(&content_name).is_some()
                    {
                        seg_held_media_names.insert(leaf.clone());
                    }
                }
            }
        }

        for name in seg_omitted_names {
            if let Some(&size) = inv_files.get(&name) {
                omitted_members.insert(format!("{rel_prefix}/{name}"));
                processed_media_omitted.bytes = processed_media_omitted.bytes.saturating_add(size);
                processed_media_omitted.files = processed_media_omitted.files.saturating_add(1);
            }
        }
        for name in seg_held_media_names {
            if let Some(&size) = inv_files.get(&name) {
                unprocessed_or_failed_media.bytes =
                    unprocessed_or_failed_media.bytes.saturating_add(size);
                unprocessed_or_failed_media.files =
                    unprocessed_or_failed_media.files.saturating_add(1);
            }
        }

        segments_scanned = segments_scanned.saturating_add(1);
        files_measured = files_measured.saturating_add(inv_files.len() as u64);

        if segments_scanned.is_multiple_of(32) || segments_scanned == segments_total {
            on_progress(EstimateProgress {
                segments_scanned,
                segments_total,
                files_measured,
                files_total: full_files,
                full_bytes_so_far: full_bytes,
                light_bytes_so_far: full_bytes.saturating_sub(processed_media_omitted.bytes),
                processed_media_bytes_so_far: processed_media_omitted.bytes,
            });
        }
    }

    let light_bytes = full_bytes.saturating_sub(processed_media_omitted.bytes);
    let light_files = full_files.saturating_sub(processed_media_omitted.files);
    let kept_non_media = PopulationBucket {
        bytes: light_bytes.saturating_sub(unprocessed_or_failed_media.bytes),
        files: light_files.saturating_sub(unprocessed_or_failed_media.files),
    };

    Ok(LighterExportPlan {
        omitted_members,
        breakdown: ExportBreakdown {
            full_bytes,
            full_files,
            light_bytes,
            light_files,
            processed_media_omitted,
            unprocessed_or_failed_media,
            kept_non_media,
            already_offloaded,
            measurement_degraded,
        },
    })
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "unit tests for lighter-export eligibility"
)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;

    use serde_json::json;
    use solstone_core_retention::archive_catalog::OffloadFile;

    use super::*;
    use crate::ArchiveSource;

    struct TempJournal(PathBuf);

    impl TempJournal {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-export-elig-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp journal");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let full = self.0.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create parents");
            }
            fs::write(full, bytes).expect("write fixture");
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn processed_audio_sidecar(input_size: u64) -> String {
        let header = json!({
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "handler": "transcribe",
                "state": "analyzed",
                "input_size": input_size,
                "attempted_at": "2026-11-26T10:00:00Z"
            }
        });
        let row = json!({
            "start": 0.0,
            "end": 1.2,
            "text": "hello world"
        });
        format!("{header}\n{row}\n")
    }

    fn empty_audio_sidecar(input_size: u64) -> String {
        let header = json!({
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "handler": "transcribe",
                "state": "empty",
                "reason_code": "no_speech",
                "input_size": input_size,
                "attempted_at": "2026-11-26T10:00:00Z"
            }
        });
        format!("{header}\n")
    }

    #[test]
    fn lighter_export_omits_only_proven_processed_media_and_keeps_photos_imports_unprocessed() {
        let journal = TempJournal::new("four-pop");
        journal.write("config/journal.json", br#"{"schema": 1}"#);
        // 1. Proven processed audio segment -> eligible to omit in lighter export.
        journal.write(
            "chronicle/20261125/mic/090000_60/audio.flac",
            &vec![b'A'; 4000],
        );
        journal.write(
            "chronicle/20261125/mic/090000_60/audio.jsonl",
            processed_audio_sidecar(4000).as_bytes(),
        );
        // 2. Imported audio segment that IS processed -> also omitted in lighter export.
        journal.write(
            "chronicle/20261125/import.plaud/100000_60/audio.m4a",
            &vec![b'P'; 3000],
        );
        journal.write(
            "chronicle/20261125/import.plaud/100000_60/audio.jsonl",
            processed_audio_sidecar(3000).as_bytes(),
        );
        // 3. Unprocessed audio segment (no sidecar) -> MUST stay in both modes.
        journal.write(
            "chronicle/20261125/mic/110000_60/audio.flac",
            &vec![b'U'; 2000],
        );
        // 4. Still photo & kept import original -> MUST stay in both modes.
        journal.write(
            "chronicle/20261125/photos/120000_0/photo.jpg",
            &vec![b'J'; 1500],
        );
        journal.write("imports/20261125_120000/original.pdf", &vec![b'D'; 1000]);

        // 5. Already offloaded recording on another day (only in backup).
        solstone_core_offload::ledger::append_offload_event(
            journal.path(),
            "20261120",
            "mic",
            "080000_60",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            &[OffloadFile {
                name: "audio.flac".to_owned(),
                bytes: 50_000,
                sha256: "0".repeat(64),
            }],
            1_763_625_600,
        )
        .expect("append offload event");

        let source = ArchiveSource::open(journal.path()).expect("open source");
        let plan = evaluate_lighter_export(journal.path(), source.inventory(), None, None, |_| {})
            .expect("evaluate");

        assert!(
            plan.omitted_members
                .contains("chronicle/20261125/mic/090000_60/audio.flac")
        );
        assert!(
            plan.omitted_members
                .contains("chronicle/20261125/import.plaud/100000_60/audio.m4a")
        );
        assert_eq!(plan.omitted_members.len(), 2);

        let b = plan.breakdown;
        assert!(!b.measurement_degraded);
        assert_eq!(b.processed_media_omitted.files, 2);
        assert_eq!(b.processed_media_omitted.bytes, 7000);
        assert_eq!(b.unprocessed_or_failed_media.files, 1);
        assert_eq!(b.unprocessed_or_failed_media.bytes, 2000);
        assert_eq!(
            b.full_bytes,
            b.light_bytes + b.processed_media_omitted.bytes
        );
        assert_eq!(
            b.light_bytes,
            b.unprocessed_or_failed_media.bytes + b.kept_non_media.bytes
        );
        assert_eq!(b.already_offloaded.days, 1);
        assert_eq!(b.already_offloaded.files, 1);
        assert_eq!(b.already_offloaded.bytes, 50_000);
        assert!(!b.already_offloaded.degraded);
    }

    #[test]
    fn corrupt_offload_ledger_and_corrupt_config_fail_closed() {
        let journal = TempJournal::new("fail-closed");
        journal.write("config/journal.json", b"{corrupt json");
        journal.write(
            "chronicle/20261125/mic/090000_60/audio.flac",
            &vec![b'A'; 4000],
        );
        journal.write(
            "chronicle/20261125/mic/090000_60/audio.jsonl",
            processed_audio_sidecar(4000).as_bytes(),
        );
        journal.write("health/offload/20261120.jsonl", b"{not valid jsonl\n");

        let source = ArchiveSource::open(journal.path()).expect("open source");
        let plan = evaluate_lighter_export(journal.path(), source.inventory(), None, None, |_| {})
            .expect("evaluate");

        assert!(plan.breakdown.measurement_degraded);
        assert!(plan.breakdown.already_offloaded.degraded);
        assert!(
            plan.omitted_members.is_empty(),
            "corrupt config must fail closed and omit nothing"
        );
        assert_eq!(plan.breakdown.light_bytes, plan.breakdown.full_bytes);
    }

    #[test]
    fn preserve_all_keeps_empty_audio_in_lighter_export() {
        let journal = TempJournal::new("preserve-all");
        journal.write(
            "chronicle/20261125/mic/090000_60/audio.flac",
            &vec![b'E'; 1200],
        );
        journal.write(
            "chronicle/20261125/mic/090000_60/audio.jsonl",
            empty_audio_sidecar(1200).as_bytes(),
        );

        journal.write(
            "config/journal.json",
            br#"{"transcribe":{"preserve_all":true}}"#,
        );
        let source = ArchiveSource::open(journal.path()).expect("open");
        let kept_plan =
            evaluate_lighter_export(journal.path(), source.inventory(), None, None, |_| {})
                .expect("evaluate preserve_all=true");
        assert!(kept_plan.omitted_members.is_empty());

        journal.write(
            "config/journal.json",
            br#"{"transcribe":{"preserve_all":false}}"#,
        );
        let source2 = ArchiveSource::open(journal.path()).expect("open");
        let omit_plan =
            evaluate_lighter_export(journal.path(), source2.inventory(), None, None, |_| {})
                .expect("evaluate preserve_all=false");
        assert!(
            omit_plan
                .omitted_members
                .contains("chronicle/20261125/mic/090000_60/audio.flac")
        );
    }
}
