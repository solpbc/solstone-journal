// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded two-tier local journal integrity scanner, on-demand original
//! checksum verifier, and durable integrity receipt (`health/integrity.json`).
//!
//! Every verdict is worst-signal and honest about coverage: an incomplete or
//! partially unreadable pass never reports `ok`. Every finding path is
//! strictly journal-relative with forward slashes; host paths are never
//! persisted in the receipt.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_entity::{RetiredEntities, read_retired_entities};
use solstone_core_facets::{
    RetiredFacets, facet_declaration_entry, list_facet_directories, observe_facet_declaration,
    read_retired_facets,
};
use solstone_core_format::audio_archive::{
    AUDIO_ARCHIVE_PLACEMENT_FILENAME, AudioArchivePlacement,
};
use solstone_core_format::segment::is_date_key;
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace};
use solstone_core_journal_io::disk_space::{
    journal_disk_floor_bytes, journal_disk_is_low, journal_disk_percent_used,
    read_journal_disk_space,
};
use solstone_core_journal_io::durability::{
    ArtifactId, DurableObservation, artifact, observe_json_durable,
};
use solstone_core_journal_io::jsonl_integrity::{inspect_jsonl, operational_heal_recurrence};
use solstone_core_processing_record::{MediaKind, media_kind};
use solstone_core_retention::original_deletion::{
    SEGMENT_MISSING_WILDCARD, acknowledged_missing_originals, recorded_original_deletions,
};
use solstone_core_retention::original_trash::segments_with_originals_in_trash;
use solstone_core_retention::quarantined_ingest_trash::list_quarantined_ingest_for_day;
use solstone_core_retention::summary::{
    import_segments_without_originals, is_granola_only_jsonl, is_non_media_import_stream,
    manifest_expected_raw_names, sidecar_declared_raw_name,
};
use solstone_core_segment::{DEFAULT_STREAM, list_segments};

use crate::index_health::{IndexHealthState, evaluate_index_health};

/// Receipt schema version persisted in the durable integrity receipt.
pub const INTEGRITY_RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Days strictly before this `YYYYMMDD` cutoff whose originals are missing
/// without a recorded deletion are grouped into the calm, non-urgent
/// `older_missing_originals` list rather than firing Tier 1 urgent alarms.
pub const OLDER_MISSING_CUTOFF_DAY: &str = "20260916";

/// Recent days always covered by a routine pass.
pub const ROUTINE_RECENT_DAYS: usize = 30;

/// Additional older days checked per routine pass so weekly runs roll across
/// the full journal within the bounded time budget.
pub const ROUTINE_ROLLING_OLDER_DAYS: usize = 60;

/// Default wall-clock budget for a bounded integrity scan pass (comfortably
/// within the scheduled heartbeat `max_runtime: "10m"`).
pub const DEFAULT_SCAN_BUDGET: Duration = Duration::from_secs(120);

/// Default wall-clock budget for on-demand SHA-256 verification of kept originals.
pub const DEFAULT_VERIFY_BUDGET: Duration = Duration::from_secs(300);

/// Worst-signal local integrity verdict. Unknown or partial coverage never
/// reports `Ok`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityState {
    NotCheckedYet,
    Unavailable,
    Incomplete,
    NeedsAttention,
    Maintenance,
    Ok,
}

impl IntegrityState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotCheckedYet => "not_checked_yet",
            Self::Unavailable => "unavailable",
            Self::Incomplete => "incomplete",
            Self::NeedsAttention => "needs_attention",
            Self::Maintenance => "maintenance",
            Self::Ok => "ok",
        }
    }
}

/// Which population slice a scan pass targeted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityScopeKind {
    Routine,
    Continue,
    Full,
}

/// Options controlling one local integrity scan pass.
#[derive(Clone, Copy, Debug)]
pub struct IntegrityScanOptions {
    pub scope: IntegrityScopeKind,
    pub budget: Duration,
    pub recent_days: usize,
    pub rolling_older_days: usize,
}

impl Default for IntegrityScanOptions {
    fn default() -> Self {
        Self {
            scope: IntegrityScopeKind::Routine,
            budget: DEFAULT_SCAN_BUDGET,
            recent_days: ROUTINE_RECENT_DAYS,
            rolling_older_days: ROUTINE_ROLLING_OLDER_DAYS,
        }
    }
}

/// Options controlling on-demand SHA-256 verification of kept originals.
#[derive(Clone, Copy, Debug)]
pub struct ChecksumVerifyOptions {
    pub budget: Duration,
}

impl Default for ChecksumVerifyOptions {
    fn default() -> Self {
        Self {
            budget: DEFAULT_VERIFY_BUDGET,
        }
    }
}

/// Disk-space snapshot for the journal volume, using the shared 20 GB decimal
/// floor (`min(20_000_000_000, total / 4)`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JournalDiskStatus {
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub floor_bytes: u64,
    pub percent_used: u8,
    pub low: bool,
}

/// One structured integrity finding. Paths are strictly journal-relative.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IntegrityFinding {
    /// `1` for urgent attention items, `2` for maintenance choices.
    pub tier: u8,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_lines: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damaged_lines: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_tail: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interior_damaged: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heal_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent_used: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl IntegrityFinding {
    fn base(tier: u8, kind: &str) -> Self {
        Self {
            tier,
            kind: kind.to_owned(),
            path: None,
            day: None,
            stream: None,
            segment: None,
            file: None,
            files: None,
            stamp: None,
            size_bytes: None,
            valid_lines: None,
            damaged_lines: None,
            truncated_tail: None,
            interior_damaged: None,
            expected_sha256: None,
            actual_sha256: None,
            facet: None,
            heal_count: None,
            logs: None,
            free_bytes: None,
            total_bytes: None,
            floor_bytes: None,
            percent_used: None,
            search_reason: None,
            detail: None,
        }
    }
}

/// One pre-2026-09-16 segment whose original media is missing without a
/// recorded deletion, grouped outside urgent Tier 1 alarms.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OlderMissingOriginal {
    pub path: String,
    pub day: String,
    pub stream: String,
    pub segment: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// Explicit coverage report for on-demand SHA-256 checksum verification of kept
/// original media files.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChecksumVerificationSummary {
    pub verified_at_ms: i64,
    pub checksums_verified: usize,
    pub checksums_total_kept: usize,
    pub checksums_unavailable_older: usize,
    pub mismatches: usize,
    pub incomplete: bool,
}

/// Durable receipt persisted atomically at the registered `IntegrityReceipt`
/// path (`health/integrity.json`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IntegrityReceipt {
    pub schema_version: u32,
    pub state: IntegrityState,
    pub scope_kind: IntegrityScopeKind,
    pub checked_at_ms: Option<i64>,
    pub duration_ms: u64,
    pub days_checked: usize,
    pub days_total: usize,
    pub days_unreadable: usize,
    pub segments_checked: usize,
    pub incomplete: bool,
    #[serde(default)]
    pub unavailable: bool,
    #[serde(default)]
    pub urgent_count: usize,
    #[serde(default)]
    pub maintenance_count: usize,
    #[serde(default)]
    pub rolling_cursor_day: Option<String>,
    #[serde(default)]
    pub full_coverage_complete: bool,
    #[serde(default)]
    pub journal_disk: Option<JournalDiskStatus>,
    #[serde(default)]
    pub findings: Vec<IntegrityFinding>,
    #[serde(default)]
    pub older_missing_originals: Vec<OlderMissingOriginal>,
    #[serde(default)]
    pub checksums: Option<ChecksumVerificationSummary>,
}

