// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Restore offloaded raw media without removing files that predated an attempt.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_backup::{
    assemble_backend_env, get_destination, get_keys, record_restore_result,
};
use solstone_core_backup_runtime::{BackupServices, reason_for_returncode, run_restic};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments, segment_path};
use solstone_core_retention::{Target, resolve_offload};

use crate::ledger::{
    OffloadFile, SegmentOffloadSummary, append_restore_event, summarize_day, summarize_journal,
};
use crate::measurement::device_free_bytes;

pub const RESTORE_RESERVE_BYTES: u64 = 1_000_000_000;
pub const OFFLOAD_RESTORE_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
pub const OFFLOAD_RESTORE_STATUSES: [&str; 5] = ["ok", "no_op", "refused", "degraded", "error"];
pub const OFFLOAD_RESTORE_REASONS: [&str; 12] = [
    "auth_failed",
    "backup_not_ready",
    "failed",
    "insufficient_free_space",
    "ledger_degraded",
    "locked",
    "missing_file_after_restore",
    "nothing_to_restore",
    "repo_missing",
    "restic_unavailable",
    "timeout",
    "verification_failed",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreSegmentResult {
    pub status: String,
    pub reason: Option<String>,
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub snapshot_id: Option<String>,
    pub files_expected: u64,
    pub files_restored: u64,
    pub bytes_expected: u64,
    pub bytes_restored: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreResult {
    pub status: String,
    pub reason: Option<String>,
    pub scope: String,
    pub day: Option<String>,
    pub segments_selected: u64,
    pub segments_restored: u64,
    pub files_expected: u64,
    pub files_restored: u64,
    pub bytes_expected: u64,
    pub bytes_restored: u64,
    pub details: Vec<RestoreSegmentResult>,
}
fn base(status: &str, reason: Option<&str>, scope: &str, day: Option<&str>) -> RestoreResult {
    RestoreResult {
        status: status.into(),
        reason: reason.map(str::to_owned),
        scope: scope.into(),
        day: day.map(str::to_owned),
        segments_selected: 0,
        segments_restored: 0,
        files_expected: 0,
        files_restored: 0,
        bytes_expected: 0,
        bytes_restored: 0,
        details: vec![],
    }
}
fn sha(path: &Path) -> Option<String> {
    Some(format!("{:x}", Sha256::digest(fs::read(path).ok()?)))
}
fn verify(directory: &Path, files: &[OffloadFile]) -> Option<&'static str> {
    for file in files {
        let path = directory.join(&file.name);
        if !path.is_file() {
            return Some("missing_file_after_restore");
        }
        if path.metadata().ok()?.len() != file.bytes || sha(&path).as_deref() != Some(&file.sha256)
        {
            return Some("verification_failed");
        }
    }
    None
}
fn rollback(directory: &Path, absent: &[OffloadFile]) {
    for file in absent {
        let _ = fs::remove_file(directory.join(&file.name));
    }
}
fn directory(journal: &Path, summary: &SegmentOffloadSummary) -> PathBuf {
    iter_segments(journal, PathOrDay::Day(&summary.day))
        .ok()
        .and_then(|segments| {
            segments
                .into_iter()
                .find(|segment| segment.stream == summary.stream && segment.key == summary.segment)
                .map(|segment| segment.path)
        })
        .unwrap_or_else(|| {
            segment_path(
                journal,
                &summary.day,
                &summary.segment,
                &summary.stream,
                false,
            )
            .unwrap_or_else(|_| PathBuf::new())
        })
}
fn restore_segment(
    journal: &Path,
    services: &BackupServices<'_>,
    summary: &SegmentOffloadSummary,
) -> RestoreSegmentResult {
    let directory = directory(journal, summary);
    let err = |reason: &str| RestoreSegmentResult {
        status: "error".into(),
        reason: Some(reason.into()),
        day: summary.day.clone(),
        stream: summary.stream.clone(),
        segment: summary.segment.clone(),
        snapshot_id: summary.snapshot_id.clone(),
        files_expected: summary.offloaded_file_count,
        files_restored: 0,
        bytes_expected: summary.offloaded_bytes,
        bytes_restored: 0,
    };
    if !directory.is_dir() {
        return err("failed");
    }
    let absent = summary
        .files
        .iter()
        .filter(|file| !directory.join(&file.name).is_file())
        .cloned()
        .collect::<Vec<_>>();
    if !absent.is_empty() {
        let (Some(destination), Some(keys)) = (
            get_destination(journal).ok().flatten(),
            get_keys(journal).ok().flatten(),
        ) else {
            return err("backup_not_ready");
        };
        let Ok(env) = assemble_backend_env(&destination) else {
            return err("failed");
        };
        let env = env
            .into_iter()
            .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
            .collect::<BTreeMap<_, _>>();
        let mut args = vec![
            "restore".into(),
            format!(
                "{}:{}",
                summary.snapshot_id.as_deref().unwrap_or_default(),
                directory.display()
            ),
            "--target".into(),
            directory.display().to_string(),
        ];
        for file in &absent {
            args.extend(["--include".into(), format!("/{}", file.name)])
        }
        let output = match run_restic(
            services.runner,
            &args,
            &destination.repository,
            &keys.daily_key,
            services.restic_path,
            Some(&env),
            true,
            None,
            Some(Duration::from_secs(OFFLOAD_RESTORE_TIMEOUT_SECONDS)),
            &[],
        ) {
            Ok(output) => output,
            Err(_) => return err("failed"),
        };
        if output.returncode != 0 {
            rollback(&directory, &absent);
            return err(reason_for_returncode(output.returncode));
        }
    }
    if let Some(reason) = verify(&directory, &summary.files) {
        rollback(&directory, &absent);
        return err(reason);
    }
    let names = summary
        .files
        .iter()
        .map(|file| file.name.clone())
        .collect::<Vec<_>>();
    if resolve_offload(
        journal,
        &Target {
            day: summary.day.clone(),
            stream: summary.stream.clone(),
            dir: summary.segment.clone(),
        },
        &names,
    )
    .is_err()
    {
        return err("failed");
    };
    if append_restore_event(
        journal,
        &summary.day,
        &summary.stream,
        &summary.segment,
        services.clock.now_unix() as u64,
    )
    .is_err()
    {
        return err("failed");
    };
    RestoreSegmentResult {
        status: "ok".into(),
        reason: None,
        day: summary.day.clone(),
        stream: summary.stream.clone(),
        segment: summary.segment.clone(),
        snapshot_id: summary.snapshot_id.clone(),
        files_expected: summary.offloaded_file_count,
        files_restored: summary.offloaded_file_count,
        bytes_expected: summary.offloaded_bytes,
        bytes_restored: summary.offloaded_bytes,
    }
}
fn run(
    journal: &Path,
    services: &BackupServices<'_>,
    scope: &str,
    day: Option<&str>,
    segments: Vec<SegmentOffloadSummary>,
) -> RestoreResult {
    let selected = segments
        .into_iter()
        .filter(|segment| segment.currently_offloaded)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return base("no_op", Some("nothing_to_restore"), scope, day);
    }
    let expected = selected
        .iter()
        .map(|segment| segment.offloaded_bytes)
        .sum::<u64>();
    if device_free_bytes(journal).map_or(true, |free| free < expected + RESTORE_RESERVE_BYTES) {
        let mut result = base("refused", Some("insufficient_free_space"), scope, day);
        result.segments_selected = selected.len() as u64;
        result.bytes_expected = expected;
        return result;
    }
    let mut details = vec![];
    for segment in &selected {
        let detail = restore_segment(journal, services, segment);
        let hard = detail.status == "error"
            && !matches!(
                detail.reason.as_deref(),
                Some("missing_file_after_restore" | "verification_failed")
            );
        details.push(detail);
        if hard {
            break;
        }
    }
    let failures = details
        .iter()
        .filter(|detail| detail.status == "error")
        .count();
    let success = details
        .iter()
        .filter(|detail| detail.status == "ok")
        .count();
    let status = if failures == 0 {
        "ok"
    } else if success == 0
        || details.iter().any(|detail| {
            !matches!(
                detail.reason.as_deref(),
                Some("missing_file_after_restore" | "verification_failed")
            )
        })
    {
        "error"
    } else {
        "degraded"
    };
    let reason = details.iter().find_map(|detail| detail.reason.clone());
    let result = RestoreResult {
        status: status.into(),
        reason,
        scope: scope.into(),
        day: day.map(str::to_owned),
        segments_selected: selected.len() as u64,
        segments_restored: success as u64,
        files_expected: selected
            .iter()
            .map(|segment| segment.offloaded_file_count)
            .sum(),
        files_restored: details
            .iter()
            .filter(|detail| detail.status == "ok")
            .map(|detail| detail.files_restored)
            .sum(),
        bytes_expected: expected,
        bytes_restored: details
            .iter()
            .filter(|detail| detail.status == "ok")
            .map(|detail| detail.bytes_restored)
            .sum(),
        details,
    };
    let _ = record_restore_result(
        journal,
        &result.status,
        serde_json::json!(services.clock.now_unix()),
        result.reason.clone().map_or(Value::Null, Value::String),
        scope,
        day.map_or(Value::Null, |day| Value::String(day.into())),
        serde_json::json!(result.segments_selected),
        serde_json::json!(result.segments_restored),
        serde_json::json!(result.files_expected),
        serde_json::json!(result.files_restored),
        serde_json::json!(result.bytes_expected),
        serde_json::json!(result.bytes_restored),
    );
    result
}
pub fn restore_offload_day(
    journal: &Path,
    services: &BackupServices<'_>,
    day: &str,
) -> RestoreResult {
    match summarize_day(journal, day) {
        Ok(summary) if summary.degraded() => {
            base("error", Some("ledger_degraded"), "day", Some(day))
        }
        Ok(summary) => run(journal, services, "day", Some(day), summary.segments),
        Err(_) => base("error", Some("ledger_degraded"), "day", Some(day)),
    }
}
pub fn restore_all_offload(journal: &Path, services: &BackupServices<'_>) -> RestoreResult {
    let summary = summarize_journal(journal);
    if summary.degraded() {
        return base("error", Some("ledger_degraded"), "all", None);
    }
    run(
        journal,
        services,
        "all",
        None,
        summary
            .days
            .into_iter()
            .flat_map(|day| day.segments)
            .collect(),
    )
}
