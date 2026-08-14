// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Archive raw media, record the pre-mark ledger witness, then mint an offload mark.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_backup::{get_backup_config, record_offload_result};
use solstone_core_backup_runtime::{
    BackupServices, check_archive_snapshot_files, run_archive_backup,
};
use solstone_core_journal_io::paths::{PathOrDay, day_dirs, iter_segments};
use solstone_core_retention::{Target, upsert_offload};

use crate::ledger::{OffloadFile, append_offload_event};
use crate::pruning_audit::write_prune_audit;

pub const OFFLOAD_STALL_REASONS: [&str; 10] = [
    "backup_not_ready",
    "backup_failing",
    "verification_missing",
    "verification_overdue",
    "verification_failed",
    "locked",
    "archive_failed",
    "confirm_failed",
    "confirm_tool_failed",
    "unexpected_error",
];
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
    pub details: Vec<OffloadSegmentDetail>,
}

pub fn format_offload_result(result: &OffloadResult) -> String {
    if result.dry_run {
        if result.status == "stalled" {
            return format!(
                "backup offload: stalled reason={} dry_run=true",
                result.reason.as_deref().unwrap_or("unexpected_error")
            );
        }
        return format!("backup offload: {} dry_run=true", result.status);
    }
    if result.status == "ok" {
        return format!(
            "backup offload: ok files_marked={} bytes_marked={} files_already_marked={} bytes_already_marked={} bytes_released=0 ran_out_of_markable_media={}",
            result.files_marked,
            result.bytes_marked,
            result.files_already_marked,
            result.bytes_already_marked,
            result.ran_out_of_markable_media
        );
    }
    format!(
        "backup offload: stalled reason={} files_marked={} bytes_marked={} bytes_released=0 ran_out_of_markable_media={}",
        result.reason.as_deref().unwrap_or("unexpected_error"),
        result.files_marked,
        result.bytes_marked,
        result.ran_out_of_markable_media
    )
}

fn stall(
    reason: &str,
    dry_run: bool,
    details: Vec<OffloadSegmentDetail>,
    files: u64,
    bytes: u64,
) -> OffloadResult {
    OffloadResult {
        status: "stalled".into(),
        reason: Some(reason.into()),
        files_marked: files,
        bytes_marked: bytes,
        files_already_marked: 0,
        bytes_already_marked: 0,
        ran_out_of_markable_media: false,
        dry_run,
        details,
    }
}
fn raw_files(segment: &Path) -> Vec<PathBuf> {
    fs::read_dir(segment)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_none_or(|extension| extension != "jsonl")
        })
        .collect()
}
fn prepare(paths: &[PathBuf]) -> Option<Vec<OffloadFile>> {
    paths
        .iter()
        .map(|path| {
            let bytes = path.metadata().ok()?.len();
            let digest = Sha256::digest(fs::read(path).ok()?);
            Some(OffloadFile {
                name: path.file_name()?.to_string_lossy().into_owned(),
                bytes,
                sha256: format!("{digest:x}"),
            })
        })
        .collect()
}
fn precondition(config: &serde_json::Map<String, Value>, now: i64) -> Option<&'static str> {
    if config.get("enabled") != Some(&Value::Bool(true)) {
        return Some("backup_not_ready");
    }
    let backup = config.get("last_backup")?.as_object()?;
    if backup.get("status") != Some(&Value::String("ok".into())) {
        return Some("backup_failing");
    }
    if backup
        .get("snapshot_id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Some("backup_not_ready");
    }
    let verification = config.get("last_verification")?.as_object()?;
    if verification.get("status") == Some(&Value::String("error".into()))
        && verification
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| VERIFICATION_INTEGRITY_REASONS.contains(&reason))
    {
        return Some("verification_failed");
    }
    let last_ok = verification.get("last_ok_time").and_then(Value::as_i64)?;
    if now - last_ok > VERIFICATION_MAX_AGE_SECONDS {
        return Some("verification_overdue");
    }
    None
}