impl IntegrityReceipt {
    #[must_use]
    pub fn not_checked_yet(journal_disk: Option<JournalDiskStatus>) -> Self {
        Self {
            schema_version: INTEGRITY_RECEIPT_SCHEMA_VERSION,
            state: IntegrityState::NotCheckedYet,
            scope_kind: IntegrityScopeKind::Routine,
            checked_at_ms: None,
            duration_ms: 0,
            days_checked: 0,
            days_total: 0,
            days_unreadable: 0,
            segments_checked: 0,
            incomplete: false,
            unavailable: false,
            urgent_count: 0,
            maintenance_count: 0,
            rolling_cursor_day: None,
            full_coverage_complete: false,
            journal_disk,
            findings: Vec::new(),
            older_missing_originals: Vec::new(),
            checksums: None,
        }
    }

    #[must_use]
    pub fn unavailable(journal_disk: Option<JournalDiskStatus>) -> Self {
        Self {
            schema_version: INTEGRITY_RECEIPT_SCHEMA_VERSION,
            state: IntegrityState::Unavailable,
            scope_kind: IntegrityScopeKind::Routine,
            checked_at_ms: None,
            duration_ms: 0,
            days_checked: 0,
            days_total: 0,
            days_unreadable: 0,
            segments_checked: 0,
            incomplete: false,
            unavailable: true,
            urgent_count: 0,
            maintenance_count: 0,
            rolling_cursor_day: None,
            full_coverage_complete: false,
            journal_disk,
            findings: Vec::new(),
            older_missing_originals: Vec::new(),
            checksums: None,
        }
    }

    /// Recompute `urgent_count`, `maintenance_count`, and worst-signal `state`
    /// from the receipt's findings and coverage flags.
    pub fn recompute_verdict(&mut self) {
        self.urgent_count = self.findings.iter().filter(|f| f.tier == 1).count();
        self.maintenance_count = self.findings.iter().filter(|f| f.tier == 2).count();
        self.state = if self.unavailable {
            IntegrityState::Unavailable
        } else if self.checked_at_ms.is_none() {
            IntegrityState::NotCheckedYet
        } else if self.urgent_count > 0 {
            IntegrityState::NeedsAttention
        } else if self.incomplete || self.days_unreadable > 0 {
            IntegrityState::Incomplete
        } else if self.maintenance_count > 0 {
            IntegrityState::Maintenance
        } else {
            IntegrityState::Ok
        };
    }
}

/// Compact summary exposed on `GET /api/system/status` for shell badges.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IntegritySummary {
    pub state: IntegrityState,
    pub urgent_count: usize,
    pub incomplete: bool,
    pub unavailable: bool,
    pub disk_low: bool,
    pub checked_at_ms: Option<i64>,
}

/// Canonical path to the durable integrity receipt inside `journal`.
#[must_use]
pub fn integrity_receipt_path(journal: &Path) -> PathBuf {
    journal.join(artifact(ArtifactId::IntegrityReceipt).path)
}

/// Measure live free/total disk space on the journal volume using the shared
/// 20 GB decimal floor (`min(20_000_000_000, total / 4)`).
#[must_use]
pub fn read_journal_disk_status(journal: &Path) -> Option<JournalDiskStatus> {
    let space = read_journal_disk_space(journal).ok()?;
    Some(JournalDiskStatus {
        free_bytes: space.free_bytes,
        total_bytes: space.total_bytes,
        floor_bytes: journal_disk_floor_bytes(space.total_bytes),
        percent_used: journal_disk_percent_used(space).round().clamp(0.0, 100.0) as u8,
        low: journal_disk_is_low(space),
    })
}

/// Read the last persisted integrity receipt without mutating the journal.
/// Returns `NotCheckedYet` when absent and `Unavailable` when unreadable or
/// malformed.
#[must_use]
pub fn read_integrity_receipt(journal: &Path) -> IntegrityReceipt {
    let disk = read_journal_disk_status(journal);
    let path = integrity_receipt_path(journal);
    match observe_json_durable::<IntegrityReceipt>(ArtifactId::IntegrityReceipt, &path) {
        DurableObservation::Present(mut receipt) => {
            receipt.journal_disk = disk.or(receipt.journal_disk);
            receipt.recompute_verdict();
            receipt
        }
        DurableObservation::Absent => IntegrityReceipt::not_checked_yet(disk),
        DurableObservation::Malformed { .. } | DurableObservation::Unreadable { .. } => {
            IntegrityReceipt::unavailable(disk)
        }
    }
}

/// Fast worst-signal integrity summary for `GET /api/system/status`.
#[must_use]
pub fn read_integrity_summary(journal: &Path) -> IntegritySummary {
    let receipt = read_integrity_receipt(journal);
    let disk_low = receipt.journal_disk.as_ref().is_some_and(|disk| disk.low);
    let (state, urgent_count) = if receipt.unavailable {
        (IntegrityState::Unavailable, receipt.urgent_count)
    } else if disk_low && receipt.urgent_count == 0 {
        (IntegrityState::NeedsAttention, 1)
    } else {
        (receipt.state, receipt.urgent_count)
    };
    IntegritySummary {
        state,
        urgent_count,
        incomplete: receipt.incomplete || receipt.days_unreadable > 0,
        unavailable: receipt.unavailable,
        disk_low,
        checked_at_ms: receipt.checked_at_ms,
    }
}

/// Persist `receipt` atomically with `0o600` permissions.
pub fn write_integrity_receipt(journal: &Path, receipt: &IntegrityReceipt) -> io::Result<()> {
    let path = integrity_receipt_path(journal);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(receipt).map_err(io::Error::other)?;
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|error| io::Error::other(error.to_string()))
}

