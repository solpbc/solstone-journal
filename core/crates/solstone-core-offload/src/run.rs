// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Archive raw media, record the pre-mark ledger witness, then mint an offload mark.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, Local, TimeZone, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_backup::{get_backup_config, record_offload_result};
use solstone_core_backup_runtime::{
    BackupServices, check_archive_snapshot_files, run_archive_backup,
};
use solstone_core_journal_io::check_record_identities;
use solstone_core_journal_io::paths::{PathOrDay, day_dirs, iter_segments};
use solstone_core_retention::{
    RawRelease, Target,
    content::{ClosedHandlerSet, JournalMedia},
    eligibility::resolve as resolve_segment_gate,
    marks::load as load_marks,
    scan::scan_segment,
    upsert_offload,
};

use crate::ledger::{OffloadFile, append_offload_event};
use crate::marks::OffloadMarkIndex;
use crate::measurement::measure_raw_media_usage;
use crate::pruning_audit::{PruneAuditWriter, open_prune_audit, write_prune_audit};

pub const OFFLOAD_STALL_BACKUP_NOT_READY: &str = "backup_not_ready";
pub const OFFLOAD_STALL_BACKUP_FAILING: &str = "backup_failing";
pub const OFFLOAD_STALL_VERIFICATION_MISSING: &str = "verification_missing";
pub const OFFLOAD_STALL_VERIFICATION_OVERDUE: &str = "verification_overdue";
pub const OFFLOAD_STALL_VERIFICATION_FAILED: &str = "verification_failed";
pub const OFFLOAD_STALL_LOCKED: &str = "locked";
pub const OFFLOAD_STALL_ARCHIVE_FAILED: &str = "archive_failed";
pub const OFFLOAD_STALL_CONFIRM_FAILED: &str = "confirm_failed";
pub const OFFLOAD_STALL_CONFIRM_TOOL_FAILED: &str = "confirm_tool_failed";
pub const OFFLOAD_STALL_SEGMENT_IDENTITY: &str = "segment_identity";
pub const OFFLOAD_STALL_UNEXPECTED_ERROR: &str = "unexpected_error";
pub const OFFLOAD_STALL_REASONS: [&str; 11] = [
    OFFLOAD_STALL_BACKUP_NOT_READY,
    OFFLOAD_STALL_BACKUP_FAILING,
    OFFLOAD_STALL_VERIFICATION_MISSING,
    OFFLOAD_STALL_VERIFICATION_OVERDUE,
    OFFLOAD_STALL_VERIFICATION_FAILED,
    OFFLOAD_STALL_LOCKED,
    OFFLOAD_STALL_ARCHIVE_FAILED,
    OFFLOAD_STALL_CONFIRM_FAILED,
    OFFLOAD_STALL_CONFIRM_TOOL_FAILED,
    OFFLOAD_STALL_SEGMENT_IDENTITY,
    OFFLOAD_STALL_UNEXPECTED_ERROR,
];
pub const OFFLOAD_OK_BUDGET_ALREADY_SATISFIED: &str = "budget_already_satisfied";
pub const OFFLOAD_OK_BUDGET_SATISFIED: &str = "budget_satisfied";
pub const OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED: &str = "markable_media_exhausted";
pub const VERIFICATION_INTEGRITY_REASONS: [&str; 3] =
    ["integrity_failed", "auth_failed", "repo_missing"];