/// Offload only records pending owner-approved release; it never deletes raw files.
pub fn run_offload(journal: &Path, services: &BackupServices<'_>, dry_run: bool) -> OffloadResult {
    let config = match get_backup_config(journal) {
        Ok(config) => config,
        Err(_) => return stall("unexpected_error", dry_run, vec![], 0, 0),
    };
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
            details: vec![],
        };
    }
    if let Some(reason) = precondition(&config, services.clock.now_unix()) {
        return stall(reason, dry_run, vec![], 0, 0);
    }
    let budget = config
        .get("offload")
        .and_then(|value| value.get("budget_bytes"))
        .and_then(Value::as_u64);
    let mut details = vec![];
    let (mut files_marked, mut bytes_marked) = (0, 0);
    let mut days = day_dirs(journal)
        .unwrap_or_default()
        .into_keys()
        .collect::<Vec<_>>();
    days.sort();
    for day in days {
        for segment in iter_segments(journal, PathOrDay::Day(&day)).unwrap_or_default() {
            let paths = raw_files(&segment.path);
            if paths.is_empty() {
                continue;
            };
            let Some(files) = prepare(&paths) else {
                return stall(
                    "unexpected_error",
                    dry_run,
                    details,
                    files_marked,
                    bytes_marked,
                );
            };
            let bytes = files.iter().map(|file| file.bytes).sum::<u64>();
            // `floor_bytes` is intentionally ignored: marking preserves bytes on disk; only status/restore use it.
            if budget.is_some_and(|budget| bytes_marked >= budget) {
                return OffloadResult {
                    status: "ok".into(),
                    reason: None,
                    files_marked,
                    bytes_marked,
                    files_already_marked: 0,
                    bytes_already_marked: 0,
                    ran_out_of_markable_media: false,
                    dry_run,
                    details,
                };
            }
            let detail = OffloadSegmentDetail {
                day: day.clone(),
                stream: segment.stream.clone(),
                segment: segment.key.clone(),
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
                    if archive.error_reason.as_deref() == Some("locked") {
                        "locked"
                    } else {
                        "archive_failed"
                    },
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                );
            };
            let expected = paths
                .iter()
                .zip(&files)
                .map(|(path, file)| (path.clone(), file.bytes))
                .collect::<BTreeMap<_, _>>();
            let check = check_archive_snapshot_files(journal, services, &snapshot, &expected);
            if check.status != "ok"
                || check
                    .verdicts
                    .as_ref()
                    .is_none_or(|verdicts| verdicts.iter().any(|verdict| !verdict.confirmed))
            {
                return stall("confirm_failed", false, details, files_marked, bytes_marked);
            }
            if append_offload_event(
                journal,
                &day,
                &segment.stream,
                &segment.key,
                &snapshot,
                &files,
                services.clock.now_unix() as u64,
            )
            .is_err()
            {
                return stall(
                    "unexpected_error",
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                );
            }
            let names = files.iter().map(|file| file.name.clone()).collect();
            if upsert_offload(
                journal,
                &Target {
                    day: day.clone(),
                    stream: segment.stream.clone(),
                    dir: segment.key.clone(),
                },
                names,
                bytes,
                format!("restic-snapshot:{snapshot}"),
                &services.clock.now_unix().to_string(),
            )
            .is_err()
            {
                return stall(
                    "unexpected_error",
                    false,
                    details,
                    files_marked,
                    bytes_marked,
                );
            }
            let messages = BTreeMap::from([(
                day.clone(),
                format!(
                    "raw-media offload: archived and marked {} raw media file(s)",
                    files.len()
                ),
            )]);
            let record = serde_json::json!({"kind":"raw_media_offload","day":day,"stream":segment.stream,"segment":segment.key,"bytes_marked":bytes});
            let _audit =
                write_prune_audit(journal, "raw_media_offload", &record, &messages, "19700101");
            files_marked += files.len() as u64;
            bytes_marked += bytes;
        }
    }
    let result = OffloadResult {
        status: "ok".into(),
        reason: None,
        files_marked,
        bytes_marked,
        files_already_marked: 0,
        bytes_already_marked: 0,
        ran_out_of_markable_media: true,
        dry_run,
        details,
    };
    if !dry_run {
        let _ = record_offload_result(
            journal,
            "ok",
            serde_json::json!(services.clock.now_unix()),
            Value::Null,
            serde_json::json!(result.files_marked),
            serde_json::json!(result.bytes_marked),
            serde_json::json!(result.ran_out_of_markable_media),
        );
    }
    result
}