/// Run a bounded two-tier local integrity scan and persist the updated durable
/// receipt to `health/integrity.json`.
pub fn run_integrity_scan(
    journal: &Path,
    options: IntegrityScanOptions,
) -> io::Result<IntegrityReceipt> {
    let started = Instant::now();
    let now_ts = Timestamp::now();
    let now_sys = SystemTime::now();
    let prior = match observe_json_durable::<IntegrityReceipt>(
        ArtifactId::IntegrityReceipt,
        &integrity_receipt_path(journal),
    ) {
        DurableObservation::Present(receipt) => Some(receipt),
        _ => None,
    };

    let mut findings: Vec<IntegrityFinding> = Vec::new();
    let mut older_missing: Vec<OlderMissingOriginal> = Vec::new();

    // 1. Load-bearing core records.
    check_core_records(journal, &mut findings);

    // 2. Journal volume free-space check (shared 20 GB decimal floor).
    let journal_disk = read_journal_disk_status(journal);
    if let Some(disk) = &journal_disk
        && disk.low
    {
        let mut finding = IntegrityFinding::base(1, "low_disk_space");
        finding.free_bytes = Some(disk.free_bytes);
        finding.total_bytes = Some(disk.total_bytes);
        finding.floor_bytes = Some(disk.floor_bytes);
        finding.percent_used = Some(disk.percent_used);
        findings.push(finding);
    }

    // 3. Operational JSONL quiet-heal recurrence (>= 2 heals in 7 days).
    match operational_heal_recurrence(journal, now_sys) {
        Ok(recurrence) if recurrence.recurring => {
            let mut logs: Vec<String> = recurrence
                .heals
                .iter()
                .map(|heal| heal.log.clone())
                .collect();
            logs.sort();
            logs.dedup();
            let mut finding = IntegrityFinding::base(1, "operational_heal_recurrence");
            finding.heal_count = Some(recurrence.heals.len());
            finding.logs = Some(logs);
            findings.push(finding);
        }
        Ok(_) => {}
        Err(error) => {
            let mut finding = IntegrityFinding::base(1, "damaged_core_record");
            finding.path = Some("health".to_owned());
            finding.detail = Some(strip_host_prefix(journal, &error.to_string()));
            findings.push(finding);
        }
    }

    // 4. Search & @edges index health (escalate only actionable Tier 1 states).
    check_search_index_health(journal, now_ts, &mut findings);

    // 5. Undeclared facet folders (Tier 2 maintenance choice) & damaged facet.json (Tier 1).
    check_facet_folders(journal, &mut findings);

    // 6. Re-verify any prior checksum_mismatch findings so trashing or replacing
    // a damaged file immediately clears the alert on re-check.
    if let Some(prior_receipt) = &prior {
        recheck_prior_checksum_mismatches(journal, &prior_receipt.findings, &mut findings);
    }

    // 7. Bounded chronicle walk: recent days + rolling older slice.
    let (all_days, chronicle_unreadable) = list_chronicle_days_descending(journal);
    let days_total = all_days.len();
    let mut days_unreadable = usize::from(chronicle_unreadable);
    let mut days_checked = 0usize;
    let mut segments_checked = 0usize;
    let mut stopped_on_budget = false;

    let (target_days, next_cursor_if_finished, covers_all_days) = select_days_for_pass(
        &all_days,
        options.scope,
        prior
            .as_ref()
            .and_then(|receipt| receipt.rolling_cursor_day.as_deref()),
        options.recent_days,
        options.rolling_older_days,
    );

    let import_no_orig = import_segments_without_originals(journal);
    let trashed_segments = segments_with_originals_in_trash(journal);

    let mut rolling_cursor_day = next_cursor_if_finished;
    let mut checked_day_set = BTreeSet::new();

    for (idx, day) in target_days.iter().enumerate() {
        if idx > 0 && started.elapsed() >= options.budget {
            stopped_on_budget = true;
            rolling_cursor_day = Some(day.clone());
            break;
        }
        let day_ok = inspect_chronicle_day(
            journal,
            day,
            started,
            options.budget,
            &import_no_orig,
            &trashed_segments,
            &mut segments_checked,
            &mut findings,
            &mut older_missing,
            &mut stopped_on_budget,
        );
        if !day_ok {
            days_unreadable += 1;
        }
        if stopped_on_budget {
            rolling_cursor_day = Some(day.clone());
            break;
        }
        days_checked += 1;
        checked_day_set.insert(day.clone());
    }

    // Preserve older_missing_originals & per-day findings for days we didn't
    // re-scan during a partial continue pass (while replacing any days we DID
    // inspect in this pass).
    if options.scope == IntegrityScopeKind::Continue
        && let Some(prior_receipt) = &prior
    {
        for prior_older in &prior_receipt.older_missing_originals {
            if !checked_day_set.contains(&prior_older.day)
                && !older_missing
                    .iter()
                    .any(|current| current.path == prior_older.path)
            {
                older_missing.push(prior_older.clone());
            }
        }
        for prior_finding in &prior_receipt.findings {
            let is_day_scoped = matches!(
                prior_finding.kind.as_str(),
                "damaged_transcript_jsonl"
                    | "zero_byte_original"
                    | "missing_original"
                    | "quarantined_ingest"
            );
            if is_day_scoped
                && let Some(day) = prior_finding.day.as_deref()
                && !checked_day_set.contains(day)
                && !findings.contains(prior_finding)
            {
                findings.push(prior_finding.clone());
            }
        }
    }

    older_missing.sort_by(|a, b| a.day.cmp(&b.day).then_with(|| a.path.cmp(&b.path)));
    sort_findings(&mut findings);

    let incomplete = stopped_on_budget || days_unreadable > 0;
    let full_coverage_complete =
        !incomplete && (covers_all_days || (days_total > 0 && days_checked >= days_total));

    let mut receipt = IntegrityReceipt {
        schema_version: INTEGRITY_RECEIPT_SCHEMA_VERSION,
        state: IntegrityState::Ok,
        scope_kind: options.scope,
        checked_at_ms: Some(now_ts.as_millisecond()),
        duration_ms: started.elapsed().as_millis() as u64,
        days_checked,
        days_total,
        days_unreadable,
        segments_checked,
        incomplete,
        unavailable: false,
        urgent_count: 0,
        maintenance_count: 0,
        rolling_cursor_day: if full_coverage_complete {
            None
        } else {
            rolling_cursor_day
        },
        full_coverage_complete,
        journal_disk,
        findings,
        older_missing_originals: older_missing,
        checksums: prior.and_then(|receipt| receipt.checksums),
    };
    receipt.recompute_verdict();
    write_integrity_receipt(journal, &receipt)?;
    Ok(receipt)
}

/// Verify SHA-256 checksums of kept original media files on disk against saved
/// `ingest.json` and audio-archive / import manifests (strictly excluding
/// active `imports/.uploads/`), persisting updated coverage and any Tier 1
/// `checksum_mismatch` findings into `health/integrity.json`.
pub fn verify_kept_originals(
    journal: &Path,
    options: ChecksumVerifyOptions,
) -> io::Result<IntegrityReceipt> {
    let started = Instant::now();
    let now_ms = Timestamp::now().as_millisecond();

    let mut receipt = match observe_json_durable::<IntegrityReceipt>(
        ArtifactId::IntegrityReceipt,
        &integrity_receipt_path(journal),
    ) {
        DurableObservation::Present(receipt) => receipt,
        _ => run_integrity_scan(journal, IntegrityScanOptions::default())?,
    };

    // Drop previous checksum_mismatch findings; this pass produces fresh truth.
    receipt
        .findings
        .retain(|finding| finding.kind != "checksum_mismatch");

    let import_hashes = load_import_checksum_index(journal);
    let mut summary = ChecksumVerificationSummary {
        verified_at_ms: now_ms,
        checksums_verified: 0,
        checksums_total_kept: 0,
        checksums_unavailable_older: 0,
        mismatches: 0,
        incomplete: false,
    };

    // 1. Verify kept chronicle segment raw media files.
    let (all_days, chronicle_unreadable) = list_chronicle_days_descending(journal);
    if chronicle_unreadable {
        summary.incomplete = true;
    }

    'days: for day in &all_days {
        if started.elapsed() >= options.budget {
            summary.incomplete = true;
            break;
        }
        let segments = match list_segments(journal, day) {
            Ok(segments) => segments,
            Err(_) => {
                summary.incomplete = true;
                continue;
            }
        };
        for seg in segments {
            if started.elapsed() >= options.budget {
                summary.incomplete = true;
                break 'days;
            }
            let seg_dir = seg.path();
            let stream = seg
                .record_identity()
                .map(|id| id.stream.to_owned())
                .unwrap_or_else(|_| DEFAULT_STREAM.to_owned());
            let segment_key = seg.key().to_owned();
            let segment_dir_name = seg.name().to_str().unwrap_or(seg.key()).to_owned();
            let ingest_hashes = read_ingest_sha256_map(seg_dir);

            let entries = match fs::read_dir(seg_dir) {
                Ok(entries) => entries,
                Err(_) => {
                    summary.incomplete = true;
                    continue;
                }
            };
            for entry in entries {
                if started.elapsed() >= options.budget {
                    summary.incomplete = true;
                    break 'days;
                }
                let Ok(entry) = entry else {
                    summary.incomplete = true;
                    continue;
                };
                let Ok(ft) = entry.file_type() else {
                    summary.incomplete = true;
                    continue;
                };
                if !ft.is_file() {
                    continue;
                }
                let file_path = entry.path();
                if !is_raw_media_file(&file_path) {
                    continue;
                }
                let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                summary.checksums_total_kept += 1;

                let expected = ingest_hashes
                    .get(&file_name)
                    .or_else(|| {
                        import_hashes.tile_parts.get(&(
                            day.clone(),
                            segment_key.clone(),
                            file_name.clone(),
                        ))
                    })
                    .or_else(|| {
                        import_hashes.audio_original_parts.get(&(
                            day.clone(),
                            stream.clone(),
                            segment_key.clone(),
                            file_name.clone(),
                        ))
                    });

                let Some(expected_sha) = expected else {
                    summary.checksums_unavailable_older += 1;
                    continue;
                };

                match sha256_file(&file_path, started, options.budget) {
                    Ok(Some(actual_sha)) => {
                        if actual_sha.eq_ignore_ascii_case(expected_sha) {
                            summary.checksums_verified += 1;
                        } else {
                            summary.mismatches += 1;
                            let mut finding = IntegrityFinding::base(1, "checksum_mismatch");
                            finding.path = journal_relative_slash(journal, &file_path);
                            finding.day = Some(day.clone());
                            finding.stream = Some(stream.clone());
                            finding.segment = Some(segment_dir_name.clone());
                            finding.file = Some(file_name);
                            finding.expected_sha256 = Some(expected_sha.to_ascii_lowercase());
                            finding.actual_sha256 = Some(actual_sha);
                            receipt.findings.push(finding);
                        }
                    }
                    Ok(None) => {
                        summary.incomplete = true;
                        break 'days;
                    }
                    Err(_) => {
                        summary.incomplete = true;
                    }
                }
            }
        }
    }

    // 2. Verify preserved whole-member originals under imports/<id>/originals/
    //    (strictly skipping dot-directories such as imports/.uploads/).
    if !summary.incomplete {
        verify_import_preserved_originals(
            journal,
            &import_hashes,
            started,
            options.budget,
            &mut summary,
            &mut receipt.findings,
        );
    }

    sort_findings(&mut receipt.findings);
    receipt.checksums = Some(summary);
    receipt.journal_disk = read_journal_disk_status(journal).or(receipt.journal_disk);
    if receipt.checked_at_ms.is_none() {
        receipt.checked_at_ms = Some(now_ms);
    }
    receipt.recompute_verdict();
    write_integrity_receipt(journal, &receipt)?;
    Ok(receipt)
}