pub const VERIFICATION_MAX_AGE_SECONDS: i64 = 14 * 86400;
pub const OFFLOAD_MAX_RUNTIME: &str = "7h";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffloadSegmentDetail {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub files: u64,
    pub bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffloadResult {
    pub status: String,
    pub reason: Option<String>,
    pub files_marked: u64,
    pub bytes_marked: u64,
    pub files_already_marked: u64,
    pub bytes_already_marked: u64,
    pub ran_out_of_markable_media: bool,
    pub dry_run: bool,
    /// Extra identity detail when `reason` is `segment_identity`. The reason
    /// token itself stays `segment_identity`.
    pub reason_detail: Option<String>,
    pub details: Vec<OffloadSegmentDetail>,
    /// Best-effort failure while creating or appending the invocation audit oplog.
    pub audit_recording_failure: Option<String>,
    /// Failure while recording the backup ledger's `last_offload` state.
    pub recording_failure: Option<String>,
}

pub fn format_offload_result(result: &OffloadResult) -> String {
    let line = if result.dry_run {
        if result.status == "stalled" {
            format!(
                "backup offload: stalled reason={}{} dry_run=true",
                result
                    .reason
                    .as_deref()
                    .unwrap_or(OFFLOAD_STALL_UNEXPECTED_ERROR),
                result
                    .reason_detail
                    .as_deref()
                    .map(|detail| format!(" detail={detail}"))
                    .unwrap_or_default()
            )
        } else if result.status == "ok" {
            format!(
                "backup offload: ok reason={} dry_run=true",
                result.reason.as_deref().unwrap_or("")
            )
        } else {
            format!("backup offload: {} dry_run=true", result.status)
        }
    } else if result.status == "ok" {
        format!(
            "backup offload: ok reason={} files_marked={} bytes_marked={} files_already_marked={} bytes_already_marked={} bytes_released=0 ran_out_of_markable_media={}",
            result.reason.as_deref().unwrap_or(""),
            result.files_marked,
            result.bytes_marked,
            result.files_already_marked,
            result.bytes_already_marked,
            result.ran_out_of_markable_media
        )
    } else if result.status == "skipped" {
        "backup offload: skipped".to_owned()
    } else {
        format!(
            "backup offload: stalled reason={}{} files_marked={} bytes_marked={} bytes_released=0 ran_out_of_markable_media={}",
            result
                .reason
                .as_deref()
                .unwrap_or(OFFLOAD_STALL_UNEXPECTED_ERROR),
            result
                .reason_detail
                .as_deref()
                .map(|detail| format!(" detail={detail}"))
                .unwrap_or_default(),
            result.files_marked,
            result.bytes_marked,
            result.ran_out_of_markable_media
        )
    };
    let line = match result.recording_failure.as_deref() {
        Some(detail) => format!("{line} recording_failed={detail}"),
        None => line,
    };
    match result.audit_recording_failure.as_deref() {
        Some(detail) => format!("{line} audit_recording_failed={detail}"),
        None => line,
    }
}

fn stall(
    reason: &str,
    dry_run: bool,
    details: Vec<OffloadSegmentDetail>,
    files: u64,
    bytes: u64,
    files_already_marked: u64,
    bytes_already_marked: u64,
) -> OffloadResult {
    OffloadResult {
        status: "stalled".into(),
        reason: Some(reason.into()),
        files_marked: files,
        bytes_marked: bytes,
        files_already_marked,
        bytes_already_marked,
        ran_out_of_markable_media: false,
        dry_run,
        reason_detail: None,
        details,
        audit_recording_failure: None,
        recording_failure: None,
    }
}

impl OffloadResult {
    fn with_reason_detail(mut self, detail: String) -> Self {
        self.reason_detail = Some(detail);
        self
    }

    fn with_audit_recording_failure(mut self, failure: Option<String>) -> Self {
        self.audit_recording_failure = failure;
        self
    }
}
fn prepare(paths: &[PathBuf]) -> Option<Vec<OffloadFile>> {
    paths
        .iter()
        .map(|path| {
            let bytes = path.metadata().ok()?.len();
            let digest = Sha256::digest(fs::read(path).ok()?);
            Some(OffloadFile {
                name: path.file_name()?.to_str()?.to_owned(),
                bytes,
                sha256: format!("{digest:x}"),
            })
        })
        .collect()
}
fn budget_satisfied(start_raw_bytes: u64, freed_bytes: u64, budget_bytes: Option<u64>) -> bool {
    budget_bytes.is_none_or(|budget| start_raw_bytes.saturating_sub(freed_bytes) <= budget)
}
fn local_instant(now_unix: i64) -> Option<DateTime<FixedOffset>> {
    Local
        .timestamp_opt(now_unix, 0)
        .single()
        .map(|time| time.fixed_offset())
}
fn precondition(config: &serde_json::Map<String, Value>, now: i64) -> Option<&'static str> {
    if config.get("enabled") != Some(&Value::Bool(true)) {
        return Some(OFFLOAD_STALL_BACKUP_NOT_READY);
    }
    let backup = config.get("last_backup")?.as_object()?;
    if backup.get("status") != Some(&Value::String("ok".into())) {
        return Some(OFFLOAD_STALL_BACKUP_FAILING);
    }
    if backup
        .get("snapshot_id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Some(OFFLOAD_STALL_BACKUP_NOT_READY);
    }
    let verification = config.get("last_verification")?.as_object()?;
    if verification.get("status") == Some(&Value::String("error".into()))
        && verification
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| VERIFICATION_INTEGRITY_REASONS.contains(&reason))
    {
        return Some(OFFLOAD_STALL_VERIFICATION_FAILED);
    }
    let last_ok = verification.get("last_ok_time").and_then(Value::as_i64)?;
    if now - last_ok > VERIFICATION_MAX_AGE_SECONDS {
        return Some(OFFLOAD_STALL_VERIFICATION_OVERDUE);
    }
    None
}

/// Offload only records pending owner-approved release; it never deletes raw files.
pub fn run_offload(journal: &Path, services: &BackupServices<'_>, dry_run: bool) -> OffloadResult {
    let config = match get_backup_config(journal) {
        Ok(config) => config,
        Err(_) => return stall(OFFLOAD_STALL_UNEXPECTED_ERROR, dry_run, vec![], 0, 0, 0, 0),
    };
    finish_offload(
        journal,
        services,
        run_offload_body(journal, services, dry_run, config),
    )
}

fn finish_offload(
    journal: &Path,
    services: &BackupServices<'_>,
    mut result: OffloadResult,
) -> OffloadResult {
    if result.dry_run {
        return result;
    }
    match record_offload_result(
        journal,
        &result.status,
        serde_json::json!(services.clock.now_unix()),
        result.reason.clone().map_or(Value::Null, Value::String),
        serde_json::json!(result.files_marked),
        serde_json::json!(result.bytes_marked),
        serde_json::json!(result.ran_out_of_markable_media),
    ) {
        Ok(()) => result,
        Err(error) => {
            result.recording_failure = Some(error.to_string());
            result
        }
    }
}