fn check_core_records(journal: &Path, findings: &mut Vec<IntegrityFinding>) {
    let config_rel = artifact(ArtifactId::JournalConfig).path;
    match observe_json_durable::<Value>(ArtifactId::JournalConfig, &journal.join(config_rel)) {
        DurableObservation::Present(_) | DurableObservation::Absent => {}
        DurableObservation::Malformed { source, .. } => {
            let mut finding = IntegrityFinding::base(1, "damaged_core_record");
            finding.path = Some(config_rel.to_owned());
            finding.detail = Some(strip_host_prefix(journal, &source.to_string()));
            findings.push(finding);
        }
        DurableObservation::Unreadable { source, .. } => {
            let mut finding = IntegrityFinding::base(1, "damaged_core_record");
            finding.path = Some(config_rel.to_owned());
            finding.detail = Some(strip_host_prefix(journal, &source.to_string()));
            findings.push(finding);
        }
    }

    match read_retired_entities(journal) {
        RetiredEntities::Absent | RetiredEntities::Loaded(_) => {}
        RetiredEntities::Malformed(detail) | RetiredEntities::Unreadable(detail) => {
            let mut finding = IntegrityFinding::base(1, "damaged_core_record");
            finding.path = Some("entities/retired.json".to_owned());
            finding.detail = Some(strip_host_prefix(journal, &detail));
            findings.push(finding);
        }
    }

    match read_retired_facets(journal) {
        RetiredFacets::Absent | RetiredFacets::Loaded(_) => {}
        RetiredFacets::Malformed(detail) | RetiredFacets::Unreadable(detail) => {
            let mut finding = IntegrityFinding::base(1, "damaged_core_record");
            finding.path = Some("facets/retired.json".to_owned());
            finding.detail = Some(strip_host_prefix(journal, &detail.to_string()));
            findings.push(finding);
        }
    }
}

fn check_search_index_health(journal: &Path, now: Timestamp, findings: &mut Vec<IntegrityFinding>) {
    let health = evaluate_index_health(journal, now);
    if health.state == IndexHealthState::Failing {
        let mut finding = IntegrityFinding::base(1, "search_index_issue");
        finding.search_reason = Some(
            health
                .failure
                .and_then(|f| serde_json::to_value(f).ok())
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "failing".to_owned()),
        );
        findings.push(finding);
    } else if health.check_overdue {
        let mut finding = IntegrityFinding::base(1, "search_index_issue");
        finding.search_reason = Some("check_overdue".to_owned());
        findings.push(finding);
    }
    if health.retained_prune_guarded {
        let mut finding = IntegrityFinding::base(1, "search_index_issue");
        finding.search_reason = Some("retained_prune_guarded".to_owned());
        findings.push(finding);
    }
    if health.edges_failed {
        let mut finding = IntegrityFinding::base(1, "search_index_issue");
        finding.search_reason = Some("edges_failed".to_owned());
        findings.push(finding);
    }
    if solstone_core_indexer_store::recovery::rebuilt_more_than_once_this_week(journal) {
        let mut finding = IntegrityFinding::base(1, "search_index_issue");
        finding.search_reason = Some("repeated_rebuild".to_owned());
        findings.push(finding);
    }
}

fn check_facet_folders(journal: &Path, findings: &mut Vec<IntegrityFinding>) {
    let Ok(dirs) = list_facet_directories(journal) else {
        return;
    };
    for name in dirs {
        if !solstone_core_facets::facet_trash::facet_name(&name) {
            continue;
        }
        let declaration_rel = || {
            solstone_core_journal_io::durability::facet_declaration_path(journal, &name)
                .ok()
                .and_then(|path| journal_relative_slash(journal, &path))
                .unwrap_or_else(|| format!("facets/{name}"))
        };
        match facet_declaration_entry(journal, &name) {
            Ok(None) => {
                // Real folder under facets/<name>/ with no declaration -> Tier 2 choice.
                let mut finding = IntegrityFinding::base(2, "undeclared_facet");
                finding.path = Some(format!("facets/{name}"));
                finding.facet = Some(name);
                findings.push(finding);
            }
            Ok(Some(_)) => {
                if let Err(error) = observe_facet_declaration(journal, &name) {
                    let mut finding = IntegrityFinding::base(1, "damaged_core_record");
                    finding.path = Some(declaration_rel());
                    finding.facet = Some(name);
                    finding.detail = Some(strip_host_prefix(journal, &error.to_string()));
                    findings.push(finding);
                }
            }
            Err(error) => {
                let mut finding = IntegrityFinding::base(1, "damaged_core_record");
                finding.path = Some(declaration_rel());
                finding.facet = Some(name);
                finding.detail = Some(strip_host_prefix(journal, &error.to_string()));
                findings.push(finding);
            }
        }
    }
}

fn recheck_prior_checksum_mismatches(
    journal: &Path,
    prior_findings: &[IntegrityFinding],
    findings: &mut Vec<IntegrityFinding>,
) {
    let started = Instant::now();
    let cap = Duration::from_secs(10);
    for prior in prior_findings
        .iter()
        .filter(|finding| finding.kind == "checksum_mismatch")
    {
        let Some(rel) = prior.path.as_deref() else {
            continue;
        };
        let Ok(full) = solstone_core_journal_io::contained_path(journal, rel) else {
            continue;
        };
        if !fs::symlink_metadata(&full).is_ok_and(|meta| meta.file_type().is_file()) {
            // File was removed or moved to trash: clear the alert.
            continue;
        }
        if let Some(expected) = prior.expected_sha256.as_deref()
            && let Ok(Some(actual)) = sha256_file(&full, started, cap)
        {
            if actual.eq_ignore_ascii_case(expected) {
                continue;
            }
            let mut updated = prior.clone();
            updated.actual_sha256 = Some(actual);
            findings.push(updated);
            continue;
        }
        findings.push(prior.clone());
    }
}

fn list_chronicle_days_descending(journal: &Path) -> (Vec<String>, bool) {
    let chronicle = journal.join("chronicle");
    let entries = match fs::read_dir(&chronicle) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Vec::new(), false),
        Err(_) => return (Vec::new(), true),
    };
    let mut days = Vec::new();
    let mut unreadable = false;
    for entry in entries {
        let Ok(entry) = entry else {
            unreadable = true;
            continue;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_date_key(name) {
            continue;
        }
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => days.push(name.to_owned()),
            Ok(_) => {}
            Err(_) => unreadable = true,
        }
    }
    days.sort_unstable_by(|a, b| b.cmp(a));
    (days, unreadable)
}

fn select_days_for_pass(
    all_days_desc: &[String],
    scope: IntegrityScopeKind,
    prior_cursor: Option<&str>,
    recent_days: usize,
    rolling_older_days: usize,
) -> (Vec<String>, Option<String>, bool) {
    if all_days_desc.is_empty() {
        return (Vec::new(), None, true);
    }
    match scope {
        IntegrityScopeKind::Full => (all_days_desc.to_vec(), None, true),
        IntegrityScopeKind::Continue => {
            if let Some(cursor) = prior_cursor
                && let Some(pos) = all_days_desc.iter().position(|day| day == cursor)
            {
                return (all_days_desc[pos..].to_vec(), None, true);
            }
            (all_days_desc.to_vec(), None, true)
        }
        IntegrityScopeKind::Routine => {
            let recent_count = recent_days.min(all_days_desc.len());
            let mut selected = all_days_desc[..recent_count].to_vec();
            let older = &all_days_desc[recent_count..];
            if older.is_empty() {
                return (selected, None, true);
            }
            let start = prior_cursor
                .and_then(|cursor| older.iter().position(|day| day == cursor))
                .unwrap_or(0);
            let end = (start + rolling_older_days).min(older.len());
            selected.extend_from_slice(&older[start..end]);
            let next_cursor = if end < older.len() {
                Some(older[end].clone())
            } else {
                None
            };
            let covers_all = start == 0 && end == older.len();
            (selected, next_cursor, covers_all)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect_chronicle_day(
    journal: &Path,
    day: &str,
    started: Instant,
    budget: Duration,
    import_no_orig: &BTreeSet<String>,
    trashed_segments: &BTreeSet<String>,
    segments_checked: &mut usize,
    findings: &mut Vec<IntegrityFinding>,
    older_missing: &mut Vec<OlderMissingOriginal>,
    stopped_on_budget: &mut bool,
) -> bool {
    let mut day_readable = true;

    // 1. Check quarantined observer/failed/ files on this day.
    match list_quarantined_ingest_for_day(journal, day) {
        Ok(items) => {
            for item in items {
                let mut finding = IntegrityFinding::base(1, "quarantined_ingest");
                finding.path = Some(item.rel_path);
                finding.day = Some(item.day);
                finding.segment = Some(item.segment);
                finding.stamp = Some(item.stamp);
                finding.file = Some(item.file);
                finding.size_bytes = Some(item.size_bytes);
                findings.push(finding);
            }
        }
        Err(_) => {
            day_readable = false;
        }
    }

    // 2. Walk segments on this day.
    let segments = match list_segments(journal, day) {
        Ok(segments) => segments,
        Err(_) => return false,
    };

    for seg in segments {
        if started.elapsed() >= budget {
            *stopped_on_budget = true;
            return day_readable;
        }
        let seg_dir = seg.path();
        let stream = seg
            .record_identity()
            .map(|id| id.stream.to_owned())
            .unwrap_or_else(|_| DEFAULT_STREAM.to_owned());
        let segment_key = seg.name().to_str().unwrap_or(seg.key()).to_owned();
        let Some(seg_rel) = journal_relative_slash(journal, seg_dir) else {
            continue;
        };

        let entries = match fs::read_dir(seg_dir) {
            Ok(entries) => entries,
            Err(_) => {
                day_readable = false;
                continue;
            }
        };

        *segments_checked += 1;

        let mut present_raw: BTreeMap<String, u64> = BTreeMap::new();
        let mut transcript_files: Vec<PathBuf> = Vec::new();
        let mut sidecar_expected_raw: BTreeSet<String> = BTreeSet::new();
        let mut has_media_sidecar = false;
        let mut has_granola_only = false;

        for entry in entries {
            let Ok(entry) = entry else {
                day_readable = false;
                continue;
            };
            let Ok(ft) = entry.file_type() else {
                day_readable = false;
                continue;
            };
            if !ft.is_file() {
                continue;
            }
            let file_path = entry.path();
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };

            if is_raw_media_file(&file_path) {
                let len = fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                present_raw.insert(file_name.clone(), len);
                if len == 0 {
                    let mut finding = IntegrityFinding::base(1, "zero_byte_original");
                    finding.path = journal_relative_slash(journal, &file_path);
                    finding.day = Some(day.to_owned());
                    finding.stream = Some(stream.clone());
                    finding.segment = Some(segment_key.clone());
                    finding.file = Some(file_name);
                    findings.push(finding);
                }
            } else if is_owner_transcript_jsonl(&file_name) {
                transcript_files.push(file_path.clone());
                if file_name == "audio.jsonl"
                    || file_name == "screen.jsonl"
                    || file_name.starts_with('p')
                {
                    if file_name == "audio.jsonl" && is_granola_only_jsonl(&file_path) {
                        has_granola_only = true;
                    } else {
                        has_media_sidecar = true;
                        if let Some(expected) = sidecar_declared_raw_name(&file_path) {
                            sidecar_expected_raw.insert(expected);
                        }
                    }
                }
            }
        }

        // Inspect owner transcript JSONLs with the byte-safe JSONL inspector.
        for jsonl_path in &transcript_files {
            if started.elapsed() >= budget {
                *stopped_on_budget = true;
                return day_readable;
            }
            match inspect_jsonl(journal, jsonl_path) {
                Ok(inspection) if inspection.is_damaged() => {
                    let truncated_tail = inspection.truncated_tail();
                    let interior_damaged = inspection.interior_damaged();
                    let mut finding = IntegrityFinding::base(1, "damaged_transcript_jsonl");
                    finding.path = Some(inspection.path);
                    finding.day = Some(day.to_owned());
                    finding.stream = Some(stream.clone());
                    finding.segment = Some(segment_key.clone());
                    finding.valid_lines = Some(inspection.valid_lines);
                    finding.damaged_lines = Some(inspection.damaged.len());
                    finding.truncated_tail = Some(truncated_tail);
                    finding.interior_damaged = Some(interior_damaged);
                    findings.push(finding);
                }
                Ok(_) => {}
                Err(_) => {
                    day_readable = false;
                }
            }
        }

        // Original custody check: skip non-media imports, keep_original_at_import=false,
        // Granola-only notes, and segments currently held in trash.
        let non_media_import =
            is_non_media_import_stream(&stream) || import_no_orig.contains(&seg_rel);
        if non_media_import || trashed_segments.contains(&seg_rel) {
            continue;
        }

        let mut expected_raw = manifest_expected_raw_names(seg_dir);
        expected_raw.extend(sidecar_expected_raw);

        // Also inspect placement.json parts for audio-archive segments.
        let placement_path = seg_dir.join(AUDIO_ARCHIVE_PLACEMENT_FILENAME);
        if placement_path.is_file()
            && let Ok(bytes) = fs::read(&placement_path)
            && let Ok(placement) = serde_json::from_slice::<AudioArchivePlacement>(&bytes)
        {
            for part in placement.parts {
                expected_raw.insert(part.stored_name);
            }
        }

        let deleted = recorded_original_deletions(seg_dir);
        let acknowledged = acknowledged_missing_originals(seg_dir);
        let wildcard_ack = acknowledged.contains(SEGMENT_MISSING_WILDCARD);

        let missing_files: Vec<String> = if !expected_raw.is_empty() {
            expected_raw
                .into_iter()
                .filter(|name| {
                    !present_raw.contains_key(name)
                        && !deleted.contains_key(name)
                        && !wildcard_ack
                        && !acknowledged.contains(name)
                })
                .collect()
        } else if present_raw.is_empty()
            && has_media_sidecar
            && !has_granola_only
            && deleted.is_empty()
            && !wildcard_ack
            && acknowledged.is_empty()
        {
            // Media sidecar exists without any raw media or deletion record,
            // and neither manifest nor header named the raw filename.
            vec![]
        } else {
            continue;
        };

        let is_unrecorded_missing = !missing_files.is_empty()
            || (present_raw.is_empty()
                && has_media_sidecar
                && !has_granola_only
                && deleted.is_empty()
                && !wildcard_ack
                && acknowledged.is_empty());

        if is_unrecorded_missing {
            if day < OLDER_MISSING_CUTOFF_DAY {
                older_missing.push(OlderMissingOriginal {
                    path: seg_rel,
                    day: day.to_owned(),
                    stream,
                    segment: segment_key,
                    files: missing_files,
                });
            } else {
                let mut finding = IntegrityFinding::base(1, "missing_original");
                finding.path = Some(seg_rel);
                finding.day = Some(day.to_owned());
                finding.stream = Some(stream);
                finding.segment = Some(segment_key);
                if !missing_files.is_empty() {
                    finding.files = Some(missing_files);
                }
                findings.push(finding);
            }
        }
    }

    day_readable
}

#[derive(Default)]
struct ImportChecksumIndex {
    /// `(day, segment_key, stored_name) -> sha256` from `imports/<id>/tiles/*.intent.json`.
    tile_parts: BTreeMap<(String, String, String), String>,
    /// `(day, stream, segment, stored_name) -> sha256` from `imports/<id>/import.json` `audio_originals_v1`.
    audio_original_parts: BTreeMap<(String, String, String, String), String>,
    /// `(import_id, relative_slash_path) -> sha256` for preserved originals under `imports/<id>/originals/`.
    preserved_originals: BTreeMap<(String, String), String>,
}

fn load_import_checksum_index(journal: &Path) -> ImportChecksumIndex {
    let mut index = ImportChecksumIndex::default();
    let imports_dir = journal.join("imports");
    let Ok(entries) = fs::read_dir(&imports_dir) else {
        return index;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(import_id) = name.to_str() else {
            continue;
        };
        // Strictly skip dot-directories such as active resumable upload staging (.uploads).
        if import_id.starts_with('.') {
            continue;
        }
        let dir = entry.path();
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }

        // 1. Read import.json audio_originals_v1 (segment parts + preserved originals).
        if let Ok(bytes) = fs::read(dir.join("import.json"))
            && let Ok(doc) = serde_json::from_slice::<Value>(&bytes)
            && let Some(parts) = doc
                .pointer("/audio_originals_v1/parts")
                .and_then(Value::as_array)
        {
            for part in parts {
                if let (Some(day), Some(stream), Some(segment), Some(name), Some(sha)) = (
                    part.get("day").and_then(Value::as_str),
                    part.get("stream").and_then(Value::as_str),
                    part.get("segment").and_then(Value::as_str),
                    part.get("name").and_then(Value::as_str),
                    part.get("sha256")
                        .and_then(Value::as_str)
                        .filter(|s| is_valid_sha256(s)),
                ) {
                    index.audio_original_parts.insert(
                        (
                            day.to_owned(),
                            stream.to_owned(),
                            segment.to_owned(),
                            name.to_owned(),
                        ),
                        sha.to_ascii_lowercase(),
                    );
                }
                if let Some(orig) = part.get("original").and_then(Value::as_object)
                    && let (Some(rel), Some(sha)) = (
                        orig.get("relative").and_then(Value::as_str),
                        orig.get("sha256")
                            .and_then(Value::as_str)
                            .filter(|s| is_valid_sha256(s)),
                    )
                {
                    let normalized = rel.replace('\\', "/");
                    index
                        .preserved_originals
                        .insert((import_id.to_owned(), normalized), sha.to_ascii_lowercase());
                }
            }
        }

        // 2. Read imports/<id>/tiles/*.intent.json for audio-archive segment tiles.
        let tiles_dir = dir.join("tiles");
        if let Ok(tiles) = fs::read_dir(&tiles_dir) {
            for tile in tiles.flatten() {
                if !tile
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.ends_with(".intent.json"))
                {
                    continue;
                }
                let Ok(bytes) = fs::read(tile.path()) else {
                    continue;
                };
                let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                let (Some(day), Some(segment_key), Some(parts)) = (
                    doc.get("day").and_then(Value::as_str),
                    doc.get("segment_key").and_then(Value::as_str),
                    doc.get("parts").and_then(Value::as_array),
                ) else {
                    continue;
                };
                for part in parts {
                    if let (Some(stored_name), Some(sha)) = (
                        part.get("stored_name").and_then(Value::as_str),
                        part.get("sha256")
                            .and_then(Value::as_str)
                            .filter(|s| is_valid_sha256(s)),
                    ) {
                        index.tile_parts.insert(
                            (
                                day.to_owned(),
                                segment_key.to_owned(),
                                stored_name.to_owned(),
                            ),
                            sha.to_ascii_lowercase(),
                        );
                    }
                }
            }
        }

        // 3. Read whole-member hashes from read-only audio-archive.sqlite for
        //    files preserved under imports/<id>/originals/.
        let db_path = dir.join("audio-archive.sqlite");
        if db_path.is_file()
            && let Ok(conn) =
                Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            && let Ok(mut stmt) =
                conn.prepare("SELECT sha256, payload FROM members WHERE included = 1")
            && let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
        {
            for row in rows.flatten() {
                let (sha, payload) = row;
                if !is_valid_sha256(&sha) {
                    continue;
                }
                if let Ok(doc) = serde_json::from_str::<Value>(&payload)
                    && let Some(rel) = doc
                        .get("original_relative_name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                {
                    let normalized = rel.replace('\\', "/");
                    index
                        .preserved_originals
                        .entry((import_id.to_owned(), normalized))
                        .or_insert_with(|| sha.to_ascii_lowercase());
                }
            }
        }
    }
    index
}