fn run_offload_body(
    journal: &Path,
    services: &BackupServices<'_>,
    dry_run: bool,
    config: serde_json::Map<String, Value>,
) -> OffloadResult {
    if config.get("offload").and_then(|value| value.get("enabled")) != Some(&Value::Bool(true)) {
        return OffloadResult {
            status: "skipped".into(),
            reason: None,
            files_marked: 0,
            bytes_marked: 0,
            files_already_marked: 0,
            bytes_already_marked: 0,
            ran_out_of_markable_media: false,
            dry_run,
            reason_detail: None,
            details: vec![],
            audit_recording_failure: None,
            recording_failure: None,
        };
    }
    if let Some(reason) = precondition(&config, services.clock.now_unix()) {
        return stall(reason, dry_run, vec![], 0, 0, 0, 0);
    }
    let mark_index = match load_marks(journal) {
        Ok(register) => OffloadMarkIndex::from_register(&register),
        Err(_) => return stall(OFFLOAD_STALL_UNEXPECTED_ERROR, dry_run, vec![], 0, 0, 0, 0),
    };
    let files_already_marked = mark_index
        .entries
        .iter()
        .map(|mark| mark.names.len() as u64)
        .sum();
    let bytes_already_marked = mark_index.entries.iter().map(|mark| mark.bytes).sum();
    let start_raw_bytes = measure_raw_media_usage(journal).total_bytes;
    let budget = config
        .get("offload")
        .and_then(|value| value.get("budget_bytes"))
        .and_then(Value::as_u64);
    let mut details = vec![];
    let (mut files_marked, mut bytes_marked) = (0, 0);
    if budget_satisfied(start_raw_bytes, bytes_already_marked, budget) {
        return OffloadResult {
            status: "ok".into(),
            reason: Some(OFFLOAD_OK_BUDGET_ALREADY_SATISFIED.into()),
            files_marked,
            bytes_marked,
            files_already_marked,
            bytes_already_marked,
            ran_out_of_markable_media: false,
            dry_run,
            reason_detail: None,
            details,
            audit_recording_failure: None,
            recording_failure: None,
        };
    }
    let mut days = day_dirs(journal)
        .unwrap_or_default()
        .into_keys()
        .collect::<Vec<_>>();
    days.sort();
    let mut listed = Vec::new();
    for day in &days {
        listed.extend(iter_segments(journal, PathOrDay::Day(day)).unwrap_or_default());
    }
    if let Err(error) = check_record_identities(&listed) {
        return stall(
            OFFLOAD_STALL_SEGMENT_IDENTITY,
            dry_run,
            vec![],
            0,
            0,
            files_already_marked,
            bytes_already_marked,
        )
        .with_reason_detail(error.to_string());
    }
    let (mut audit, mut audit_recording_failure): (Option<PruneAuditWriter>, Option<String>) =
        if dry_run {
            (None, None)
        } else if let Some(opened) = local_instant(services.clock.now_unix()) {
            let audit = open_prune_audit(journal, opened);
            let failure = audit.recording_failure();
            (Some(audit), failure)
        } else {
            (
                None,
                Some("failed to create offload audit oplog: local time unavailable".into()),
            )
        };
    for day in days {
        for segment in iter_segments(journal, PathOrDay::Day(&day)).unwrap_or_default() {
            let identity = match segment.record_identity() {
                Ok(identity) => identity,
                Err(error) => {
                    return stall(
                        OFFLOAD_STALL_SEGMENT_IDENTITY,
                        dry_run,
                        details,
                        files_marked,
                        bytes_marked,
                        files_already_marked,
                        bytes_already_marked,
                    )
                    .with_reason_detail(error.to_string())
                    .with_audit_recording_failure(audit_recording_failure);
                }
            };
            let registry = ClosedHandlerSet;
            let classifier = JournalMedia;
            let found = scan_segment(segment.path(), &registry, &classifier);
            let RawRelease::Releasable(proven) = resolve_segment_gate(
                &registry,
                &classifier,
                &day,
                identity.stream,
                identity.key,
                &found,
            ) else {
                continue;
            };
            let mut paths = proven
                .iter()
                .map(|file| segment.path().join(file.name()))
                .collect::<Vec<_>>();
            paths.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
            if paths.is_empty() {
                continue;
            }
            let Some(files) = prepare(&paths) else {
                return stall(
                    OFFLOAD_STALL_UNEXPECTED_ERROR,
                    dry_run,
                    details,
                    files_marked,
                    bytes_marked,
                    files_already_marked,
                    bytes_already_marked,
                )
                .with_audit_recording_failure(audit_recording_failure);
            };
            let bytes = files.iter().map(|file| file.bytes).sum::<u64>();
            let names = files
                .iter()
                .map(|file| file.name.clone())
                .collect::<Vec<_>>();
            if mark_index
                .matches(&day, identity.stream, identity.key, &names)
                .is_some()
            {
                continue;
            }
            // `floor_bytes` is intentionally ignored: marking preserves bytes on disk; only status/restore use it.
            let effective_marked_bytes = if dry_run {
                details
                    .iter()
                    .map(|detail: &OffloadSegmentDetail| detail.bytes)
                    .sum()
            } else {
                bytes_marked
            };
            if budget_satisfied(
                start_raw_bytes,
                bytes_already_marked.saturating_add(effective_marked_bytes),
                budget,
            ) {
                return OffloadResult {
                    status: "ok".into(),
                    reason: Some(OFFLOAD_OK_BUDGET_SATISFIED.into()),
                    files_marked,
                    bytes_marked,
                    files_already_marked,
                    bytes_already_marked,
                    ran_out_of_markable_media: false,
                    dry_run,
                    reason_detail: None,
                    details,
                    audit_recording_failure,
                    recording_failure: None,
                };
            }
            let detail = OffloadSegmentDetail {
                day: day.clone(),
                stream: identity.stream.to_owned(),
                segment: identity.key.to_owned(),
                files: files.len() as u64,
                bytes,
            };
            details.push(detail);
            if dry_run {
                continue;
            }
            let archive = run_archive_backup(journal, services, &paths);
            let Some(snapshot) = archive.snapshot_id else {
                return stall(
                    if archive.error_reason.as_deref() == Some(OFFLOAD_STALL_LOCKED) {
                        OFFLOAD_STALL_LOCKED
                    } else {
                        OFFLOAD_STALL_ARCHIVE_FAILED
                    },
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                    files_already_marked,
                    bytes_already_marked,
                )
                .with_audit_recording_failure(audit_recording_failure);
            };
            let expected = paths
                .iter()
                .zip(&files)
                .map(|(path, file)| (path.clone(), file.bytes))
                .collect::<BTreeMap<_, _>>();
            let check = check_archive_snapshot_files(journal, services, &snapshot, &expected);
            let confirmation_reason = match check.status.as_str() {
                "skipped" => Some(OFFLOAD_STALL_BACKUP_NOT_READY),
                "error" if check.error_reason.as_deref() == Some(OFFLOAD_STALL_LOCKED) => {
                    Some(OFFLOAD_STALL_LOCKED)
                }
                "error" => Some(OFFLOAD_STALL_CONFIRM_TOOL_FAILED),
                "ok" => match check.verdicts.as_ref() {
                    None => Some(OFFLOAD_STALL_CONFIRM_TOOL_FAILED),
                    Some(verdicts) if verdicts.iter().any(|verdict| !verdict.confirmed) => {
                        Some(OFFLOAD_STALL_CONFIRM_FAILED)
                    }
                    Some(_) => None,
                },
                _ => Some(OFFLOAD_STALL_CONFIRM_TOOL_FAILED),
            };
            if let Some(reason) = confirmation_reason {
                return stall(
                    reason,
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                    files_already_marked,
                    bytes_already_marked,
                )
                .with_audit_recording_failure(audit_recording_failure);
            }
            if append_offload_event(
                journal,
                &day,
                identity.stream,
                identity.key,
                &snapshot,
                &files,
                services.clock.now_unix() as u64,
            )
            .is_err()
            {
                return stall(
                    OFFLOAD_STALL_UNEXPECTED_ERROR,
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                    files_already_marked,
                    bytes_already_marked,
                )
                .with_audit_recording_failure(audit_recording_failure);
            }
            let Some(marked_at) = DateTime::<Utc>::from_timestamp(services.clock.now_unix(), 0)
            else {
                return stall(
                    OFFLOAD_STALL_UNEXPECTED_ERROR,
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                    files_already_marked,
                    bytes_already_marked,
                )
                .with_audit_recording_failure(audit_recording_failure);
            };
            if upsert_offload(
                journal,
                &Target {
                    day: day.clone(),
                    stream: identity.stream.to_owned(),
                    dir: identity.key.to_owned(),
                },
                names,
                bytes,
                format!("restic-snapshot:{snapshot}"),
                marked_at,
            )
            .is_err()
            {
                return stall(
                    OFFLOAD_STALL_UNEXPECTED_ERROR,
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                    files_already_marked,
                    bytes_already_marked,
                )
                .with_audit_recording_failure(audit_recording_failure);
            }
            let message = format!(
                "raw-media offload: archived and marked {} raw media file(s)",
                files.len()
            );
            let record = serde_json::json!({
                "event": "raw_media_offload",
                "kind": "raw_media_offload",
                "outcome": "success",
                "stream": identity.stream,
                "segment": identity.key,
                "bytes_marked": bytes,
                "ts": services.clock.now_unix().saturating_mul(1_000),
            });
            if let Some(audit) = audit.as_mut() {
                let outcome = write_prune_audit(audit, &record, &day, &message);
                if audit_recording_failure.is_none() {
                    audit_recording_failure = outcome.recording_failure;
                }
            }
            files_marked += files.len() as u64;
            bytes_marked += bytes;
        }
    }
    OffloadResult {
        status: "ok".into(),
        reason: Some(OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED.into()),
        files_marked,
        bytes_marked,
        files_already_marked,
        bytes_already_marked,
        ran_out_of_markable_media: true,
        dry_run,
        reason_detail: None,
        details,
        audit_recording_failure,
        recording_failure: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;

    use serde_json::json;
    use solstone_core_backup::{
        Destination, generate_and_store_keys, record_backup_result, record_verification_result,
        set_destination, set_enabled, set_offload,
    };
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::{
        Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };
    use solstone_core_journal_io::{
        JournalRoot, MalformedPolicy,
        operational_log::{OplogFormat, catalog_oplogs},
        readers::read_jsonl_with_report,
    };

    struct Script {
        outputs: RefCell<VecDeque<ToolOutput>>,
        calls: RefCell<Vec<Vec<String>>>,
    }
    impl ToolRunner for Script {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.calls.borrow_mut().push(
                request
                    .argv
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
            );
            Ok(self
                .outputs
                .borrow_mut()
                .pop_front()
                .expect("fixture output"))
        }
    }
    struct Http;
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            unreachable!("BYO offload does not use HTTP")
        }
    }
    struct TestClock {
        now: i64,
    }
    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            self.now
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    struct Maintenance;
    impl JournalMaintenance for Maintenance {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
    }
    fn output(stdout: String) -> ToolOutput {
        ToolOutput {
            returncode: 0,
            stdout: stdout.into_bytes(),
            stderr: vec![],
        }
    }

    fn output_with_code(returncode: i32, stdout: String) -> ToolOutput {
        ToolOutput {
            returncode,
            stdout: stdout.into_bytes(),
            stderr: vec![],
        }
    }

    fn write_eligible_sidecar(raw: &Path) {
        let size = raw.metadata().unwrap().len();
        let header = json!({"_solstone_processing": {
            "schema":"solstone.processing.v1",
            "state":"empty",
            "reason_code":"no_decodable_frames",
            "handler":"describe",
            "attempted_at":"2026-01-01T00:00:00Z",
            "input_size":size
        }});
        fs::write(raw.with_extension("jsonl"), format!("{header}\n")).unwrap();
    }

    fn configure_offload(journal: &Path, now: i64, budget_bytes: Option<u64>) {
        set_destination(
            journal,
            &Destination {
                repository: "s3:bucket/prefix".into(),
                backend: "s3".into(),
                credentials:
                    serde_json::json!({"access_key_id":"ACCESS","secret_access_key":"SECRET"})
                        .as_object()
                        .unwrap()
                        .clone(),
            },
        )
        .unwrap();
        generate_and_store_keys(journal).unwrap();
        set_enabled(journal, true).unwrap();
        record_backup_result(journal, "ok", json!(now), json!("ready"), Value::Null).unwrap();
        record_verification_result(journal, "ok", json!(now), Value::Null, json!("1/52")).unwrap();
        set_offload(
            journal,
            &serde_json::json!({"enabled":true,"budget_bytes":budget_bytes,"floor_bytes":1})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
    }
    fn services<'a>(
        runner: &'a Script,
        http: &'a Http,
        clock: &'a TestClock,
        maintenance: &'a Maintenance,
    ) -> BackupServices<'a> {
        BackupServices {
            runner,
            http,
            clock,
            restic_path: Some(Path::new("/fixture/bin/restic")),
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }

    fn successful_offload(
        floor_bytes: u64,
    ) -> (tempfile::TempDir, Vec<(PathBuf, Vec<u8>)>, OffloadResult) {
        let journal = tempfile::tempdir().unwrap();
        let first = journal
            .path()
            .join("chronicle/20260101/010000_001/raw.webm");
        let second = journal
            .path()
            .join("chronicle/20260102/020000_002/raw.webm");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"0123456").unwrap();
        write_eligible_sidecar(&first);
        write_eligible_sidecar(&second);
        let first_bytes = fs::read(&first).unwrap();
        let second_bytes = fs::read(&second).unwrap();

        configure_offload(journal.path(), 100, Some(1));
        // Marking releases no bytes. A huge floor would suppress work if this
        // run path consulted it, so this fixture pins that it deliberately does not.
        set_offload(
            journal.path(),
            &serde_json::json!({"enabled":true,"budget_bytes":1,"floor_bytes":floor_bytes})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();

        let nodes = json!([
            {"message_type":"snapshot","id":"snapshot"},
            {"message_type":"node","path":first.display().to_string(),"size":3},
            {"message_type":"node","path":second.display().to_string(),"size":7}
        ]);
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output(nodes.to_string()),
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output(nodes.to_string()),
            ])),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );

        (
            journal,
            vec![(first, first_bytes), (second, second_bytes)],
            result,
        )
    }

    fn audit_rows(journal: &Path, day: chrono::NaiveDate) -> Vec<Value> {
        let snapshot = catalog_oplogs(JournalRoot::open(journal).unwrap(), &[day]).unwrap();
        let entries = snapshot
            .entries()
            .iter()
            .filter(|entry| {
                entry.name().source().display_slug() == "offload"
                    && entry.name().run().display_slug() == "raw-media-offload"
                    && entry.name().format() == OplogFormat::Jsonl
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        read_jsonl_with_report::<Value>(
            journal
                .join("chronicle")
                .join(entries[0].day())
                .join("health")
                .join(entries[0].leaf()),
            Vec::new(),
            MalformedPolicy::Raise,
        )
        .unwrap()
        .records
        .into_iter()
        .map(|record| record.value)
        .collect()
    }

    #[test]
    fn floor_is_ignored_when_marking_raw_media() {
        // Marking releases no bytes. A huge floor would suppress work if this
        // run path consulted it, so this fixture pins that it deliberately does not.
        let (_journal, _files, result) = successful_offload(999_999_999_999);

        assert_eq!(result.status, "ok");
        assert_eq!(result.bytes_marked, 10);
        assert_eq!(result.details.len(), 2);
    }

    #[test]
    fn successful_offload_keeps_distinct_raw_media_bytes() {
        let (_journal, files, result) = successful_offload(1);

        assert_eq!(result.status, "ok");
        // Success is pending owner-approved release, never deletion of raw media.
        for (path, expected) in files {
            assert_eq!(fs::read(path).unwrap(), expected);
        }
    }

    #[test]
    fn successful_offload_writes_per_segment_rows_to_one_invocation_oplog() {
        let (journal, _, result) = successful_offload(1);

        assert_eq!(result.audit_recording_failure, None);
        let rows = audit_rows(journal.path(), local_instant(100).unwrap().date_naive());
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| {
            row["event"] == "raw_media_offload"
                && row["kind"] == "raw_media_offload"
                && row["outcome"] == "success"
                && row["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("raw-media offload:"))
        }));
        let mut days = rows
            .iter()
            .map(|row| row["day"].as_str().unwrap())
            .collect::<Vec<_>>();
        days.sort_unstable();
        assert_eq!(days, ["20260101", "20260102"]);
    }

    #[test]
    fn partial_pruning_audit_failure_does_not_change_success() {
        let journal = tempfile::tempdir().unwrap();
        let raw = journal
            .path()
            .join("chronicle/20260103/030000_003/audit.webm");
        fs::create_dir_all(raw.parent().unwrap()).unwrap();
        fs::write(&raw, b"audit").unwrap();
        write_eligible_sidecar(&raw);
        let clock = TestClock { now: 1_767_225_600 };
        configure_offload(journal.path(), clock.now, Some(1));
        let audit_day = local_instant(clock.now)
            .unwrap()
            .date_naive()
            .format("%Y%m%d")
            .to_string();
        let audit_parent = journal.path().join("chronicle").join(&audit_day);
        fs::create_dir_all(&audit_parent).unwrap();
        // A file where the invocation's health directory belongs makes canonical
        // oplog creation fail, but a completed archive and mark must still succeed.
        fs::write(audit_parent.join("health"), b"blocked").unwrap();

        let nodes = json!([
            {"message_type":"snapshot","id":"snapshot"},
            {"message_type":"node","path":raw.display().to_string(),"size":5}
        ]);
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output(nodes.to_string()),
            ])),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.files_marked, 1);
        assert_eq!(result.bytes_marked, 5);
        assert!(
            result
                .audit_recording_failure
                .as_deref()
                .is_some_and(|error| error.starts_with("failed to create offload audit oplog:"))
        );
        assert!(format_offload_result(&result).contains("audit_recording_failed="));
    }

    #[test]
    fn second_run_skips_current_marks_without_rearchiving() {
        let (journal, _, first) = successful_offload(1);
        assert_eq!(first.status, "ok");
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;

        let second = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );

        assert_eq!(second.status, "ok");
        assert_eq!(second.files_marked, 0);
        assert_eq!(second.bytes_marked, 0);
        assert_eq!(second.files_already_marked, 2);
        assert_eq!(second.bytes_already_marked, 10);
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn existing_marks_satisfy_the_budget_before_new_archives() {
        let journal = tempfile::tempdir().unwrap();
        let marked = journal
            .path()
            .join("chronicle/20260101/010000_001/marked.webm");
        let pending = journal
            .path()
            .join("chronicle/20260102/020000_002/pending.webm");
        fs::create_dir_all(marked.parent().unwrap()).unwrap();
        fs::create_dir_all(pending.parent().unwrap()).unwrap();
        fs::write(&marked, b"marked").unwrap();
        fs::write(&pending, b"pending").unwrap();
        write_eligible_sidecar(&marked);
        write_eligible_sidecar(&pending);
        configure_offload(journal.path(), 100, Some(7));
        upsert_offload(
            journal.path(),
            &Target {
                day: "20260101".into(),
                stream: "default".into(),
                dir: "010000_001".into(),
            },
            vec!["marked.webm".into()],
            6,
            "restic-snapshot:existing".into(),
            DateTime::<Utc>::from_timestamp(100, 0).unwrap(),
        )
        .unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;

        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );

        // 13 bytes raw minus the 6-byte current mark is within the 7-byte budget.
        assert_eq!(result.status, "ok");
        assert!(!result.ran_out_of_markable_media);
        assert_eq!(result.bytes_already_marked, 6);
        assert_eq!(result.files_marked, 0);
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn last_offload_records_mid_loop_budget_satisfaction() {
        let journal = tempfile::tempdir().unwrap();
        let first = journal
            .path()
            .join("chronicle/20260101/010000_001/raw.webm");
        let second = journal
            .path()
            .join("chronicle/20260102/020000_002/raw.webm");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"0123456").unwrap();
        write_eligible_sidecar(&first);
        write_eligible_sidecar(&second);
        configure_offload(journal.path(), 100, Some(7));
        let nodes = json!([
            {"message_type":"snapshot","id":"snapshot"},
            {"message_type":"node","path":first.display().to_string(),"size":3}
        ]);
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output(nodes.to_string()),
            ])),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.files_marked, 1);
        assert_eq!(result.bytes_marked, 3);
        assert!(!result.ran_out_of_markable_media);
        let register = load_marks(journal.path()).unwrap();
        assert_eq!(register.marks.len(), 1);
        let mark = register.marks.values().next().unwrap();
        assert_eq!(mark.proposal.names.len(), 1);
        assert_eq!(mark.proposal.bytes, 3);
        let last = get_backup_config(journal.path()).unwrap()["last_offload"].clone();
        assert_eq!(last["status"], "ok");
        assert_eq!(last["files_marked"], 1);
        assert_eq!(last["bytes_marked"], 3);
        assert_eq!(last["reason"], OFFLOAD_OK_BUDGET_SATISFIED);
        assert_eq!(last["ran_out_of_markable_media"], false);
    }

    fn last_offload(journal: &Path) -> Value {
        get_backup_config(journal).unwrap()["last_offload"].clone()
    }

    fn raw_config_bytes(journal: &Path) -> Vec<u8> {
        fs::read(journal.join("config/journal.json")).unwrap()
    }

    fn raw_config(journal: &Path) -> Value {
        serde_json::from_slice(&raw_config_bytes(journal)).unwrap()
    }

    fn raw_last_ok_time(journal: &Path) -> Value {
        raw_config(journal)["backup"]["last_offload"]["last_ok_time"].clone()
    }

    fn marks_path(journal: &Path) -> PathBuf {
        journal.join("health/retention-marks.json")
    }

    fn assert_dry_run_wrote_nothing(journal: &Path, before: &[u8]) {
        assert_eq!(raw_config_bytes(journal), before);
        assert!(!marks_path(journal).exists());
    }

    fn run_offload_dry(journal: &Path) -> OffloadResult {
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        run_offload(
            journal,
            &services(&runner, &http, &clock, &maintenance),
            true,
        )
    }

    fn seed_last_ok(journal: &Path, last_ok: i64) {
        record_offload_result(
            journal,
            "ok",
            json!(last_ok),
            Value::Null,
            json!(0),
            json!(0),
            json!(false),
        )
        .unwrap();
    }

    fn eligible_pair(journal: &Path) -> (PathBuf, PathBuf) {
        let first = journal.join("chronicle/20260101/010000_001/raw.webm");
        let second = journal.join("chronicle/20260102/020000_002/raw.webm");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"0123456").unwrap();
        write_eligible_sidecar(&first);
        write_eligible_sidecar(&second);
        (first, second)
    }

    #[cfg(unix)]
    struct RestoreMode {
        path: PathBuf,
        previous: fs::Permissions,
    }

    #[cfg(unix)]
    impl RestoreMode {
        fn chmod(path: &Path, mode: u32) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let previous = fs::metadata(path).unwrap().permissions();
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            Self {
                path: path.to_path_buf(),
                previous,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.path, self.previous.clone());
        }
    }

    #[test]
    fn last_offload_records_skipped_when_disabled() {
        let journal = tempfile::tempdir().unwrap();
        configure_offload(journal.path(), 100, Some(1));
        set_offload(
            journal.path(),
            &serde_json::json!({"enabled":false,"budget_bytes":1,"floor_bytes":1})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );
        assert_eq!(result.status, "skipped");
        assert_eq!(result.reason, None);
        let last = last_offload(journal.path());
        assert_eq!(last["status"], "skipped");
        assert_eq!(last["time"], 100);
        assert!(last["reason"].is_null());
    }

    #[test]
    fn last_offload_records_budget_already_satisfied_at_entry() {
        let journal = tempfile::tempdir().unwrap();
        eligible_pair(journal.path());
        configure_offload(journal.path(), 100, Some(999_999));
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );
        assert_eq!(result.status, "ok");
        assert_eq!(
            result.reason.as_deref(),
            Some(OFFLOAD_OK_BUDGET_ALREADY_SATISFIED)
        );
        assert!(!result.ran_out_of_markable_media);
        let last = last_offload(journal.path());
        assert_eq!(last["status"], "ok");
        assert_eq!(last["reason"], OFFLOAD_OK_BUDGET_ALREADY_SATISFIED);
        assert_eq!(last["files_marked"], 0);
        assert_eq!(last["ran_out_of_markable_media"], false);
    }

    #[test]
    fn last_offload_records_markable_media_exhausted() {
        let (journal, _, result) = successful_offload(1);
        assert_eq!(result.status, "ok");
        assert_eq!(
            result.reason.as_deref(),
            Some(OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED)
        );
        assert!(result.ran_out_of_markable_media);
        let last = last_offload(journal.path());
        assert_eq!(last["status"], "ok");
        assert_eq!(last["reason"], OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED);
        assert_eq!(last["files_marked"], 2);
        assert_eq!(last["bytes_marked"], 10);
        assert_eq!(last["ran_out_of_markable_media"], true);
    }

    #[test]
    fn last_offload_records_stall_after_a_mark_and_preserves_last_ok_time() {
        let journal = tempfile::tempdir().unwrap();
        let (first, second) = eligible_pair(journal.path());
        configure_offload(journal.path(), 100, Some(1));
        seed_last_ok(journal.path(), 50);
        let prior = raw_last_ok_time(journal.path());
        assert_eq!(prior, 50);
        let first_nodes = json!([
            {"message_type":"snapshot","id":"snapshot"},
            {"message_type":"node","path":first.display().to_string(),"size":3}
        ]);
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output(first_nodes.to_string()),
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output_with_code(1, String::new()),
            ])),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );
        assert_eq!(result.status, "stalled");
        assert_eq!(
            result.reason.as_deref(),
            Some(OFFLOAD_STALL_CONFIRM_TOOL_FAILED)
        );
        assert_eq!(result.files_marked, 1);
        assert_eq!(result.bytes_marked, 3);
        assert_eq!(load_marks(journal.path()).unwrap().marks.len(), 1);
        let last = last_offload(journal.path());
        assert_eq!(last["status"], "stalled");
        assert_eq!(last["reason"], OFFLOAD_STALL_CONFIRM_TOOL_FAILED);
        assert_eq!(last["files_marked"], 1);
        assert_eq!(last["bytes_marked"], 3);
        assert_eq!(last["last_ok_time"], prior);
        assert_eq!(raw_last_ok_time(journal.path()), prior);
        let _ = second;
    }

    #[test]
    fn ok_reason_tokens_are_pairwise_distinct_and_match_ran_out() {
        assert_ne!(
            OFFLOAD_OK_BUDGET_ALREADY_SATISFIED,
            OFFLOAD_OK_BUDGET_SATISFIED
        );
        assert_ne!(
            OFFLOAD_OK_BUDGET_ALREADY_SATISFIED,
            OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED
        );
        assert_ne!(
            OFFLOAD_OK_BUDGET_SATISFIED,
            OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED
        );
        let journal = tempfile::tempdir().unwrap();
        configure_offload(journal.path(), 100, Some(1));
        seed_last_ok(journal.path(), 50);
        let prior = raw_last_ok_time(journal.path());
        record_verification_result(
            journal.path(),
            "ok",
            json!(100 - VERIFICATION_MAX_AGE_SECONDS - 1),
            Value::Null,
            json!("1/52"),
        )
        .unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );
        assert_eq!(result.status, "stalled");
        assert_eq!(
            result.reason.as_deref(),
            Some(OFFLOAD_STALL_VERIFICATION_OVERDUE)
        );
        let last = last_offload(journal.path());
        assert_eq!(last["status"], "stalled");
        assert_eq!(last["reason"], OFFLOAD_STALL_VERIFICATION_OVERDUE);
        assert_eq!(last["last_ok_time"], prior);
        assert_eq!(raw_last_ok_time(journal.path()), prior);
    }

    #[test]
    fn dry_run_does_not_write_last_offload_for_reachable_outcomes() {
        {
            let journal = tempfile::tempdir().unwrap();
            configure_offload(journal.path(), 100, Some(1));
            set_offload(
                journal.path(),
                &serde_json::json!({"enabled":false,"budget_bytes":1,"floor_bytes":1})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
            let before = raw_config_bytes(journal.path());
            let result = run_offload_dry(journal.path());
            assert_eq!(result.status, "skipped");
            assert_eq!(result.reason, None);
            assert_dry_run_wrote_nothing(journal.path(), &before);
        }
        {
            let journal = tempfile::tempdir().unwrap();
            configure_offload(journal.path(), 100, Some(1));
            record_verification_result(
                journal.path(),
                "ok",
                json!(100 - VERIFICATION_MAX_AGE_SECONDS - 1),
                Value::Null,
                json!("1/52"),
            )
            .unwrap();
            let before = raw_config_bytes(journal.path());
            let result = run_offload_dry(journal.path());
            assert_eq!(result.status, "stalled");
            assert_eq!(
                result.reason.as_deref(),
                Some(OFFLOAD_STALL_VERIFICATION_OVERDUE)
            );
            assert_dry_run_wrote_nothing(journal.path(), &before);
        }
        {
            let journal = tempfile::tempdir().unwrap();
            eligible_pair(journal.path());
            configure_offload(journal.path(), 100, Some(999_999));
            let before = raw_config_bytes(journal.path());
            let result = run_offload_dry(journal.path());
            assert_eq!(result.status, "ok");
            assert_eq!(
                result.reason.as_deref(),
                Some(OFFLOAD_OK_BUDGET_ALREADY_SATISFIED)
            );
            assert_eq!(result.files_marked, 0);
            assert!(!result.ran_out_of_markable_media);
            assert_dry_run_wrote_nothing(journal.path(), &before);
        }
        {
            let journal = tempfile::tempdir().unwrap();
            eligible_pair(journal.path());
            configure_offload(journal.path(), 100, Some(7));
            let before = raw_config_bytes(journal.path());
            let result = run_offload_dry(journal.path());
            assert_eq!(result.status, "ok");
            assert_eq!(result.reason.as_deref(), Some(OFFLOAD_OK_BUDGET_SATISFIED));
            assert_eq!(result.files_marked, 0);
            assert_eq!(result.details.len(), 1);
            assert!(!result.ran_out_of_markable_media);
            assert_dry_run_wrote_nothing(journal.path(), &before);
        }
        {
            let journal = tempfile::tempdir().unwrap();
            eligible_pair(journal.path());
            configure_offload(journal.path(), 100, Some(1));
            let before = raw_config_bytes(journal.path());
            let result = run_offload_dry(journal.path());
            assert_eq!(result.status, "ok");
            assert_eq!(
                result.reason.as_deref(),
                Some(OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED)
            );
            assert_eq!(result.files_marked, 0);
            assert_eq!(result.details.len(), 2);
            assert!(result.ran_out_of_markable_media);
            assert_dry_run_wrote_nothing(journal.path(), &before);
        }
    }

    #[cfg(unix)]
    #[test]
    fn config_read_failure_stalls_without_last_offload() {
        let journal = tempfile::tempdir().unwrap();
        configure_offload(journal.path(), 100, Some(1));
        let config_path = journal.path().join("config/journal.json");
        let _restore = RestoreMode::chmod(&config_path, 0o000);
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );
        assert_eq!(result.status, "stalled");
        assert_eq!(
            result.reason.as_deref(),
            Some(OFFLOAD_STALL_UNEXPECTED_ERROR)
        );
        drop(_restore);
        let backup = &raw_config(journal.path())["backup"];
        assert!(backup.get("last_offload").is_none(), "{backup:?}");
    }

    #[cfg(unix)]
    #[test]
    fn recording_failure_after_marks_when_config_dir_is_unwritable() {
        let journal = tempfile::tempdir().unwrap();
        let (first, second) = eligible_pair(journal.path());
        configure_offload(journal.path(), 100, Some(1));
        let first_nodes = json!([
            {"message_type":"snapshot","id":"snapshot"},
            {"message_type":"node","path":first.display().to_string(),"size":3},
            {"message_type":"node","path":second.display().to_string(),"size":7}
        ]);
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output(first_nodes.to_string()),
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                output(first_nodes.to_string()),
            ])),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        let config_dir = journal.path().join("config");
        let before = raw_config_bytes(journal.path());
        let _restore = RestoreMode::chmod(&config_dir, 0o555);
        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );
        assert_eq!(result.status, "ok");
        assert!(result.files_marked > 0);
        assert!(result.recording_failure.is_some());
        assert_eq!(
            result.reason.as_deref(),
            Some(OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED)
        );
        assert!(
            format_offload_result(&result).contains("recording_failed="),
            "{}",
            format_offload_result(&result)
        );
        assert!(marks_path(journal.path()).exists());
        assert_eq!(load_marks(journal.path()).unwrap().marks.len(), 2);
        drop(_restore);
        assert_eq!(raw_config_bytes(journal.path()), before);
    }

    #[test]
    fn offload_mark_uses_rfc3339_from_clock() {
        let (journal, _, result) = successful_offload(1);
        assert_eq!(result.status, "ok");
        let register = load_marks(journal.path()).unwrap();
        for mark in register.marks.values() {
            assert_eq!(mark.marked_at, "1970-01-01T00:01:40Z");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn identity_stall_keeps_reason_token_and_carries_detail() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let journal = tempfile::tempdir().unwrap();
        let raw = journal
            .path()
            .join("chronicle/20260101/010000_001/raw.webm");
        fs::create_dir_all(raw.parent().unwrap()).unwrap();
        fs::write(&raw, b"raw-bytes").unwrap();
        write_eligible_sidecar(&raw);
        let unreadable = journal
            .path()
            .join("chronicle/20260101")
            .join(OsStr::from_bytes(b"s\xff"))
            .join("020000_002");
        fs::create_dir_all(&unreadable).unwrap();
        fs::write(unreadable.join("raw.webm"), b"other").unwrap();
        configure_offload(journal.path(), 100, Some(1));
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;

        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            true,
        );

        assert_eq!(result.status, "stalled");
        assert_eq!(result.reason.as_deref(), Some("segment_identity"));
        let detail = result.reason_detail.expect("identity detail");
        assert!(detail.contains("not UTF-8 representable"), "{detail}");
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn held_segment_is_not_prepared_archived_or_marked() {
        let journal = tempfile::tempdir().unwrap();
        let raw = journal
            .path()
            .join("chronicle/20260103/030000_003/photo.png");
        fs::create_dir_all(raw.parent().unwrap()).unwrap();
        fs::write(&raw, b"owner-image").unwrap();
        configure_offload(journal.path(), 100, Some(1));
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;

        let result = run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        );

        assert_eq!(result.status, "ok");
        assert!(result.details.is_empty());
        assert_eq!(result.files_marked, 0);
        assert!(runner.calls.borrow().is_empty());
        assert!(!journal.path().join("health/retention-marks.json").exists());
    }

    fn confirmation_failure(check: ToolOutput) -> OffloadResult {
        let journal = tempfile::tempdir().unwrap();
        let raw = journal
            .path()
            .join("chronicle/20260104/040000_004/confirm.webm");
        fs::create_dir_all(raw.parent().unwrap()).unwrap();
        fs::write(&raw, b"confirm").unwrap();
        write_eligible_sidecar(&raw);
        configure_offload(journal.path(), 100, Some(1));
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(String::new()),
                output("[{\"message_type\":\"summary\",\"snapshot_id\":\"snapshot\"}]".into()),
                check,
            ])),
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock { now: 100 };
        let maintenance = Maintenance;
        run_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            false,
        )
    }

    #[test]
    fn archive_content_mismatch_is_confirm_failed() {
        let result = confirmation_failure(output(
            serde_json::json!([{"message_type":"snapshot","id":"snapshot"}]).to_string(),
        ));

        assert_eq!(result.status, "stalled");
        assert_eq!(result.reason.as_deref(), Some("confirm_failed"));
    }

    #[test]
    fn archive_check_tool_failure_is_confirm_tool_failed() {
        let result = confirmation_failure(output_with_code(1, String::new()));

        assert_eq!(result.status, "stalled");
        assert_eq!(result.reason.as_deref(), Some("confirm_tool_failed"));
    }
}