fn verify_import_preserved_originals(
    journal: &Path,
    import_hashes: &ImportChecksumIndex,
    started: Instant,
    budget: Duration,
    summary: &mut ChecksumVerificationSummary,
    findings: &mut Vec<IntegrityFinding>,
) {
    let imports_dir = journal.join("imports");
    let Ok(entries) = fs::read_dir(&imports_dir) else {
        return;
    };
    let mut import_dirs: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            // Strictly skip dot-directories such as .uploads.
            if name.starts_with('.') {
                return None;
            }
            entry
                .file_type()
                .ok()
                .filter(|ft| ft.is_dir())
                .map(|_| (name, entry.path()))
        })
        .collect();
    import_dirs.sort_by(|a, b| a.0.cmp(&b.0));

    for (import_id, dir) in import_dirs {
        let originals_root = dir.join("originals");
        if !originals_root.is_dir() {
            continue;
        }
        let mut stack = vec![originals_root.clone()];
        while let Some(current) = stack.pop() {
            if started.elapsed() >= budget {
                summary.incomplete = true;
                return;
            }
            let Ok(children) = fs::read_dir(&current) else {
                summary.incomplete = true;
                continue;
            };
            for child in children.flatten() {
                if started.elapsed() >= budget {
                    summary.incomplete = true;
                    return;
                }
                let Ok(ft) = child.file_type() else {
                    summary.incomplete = true;
                    continue;
                };
                let path = child.path();
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !ft.is_file() {
                    continue;
                }
                summary.checksums_total_kept += 1;
                let Some(rel_within_originals) = journal_relative_slash(&originals_root, &path)
                else {
                    summary.checksums_unavailable_older += 1;
                    continue;
                };
                let Some(expected_sha) = import_hashes
                    .preserved_originals
                    .get(&(import_id.clone(), rel_within_originals.clone()))
                else {
                    summary.checksums_unavailable_older += 1;
                    continue;
                };
                match sha256_file(&path, started, budget) {
                    Ok(Some(actual_sha)) => {
                        if actual_sha.eq_ignore_ascii_case(expected_sha) {
                            summary.checksums_verified += 1;
                        } else {
                            summary.mismatches += 1;
                            let mut finding = IntegrityFinding::base(1, "checksum_mismatch");
                            finding.path = journal_relative_slash(journal, &path);
                            finding.file = Some(rel_within_originals);
                            finding.expected_sha256 = Some(expected_sha.to_ascii_lowercase());
                            finding.actual_sha256 = Some(actual_sha);
                            findings.push(finding);
                        }
                    }
                    Ok(None) => {
                        summary.incomplete = true;
                        return;
                    }
                    Err(_) => {
                        summary.incomplete = true;
                    }
                }
            }
        }
    }
}

fn read_ingest_sha256_map(segment_dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(bytes) = fs::read(segment_dir.join("ingest.json")) else {
        return out;
    };
    let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else {
        return out;
    };
    let Some(files) = doc.get("files").and_then(Value::as_object) else {
        return out;
    };
    for (name, meta) in files {
        if let Some(sha) = meta
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|s| is_valid_sha256(s))
        {
            out.insert(name.clone(), sha.to_ascii_lowercase());
        }
    }
    out
}

fn sha256_file(path: &Path, started: Instant, budget: Duration) -> io::Result<Option<String>> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        if started.elapsed() >= budget {
            return Ok(None);
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(format!("{:x}", hasher.finalize())))
}

fn is_valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_raw_media_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let lower = ext.to_ascii_lowercase();
    matches!(
        media_kind(&lower),
        Some(MediaKind::Audio | MediaKind::Video | MediaKind::Image)
    ) || lower == "pdf"
}

fn is_owner_transcript_jsonl(name: &str) -> bool {
    if !name.ends_with(".jsonl") {
        return false;
    }
    name == "audio.jsonl"
        || name == "screen.jsonl"
        || name == "conversation_transcript.jsonl"
        || name == "imported_audio.jsonl"
        || name.starts_with("browser_")
        || name.ends_with("_desktop_activity.jsonl")
        || (name.starts_with('p') && name.len() > 7 && name.as_bytes()[1].is_ascii_digit())
}

fn journal_relative_slash(base: &Path, path: &Path) -> Option<String> {
    let owned_rel = if let Ok(rel) = path.strip_prefix(base) {
        rel.to_path_buf()
    } else if let Ok(canonical_base) = fs::canonicalize(base) {
        if let Ok(rel) = path.strip_prefix(&canonical_base) {
            rel.to_path_buf()
        } else {
            fs::canonicalize(path)
                .ok()?
                .strip_prefix(&canonical_base)
                .ok()?
                .to_path_buf()
        }
    } else {
        return None;
    };
    let mut parts = Vec::new();
    for component in owned_rel.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        parts.push(part.to_str()?);
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn strip_host_prefix(journal: &Path, text: &str) -> String {
    let mut cleaned = text.to_owned();
    let mut prefixes = Vec::new();
    let raw = journal.display().to_string();
    if !raw.is_empty() {
        prefixes.push(raw);
    }
    if let Ok(canonical) = fs::canonicalize(journal) {
        let canonical_str = canonical.display().to_string();
        if !canonical_str.is_empty() && !prefixes.contains(&canonical_str) {
            prefixes.push(canonical_str);
        }
    }
    for prefix in prefixes {
        cleaned = cleaned
            .replace(&format!("{prefix}/"), "")
            .replace(&format!("{prefix}\\"), "")
            .replace(&prefix, "");
    }
    cleaned.trim().to_owned()
}

fn sort_findings(findings: &mut [IntegrityFinding]) {
    findings.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.day.cmp(&b.day))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.file.cmp(&b.file))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_retention::original_deletion::record_original_missing_acknowledged;

    fn seed_basic_journal(root: &Path) {
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            r#"{"setup":{"completed_at":1700000000000},"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
    }

    #[test]
    fn absent_and_corrupt_receipts_report_not_checked_yet_and_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        seed_basic_journal(dir.path());

        let absent = read_integrity_receipt(dir.path());
        assert_eq!(absent.state, IntegrityState::NotCheckedYet);
        assert!(!absent.unavailable);
        assert!(absent.checked_at_ms.is_none());

        let path = integrity_receipt_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{not valid json").unwrap();

        let corrupt = read_integrity_receipt(dir.path());
        assert_eq!(corrupt.state, IntegrityState::Unavailable);
        assert!(corrupt.unavailable);
    }

    #[test]
    fn verdict_is_never_ok_when_stopped_early_or_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        seed_basic_journal(dir.path());
        for day in ["20261120", "20261121", "20261122"] {
            let seg = dir.path().join(format!("chronicle/{day}/100000_60"));
            fs::create_dir_all(&seg).unwrap();
            fs::write(seg.join("audio.ogg"), b"valid-audio").unwrap();
            fs::write(
                seg.join("audio.jsonl"),
                "{\"start\":\"10:00:00\",\"text\":\"ok\"}\n",
            )
            .unwrap();
        }

        // Zero budget forces early stop after day 0 even with 0 findings -> Incomplete, never Ok!
        let incomplete = run_integrity_scan(
            dir.path(),
            IntegrityScanOptions {
                scope: IntegrityScopeKind::Full,
                budget: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(incomplete.state, IntegrityState::Incomplete);
        assert!(incomplete.incomplete);
        assert!(incomplete.rolling_cursor_day.is_some());
        assert!(incomplete.days_checked < incomplete.days_total);

        // Continue finishes the remaining days cleanly -> Ok (unless disk is genuinely low).
        let continued = run_integrity_scan(
            dir.path(),
            IntegrityScanOptions {
                scope: IntegrityScopeKind::Continue,
                budget: Duration::from_secs(30),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!continued.incomplete);
        assert!(continued.full_coverage_complete);
        assert!(continued.rolling_cursor_day.is_none());
        if !continued.journal_disk.as_ref().is_some_and(|d| d.low) {
            assert_eq!(continued.state, IntegrityState::Ok);
        }
    }

    #[test]
    fn detects_tier1_and_tier2_findings_and_groups_pre_cutoff_missing_originals() {
        let dir = tempfile::tempdir().unwrap();
        seed_basic_journal(dir.path());

        // Tier 1: damaged transcript JSONL + 0-byte original on recent day
        let recent = dir.path().join("chronicle/20261120/100000_60");
        fs::create_dir_all(&recent).unwrap();
        fs::write(recent.join("audio.ogg"), b"").unwrap();
        fs::write(
            recent.join("audio.jsonl"),
            "{\"start\":\"10:00:00\",\"text\":\"good\"}\n{\"broken\":\n",
        )
        .unwrap();

        // Tier 1: post-2026-09-16 unrecorded missing original
        let post_missing = dir.path().join("chronicle/20261001/110000_60");
        fs::create_dir_all(&post_missing).unwrap();
        fs::write(
            post_missing.join("audio.jsonl"),
            "{\"start\":\"11:00:00\",\"text\":\"missing raw\"}\n",
        )
        .unwrap();

        // Non-urgent older_missing_originals: pre-2026-09-16 unrecorded missing original
        let pre_missing = dir.path().join("chronicle/20260815/120000_60");
        fs::create_dir_all(&pre_missing).unwrap();
        fs::write(
            pre_missing.join("audio.jsonl"),
            "{\"start\":\"12:00:00\",\"text\":\"old missing\"}\n",
        )
        .unwrap();

        // Tier 1: quarantined observer/failed file
        let failed_dir = dir
            .path()
            .join("chronicle/20261120/observer/failed/130000_60/1700000000000");
        fs::create_dir_all(&failed_dir).unwrap();
        fs::write(failed_dir.join("audio.ogg"), b"stranded-bytes").unwrap();

        // Tier 2: undeclared facet folder
        fs::create_dir_all(dir.path().join("facets/hobby")).unwrap();

        // Active resumable upload under imports/.uploads/ must NEVER trigger findings
        let uploads = dir.path().join("imports/.uploads/up_active");
        fs::create_dir_all(&uploads).unwrap();
        fs::write(uploads.join("partial.ogg"), b"").unwrap();

        let receipt = run_integrity_scan(
            dir.path(),
            IntegrityScanOptions {
                scope: IntegrityScopeKind::Full,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(receipt.state, IntegrityState::NeedsAttention);
        assert!(
            receipt
                .findings
                .iter()
                .any(|f| f.kind == "zero_byte_original" && f.tier == 1)
        );
        assert!(
            receipt
                .findings
                .iter()
                .any(|f| f.kind == "damaged_transcript_jsonl" && f.tier == 1)
        );
        assert!(
            receipt
                .findings
                .iter()
                .any(|f| f.kind == "missing_original" && f.day.as_deref() == Some("20261001"))
        );
        assert!(
            !receipt
                .findings
                .iter()
                .any(|f| f.kind == "missing_original" && f.day.as_deref() == Some("20260815"))
        );
        assert_eq!(receipt.older_missing_originals.len(), 1);
        assert_eq!(receipt.older_missing_originals[0].day, "20260815");
        assert!(
            receipt
                .findings
                .iter()
                .any(|f| f.kind == "quarantined_ingest" && f.tier == 1)
        );
        assert!(receipt.findings.iter().any(|f| f.kind == "undeclared_facet"
            && f.tier == 2
            && f.facet.as_deref() == Some("hobby")));

        // No finding path may ever contain the host temporary directory path.
        let host = dir.path().display().to_string();
        let serialized = serde_json::to_string(&receipt).unwrap();
        assert!(
            !serialized.contains(&host),
            "host path leaked: {serialized}"
        );

        // Acknowledging the post-cutoff missing original clears the active missing_original alert.
        record_original_missing_acknowledged(
            &post_missing,
            None,
            Timestamp::now().as_millisecond(),
        )
        .unwrap();
        let refreshed = run_integrity_scan(
            dir.path(),
            IntegrityScanOptions {
                scope: IntegrityScopeKind::Full,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !refreshed
                .findings
                .iter()
                .any(|f| f.kind == "missing_original" && f.day.as_deref() == Some("20261001"))
        );
    }

    #[test]
    fn checksum_verification_reports_verified_unavailable_and_mismatch_honestly() {
        let dir = tempfile::tempdir().unwrap();
        seed_basic_journal(dir.path());

        let seg1 = dir.path().join("chronicle/20261120/100000_60");
        fs::create_dir_all(&seg1).unwrap();
        let good_bytes = b"good-audio-bytes";
        let good_sha = format!("{:x}", Sha256::digest(good_bytes));
        fs::write(seg1.join("audio.ogg"), good_bytes).unwrap();
        fs::write(
            seg1.join("ingest.json"),
            serde_json::json!({
                "files": { "audio.ogg": { "sha256": good_sha } }
            })
            .to_string(),
        )
        .unwrap();

        let seg2 = dir.path().join("chronicle/20261120/110000_60");
        fs::create_dir_all(&seg2).unwrap();
        fs::write(seg2.join("audio.ogg"), b"corrupted-bytes").unwrap();
        fs::write(
            seg2.join("ingest.json"),
            serde_json::json!({
                "files": { "audio.ogg": { "sha256": "0".repeat(64) } }
            })
            .to_string(),
        )
        .unwrap();

        // Older kept original with no saved checksum -> counted honestly as unavailable_older.
        let seg3 = dir.path().join("chronicle/20260701/120000_60");
        fs::create_dir_all(&seg3).unwrap();
        fs::write(seg3.join("audio.ogg"), b"legacy-audio").unwrap();

        // Active upload under imports/.uploads/ must be ignored completely.
        let uploads = dir.path().join("imports/.uploads/pending");
        fs::create_dir_all(&uploads).unwrap();
        fs::write(uploads.join("audio.ogg"), b"uploading").unwrap();

        let receipt = verify_kept_originals(dir.path(), ChecksumVerifyOptions::default()).unwrap();
        let checksums = receipt.checksums.as_ref().expect("checksums summary");
        assert_eq!(checksums.checksums_total_kept, 3);
        assert_eq!(checksums.checksums_verified, 1);
        assert_eq!(checksums.checksums_unavailable_older, 1);
        assert_eq!(checksums.mismatches, 1);
        assert!(!checksums.incomplete);
        assert_eq!(receipt.state, IntegrityState::NeedsAttention);
        assert!(receipt.findings.iter().any(|f| {
            f.kind == "checksum_mismatch"
                && f.path.as_deref() == Some("chronicle/20261120/110000_60/audio.ogg")
        }));
    }

    #[test]
    fn imported_transcript_jsonl_allowlist_scans_conversation_and_imported_audio_only() {
        assert!(is_owner_transcript_jsonl("conversation_transcript.jsonl"));
        assert!(is_owner_transcript_jsonl("imported_audio.jsonl"));
        assert!(!is_owner_transcript_jsonl("events.jsonl"));
        assert!(!is_owner_transcript_jsonl("photo_01.jsonl"));

        let dir = tempfile::tempdir().unwrap();
        seed_basic_journal(dir.path());
        let seg = dir
            .path()
            .join("chronicle/20261120/import.chatgpt/090000_60");
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("conversation_transcript.jsonl"),
            "{\"text\":\"ok\"}\n{broken\n",
        )
        .unwrap();
        fs::write(
            seg.join("imported_audio.jsonl"),
            "{\"text\":\"ok\"}\n{truncated\n",
        )
        .unwrap();
        fs::write(seg.join("events.jsonl"), "{broken\n").unwrap();
        fs::write(seg.join("photo_01.jsonl"), "{broken\n").unwrap();

        let receipt = run_integrity_scan(
            dir.path(),
            IntegrityScanOptions {
                scope: IntegrityScopeKind::Full,
                ..Default::default()
            },
        )
        .unwrap();
        let damaged_paths: Vec<&str> = receipt
            .findings
            .iter()
            .filter(|f| f.kind == "damaged_transcript_jsonl")
            .filter_map(|f| f.path.as_deref())
            .collect();
        assert!(damaged_paths.contains(
            &"chronicle/20261120/import.chatgpt/090000_60/conversation_transcript.jsonl"
        ));
        assert!(
            damaged_paths
                .contains(&"chronicle/20261120/import.chatgpt/090000_60/imported_audio.jsonl")
        );
        assert!(!damaged_paths.iter().any(|p| p.ends_with("/events.jsonl")));
        assert!(!damaged_paths.iter().any(|p| p.ends_with("/photo_01.jsonl")));
    }
}
