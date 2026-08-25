// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Restore offloaded raw media without removing files that predated an attempt.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_backup::{
    assemble_backend_env, get_backup_config, get_destination, get_keys, load_hosted_binding,
    record_restore_result,
};
use solstone_core_backup_runtime::{BackupServices, reason_for_returncode, run_restic};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments, segment_path};
use solstone_core_journal_io::{bump_stream_marker, check_record_identities};
use solstone_core_retention::{Target, resolve_offload};

use crate::ledger::{
    OffloadFile, SegmentOffloadSummary, append_restore_event, summarize_day, summarize_journal,
};
use crate::measurement::device_free_bytes;

pub const RESTORE_RESERVE_BYTES: u64 = 1_000_000_000;
pub const OFFLOAD_RESTORE_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
pub const OFFLOAD_RESTORE_STATUSES: [&str; 5] = ["ok", "no_op", "refused", "degraded", "error"];
pub const OFFLOAD_RESTORE_REASONS: [&str; 16] = [
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
    "rclone_unavailable",
    "segment_identity",
    "segment_missing",
    "stream_marker_failed",
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
    /// Extra identity detail when `reason` is `segment_identity`. The reason
    /// token itself stays `segment_identity`.
    pub reason_detail: Option<String>,
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
        reason_detail: None,
        details: vec![],
    }
}
impl RestoreResult {
    fn with_reason_detail(mut self, detail: String) -> Self {
        self.reason_detail = Some(detail);
        self
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
    let listed = iter_segments(journal, PathOrDay::Day(&summary.day)).unwrap_or_default();
    let matches: Vec<_> = listed
        .iter()
        .filter(|segment| {
            segment.record_identity().ok().is_some_and(|identity| {
                identity.stream == summary.stream && identity.key == summary.segment
            })
        })
        .collect();
    match matches.as_slice() {
        [segment] => segment.path().to_path_buf(),
        [] => segment_path(
            journal,
            &summary.day,
            &summary.segment,
            &summary.stream,
            false,
        )
        .unwrap_or_else(|_| PathBuf::new()),
        _ => PathBuf::new(),
    }
}
fn restore_segment(
    journal: &Path,
    services: &BackupServices<'_>,
    summary: &SegmentOffloadSummary,
) -> (RestoreSegmentResult, bool, Option<String>) {
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
        return (err("segment_missing"), true, None);
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
            return (err("backup_not_ready"), true, None);
        };
        let Ok(env) = assemble_backend_env(&destination) else {
            return (err("failed"), true, None);
        };
        let env = env
            .into_iter()
            .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
            .collect::<BTreeMap<_, _>>();
        let restic_path = match services.restic_path() {
            Ok(path) => path,
            Err(reason) => return (err(&reason), true, None),
        };
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
            restic_path,
            Some(&env),
            true,
            None,
            Some(Duration::from_secs(OFFLOAD_RESTORE_TIMEOUT_SECONDS)),
            &[],
        ) {
            Ok(output) => output,
            Err(_) => return (err("failed"), true, None),
        };
        if output.returncode != 0 {
            rollback(&directory, &absent);
            return (err(reason_for_returncode(output.returncode)), true, None);
        }
    }
    if let Some(reason) = verify(&directory, &summary.files) {
        rollback(&directory, &absent);
        return (err(reason), true, None);
    }
    if !absent.is_empty()
        && let Err(error) = bump_stream_marker(journal, &summary.day)
    {
        return (
            RestoreSegmentResult {
                status: "error".into(),
                reason: Some("stream_marker_failed".into()),
                day: summary.day.clone(),
                stream: summary.stream.clone(),
                segment: summary.segment.clone(),
                snapshot_id: summary.snapshot_id.clone(),
                files_expected: summary.offloaded_file_count,
                files_restored: summary.offloaded_file_count,
                bytes_expected: summary.offloaded_bytes,
                bytes_restored: summary.offloaded_bytes,
            },
            true,
            Some(error.to_string()),
        );
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
        return (err("failed"), true, None);
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
        return (err("failed"), false, None);
    };
    (
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
        },
        true,
        None,
    )
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
    let mut listed = Vec::new();
    let days: BTreeSet<String> = selected.iter().map(|segment| segment.day.clone()).collect();
    for restore_day in &days {
        listed.extend(iter_segments(journal, PathOrDay::Day(restore_day)).unwrap_or_default());
    }
    if let Err(error) = check_record_identities(&listed) {
        return base("refused", Some("segment_identity"), scope, day)
            .with_reason_detail(error.to_string());
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
    if operated_restore_requires_rclone(journal, services) {
        let mut result = base("error", Some("rclone_unavailable"), scope, day);
        result.segments_selected = selected.len() as u64;
        result.files_expected = selected
            .iter()
            .map(|segment| segment.offloaded_file_count)
            .sum();
        result.bytes_expected = expected;
        return result;
    }
    let mut details = vec![];
    let mut record_result = true;
    let mut reason_detail = None;
    for segment in &selected {
        let (detail, should_record, detail_reason) = restore_segment(journal, services, segment);
        record_result &= should_record;
        if reason_detail.is_none() {
            reason_detail = detail_reason;
        }
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
            detail.status == "error"
                && !matches!(
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
        files_restored: details.iter().map(|detail| detail.files_restored).sum(),
        bytes_expected: expected,
        bytes_restored: details.iter().map(|detail| detail.bytes_restored).sum(),
        reason_detail,
        details,
    };
    if record_result {
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
    }
    result
}
fn operated_restore_requires_rclone(journal: &Path, services: &BackupServices<'_>) -> bool {
    let Ok(config) = get_backup_config(journal) else {
        return false;
    };
    config.get("enabled") == Some(&Value::Bool(true))
        && config.get("mode") == Some(&Value::String("operated".into()))
        && get_keys(journal).ok().flatten().is_some()
        && load_hosted_binding(journal).is_some()
        && services.rclone_path.is_none()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use crate::ledger::append_offload_event;
    use solstone_core_backup::{
        HostedBinding, generate_and_store_keys, save_hosted_binding, set_enabled, set_mode,
    };
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::{
        Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };
    use solstone_core_retention::{Target, upsert_offload};

    struct RestoreRunner {
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl ToolRunner for RestoreRunner {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            let args = request
                .argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            self.calls.borrow_mut().push(args.clone());
            if args.first().is_some_and(|arg| arg == "restore") {
                let target = args
                    .windows(2)
                    .find(|args| args[0] == "--target")
                    .map(|args| PathBuf::from(&args[1]))
                    .expect("restore target");
                // The restored file was absent before the attempt, and remains
                // corrupt so verification exercises rollback through the public path.
                fs::write(target.join("new.webm"), b"corrupt").unwrap();
            }
            Ok(ToolOutput {
                returncode: 0,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }

    struct HardFailureRunner;
    impl ToolRunner for HardFailureRunner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Ok(ToolOutput {
                returncode: 12,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }

    struct SuccessfulRestoreRunner {
        bytes: &'static [u8],
    }

    impl ToolRunner for SuccessfulRestoreRunner {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            let args = request
                .argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            if args.first().is_some_and(|arg| arg == "restore") {
                let target = args
                    .windows(2)
                    .find(|args| args[0] == "--target")
                    .map(|args| PathBuf::from(&args[1]))
                    .expect("restore target");
                for include in args
                    .windows(2)
                    .filter(|args| args[0] == "--include")
                    .map(|args| args[1].trim_start_matches('/'))
                {
                    fs::write(target.join(include), self.bytes).unwrap();
                }
            }
            Ok(ToolOutput {
                returncode: 0,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }

    struct Http;
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            unreachable!("BYO restore does not use HTTP")
        }
    }

    struct TestClock;
    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            1
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

    fn services<'a>(
        runner: &'a RestoreRunner,
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

    fn hard_failure_services<'a>(
        runner: &'a HardFailureRunner,
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

    fn successful_restore_services<'a>(
        runner: &'a SuccessfulRestoreRunner,
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

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn marked_segment(journal: &Path, day: &str, segment_key: &str, bytes: &[u8]) -> PathBuf {
        let segment = journal.join("chronicle").join(day).join(segment_key);
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("raw.webm"), bytes).unwrap();
        let file = OffloadFile {
            name: "raw.webm".into(),
            bytes: bytes.len() as u64,
            sha256: digest(bytes),
        };
        append_offload_event(
            journal,
            day,
            "_default",
            segment_key,
            "snapshot",
            &[file],
            1,
        )
        .unwrap();
        upsert_offload(
            journal,
            &Target {
                day: day.into(),
                stream: "_default".into(),
                dir: segment_key.into(),
            },
            vec!["raw.webm".into()],
            bytes.len() as u64,
            "restic-snapshot:snapshot".into(),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
        )
        .unwrap();
        segment
    }

    fn empty_runner() -> RestoreRunner {
        RestoreRunner {
            calls: RefCell::new(vec![]),
        }
    }

    fn configure_test_repository(journal: &Path) {
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            r#"{"backup":{"daily_key":"PASSWORDONLY","recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ","destination":{"repository":"s3:bucket/prefix","backend":"s3","credentials":{"access_key_id":"ACCESSFIXTURE","secret_access_key":"BACKENDSECRET"}}}}"#,
        )
        .unwrap();
    }

    #[test]
    fn restored_media_dirties_each_exact_day_and_no_untouched_day() {
        let journal = tempfile::tempdir().unwrap();
        let restored = b"restored";
        for (day, segment) in [("20260112", "120000_012"), ("20260113", "130000_013")] {
            let directory = marked_segment(journal.path(), day, segment, restored);
            fs::remove_file(directory.join("raw.webm")).unwrap();
        }
        fs::create_dir_all(journal.path().join("chronicle/20260114/140000_014")).unwrap();
        configure_test_repository(journal.path());
        let runner = SuccessfulRestoreRunner { bytes: restored };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = restore_all_offload(
            journal.path(),
            &successful_restore_services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "ok", "{result:?}");
        for day in ["20260112", "20260113"] {
            let marker: Value = serde_json::from_slice(
                &fs::read(
                    journal
                        .path()
                        .join("chronicle")
                        .join(day)
                        .join("health/stream.updated"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(marker["generation"], 1, "{day}");
        }
        assert!(
            !journal
                .path()
                .join("chronicle/20260114/health/stream.updated")
                .exists()
        );
    }

    #[test]
    fn restore_marker_failure_is_terminal_with_restored_bytes_and_mark_retained() {
        let journal = tempfile::tempdir().unwrap();
        let restored = b"restored";
        let segment = marked_segment(journal.path(), "20260115", "150000_015", restored);
        fs::remove_file(segment.join("raw.webm")).unwrap();
        let marker = journal
            .path()
            .join("chronicle/20260115/health/stream.updated");
        fs::create_dir_all(&marker).unwrap();
        configure_test_repository(journal.path());
        let runner = SuccessfulRestoreRunner { bytes: restored };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = restore_offload_day(
            journal.path(),
            &successful_restore_services(&runner, &http, &clock, &maintenance),
            "20260115",
        );

        assert_eq!(result.status, "error", "{result:?}");
        assert_eq!(result.reason.as_deref(), Some("stream_marker_failed"));
        assert!(
            result
                .reason_detail
                .as_deref()
                .is_some_and(|detail| { detail.contains(&marker.display().to_string()) })
        );
        assert_eq!(result.files_restored, 1);
        assert_eq!(result.bytes_restored, restored.len() as u64);
        assert_eq!(fs::read(segment.join("raw.webm")).unwrap(), restored);
        let register: solstone_core_retention::Register = serde_json::from_slice(
            &fs::read(journal.path().join("health/retention-marks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            register.marks.len(),
            1,
            "marker failure must precede resolution"
        );
    }

    #[test]
    fn rollback_keeps_preexisting_bytes_and_removes_only_attempted_files() {
        let journal = tempfile::tempdir().unwrap();
        let segment = journal.path().join("chronicle/20260101/010000_001");
        fs::create_dir_all(&segment).unwrap();
        let original = b"owner-owned bytes";
        fs::write(segment.join("keep.webm"), original).unwrap();
        append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000_001",
            "snapshot",
            &[
                OffloadFile {
                    name: "keep.webm".into(),
                    bytes: original.len() as u64,
                    sha256: digest(original),
                },
                OffloadFile {
                    name: "new.webm".into(),
                    bytes: 8,
                    sha256: digest(b"expected"),
                },
            ],
            1,
        )
        .unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            r#"{"backup":{"daily_key":"PASSWORDONLY","recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ","destination":{"repository":"s3:bucket/prefix","backend":"s3","credentials":{"access_key_id":"ACCESSFIXTURE","secret_access_key":"BACKENDSECRET"}}}}"#,
        )
        .unwrap();
        let destination = get_destination(journal.path()).unwrap().unwrap();
        assert!(assemble_backend_env(&destination).is_ok());
        assert!(get_keys(journal.path()).unwrap().is_some());

        let runner = RestoreRunner {
            calls: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260101",
        );

        assert_eq!(result.status, "error");
        assert_eq!(
            result.reason.as_deref(),
            Some("verification_failed"),
            "runner calls: {:?}",
            runner.calls.borrow()
        );
        assert_eq!(fs::read(segment.join("keep.webm")).unwrap(), original);
        assert!(!segment.join("new.webm").exists());
        assert_eq!(
            runner.calls.borrow().first().and_then(|args| args.first()),
            Some(&"restore".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn mark_resolution_failure_keeps_mark_and_does_not_append_restore_event() {
        let journal = tempfile::tempdir().unwrap();
        let segment = marked_segment(journal.path(), "20260102", "020000_002", b"marked");
        let register = journal.path().join("health/retention-marks.json");
        let before_mark = fs::read(&register).unwrap();
        let ledger = journal.path().join("health/offload/20260102.jsonl");
        let before_ledger = fs::read(&ledger).unwrap();
        fs::set_permissions(
            journal.path().join("health"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();

        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260102",
        );
        fs::set_permissions(
            journal.path().join("health"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        assert_eq!(result.status, "error");
        assert_eq!(result.reason.as_deref(), Some("failed"));
        assert_eq!(fs::read(segment.join("raw.webm")).unwrap(), b"marked");
        // The unchanged mark and ledger prove resolve ran before append: a failed
        // resolve returned before a restore event could be written.
        assert_eq!(fs::read(&register).unwrap(), before_mark);
        assert_eq!(fs::read(&ledger).unwrap(), before_ledger);
    }

    #[cfg(unix)]
    #[test]
    fn ledger_append_failure_resolves_mark_before_leaving_ledger_current() {
        let journal = tempfile::tempdir().unwrap();
        let segment = marked_segment(journal.path(), "20260103", "030000_003", b"ledger");
        let register = journal.path().join("health/retention-marks.json");
        let ledger = journal.path().join("health/offload/20260103.jsonl");
        let before_ledger = fs::read(&ledger).unwrap();
        fs::set_permissions(&ledger, fs::Permissions::from_mode(0o444)).unwrap();

        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260103",
        );
        fs::set_permissions(&ledger, fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(result.status, "error");
        assert_eq!(result.reason.as_deref(), Some("failed"));
        assert_eq!(fs::read(segment.join("raw.webm")).unwrap(), b"ledger");
        // A missing mark with an unchanged offload ledger proves resolution completed
        // before the failed restore-event append was attempted.
        assert!(
            serde_json::from_slice::<solstone_core_retention::Register>(
                &fs::read(&register).unwrap(),
            )
            .unwrap()
            .marks
            .is_empty()
        );
        assert_eq!(fs::read(&ledger).unwrap(), before_ledger);
        assert!(
            !journal.path().join("config/journal.json").exists(),
            "a failed ledger append must not publish the final restore result"
        );
    }

    #[test]
    fn verification_failure_leaves_mark_and_ledger_unchanged_before_resolution() {
        let journal = tempfile::tempdir().unwrap();
        let segment = marked_segment(journal.path(), "20260104", "040000_004", b"expected");
        fs::write(segment.join("raw.webm"), b"corrupt").unwrap();
        let register = journal.path().join("health/retention-marks.json");
        let before_mark = fs::read(&register).unwrap();
        let ledger = journal.path().join("health/offload/20260104.jsonl");
        let before_ledger = fs::read(&ledger).unwrap();

        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260104",
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.reason.as_deref(), Some("verification_failed"));
        // The still-current mark and ledger show verification stopped the segment
        // before either resolution or restore-event publication could run.
        assert_eq!(fs::read(&register).unwrap(), before_mark);
        assert_eq!(fs::read(&ledger).unwrap(), before_ledger);
    }

    #[test]
    fn mixed_success_and_verification_damage_is_degraded() {
        let journal = tempfile::tempdir().unwrap();
        marked_segment(journal.path(), "20260105", "050000_005", b"five");
        let damaged = marked_segment(journal.path(), "20260106", "060000_006", b"sixsix");
        fs::write(damaged.join("raw.webm"), b"damage").unwrap();

        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let result = restore_all_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "degraded", "{result:?}");
        assert_eq!(result.reason.as_deref(), Some("verification_failed"));
        assert_eq!(result.segments_restored, 1);
    }

    #[test]
    fn hard_runtime_failure_is_error_after_an_earlier_success() {
        let journal = tempfile::tempdir().unwrap();
        marked_segment(journal.path(), "20260107", "070000_007", b"seven");
        let missing = marked_segment(journal.path(), "20260108", "080000_008", b"eight");
        fs::remove_file(missing.join("raw.webm")).unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            r#"{"backup":{"daily_key":"PASSWORDONLY","recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ","destination":{"repository":"s3:bucket/prefix","backend":"s3","credentials":{"access_key_id":"ACCESSFIXTURE","secret_access_key":"BACKENDSECRET"}}}}"#,
        )
        .unwrap();

        let runner = HardFailureRunner;
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let result = restore_all_offload(
            journal.path(),
            &hard_failure_services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.reason.as_deref(), Some("auth_failed"));
        assert_eq!(result.segments_restored, 1);
    }

    #[test]
    fn empty_journal_is_a_no_op() {
        let journal = tempfile::tempdir().unwrap();
        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = restore_all_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "no_op");
        assert_eq!(result.reason.as_deref(), Some("nothing_to_restore"));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn absent_restic_path_fails_lazily_without_runner_call() {
        let journal = tempfile::tempdir().unwrap();
        let segment = marked_segment(journal.path(), "20260111", "110000_011", b"eleven");
        fs::remove_file(segment.join("raw.webm")).unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            r#"{"backup":{"daily_key":"PASSWORDONLY","recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ","destination":{"repository":"s3:bucket/prefix","backend":"s3","credentials":{"access_key_id":"ACCESSFIXTURE","secret_access_key":"BACKENDSECRET"}}}}"#,
        )
        .unwrap();
        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.restic_path = None;

        let result = restore_offload_day(journal.path(), &services, "20260111");

        assert_eq!(result.status, "error");
        assert_eq!(result.reason.as_deref(), Some("restic_unavailable"));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn day_and_all_select_their_respective_remaining_segments() {
        let journal = tempfile::tempdir().unwrap();
        marked_segment(journal.path(), "20260109", "090000_009", b"nine");
        marked_segment(journal.path(), "20260110", "100000_010", b"ten-ten");
        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let day = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260109",
        );
        let all = restore_all_offload(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!((day.scope.as_str(), day.segments_selected), ("day", 1));
        assert_eq!((all.scope.as_str(), all.segments_selected), ("all", 1));
    }

    #[test]
    fn already_present_files_restore_without_invoking_restic() {
        let journal = tempfile::tempdir().unwrap();
        marked_segment(journal.path(), "20260111", "110000_011", b"present");
        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260111",
        );

        assert_eq!(result.status, "ok");
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn free_space_guard_refuses_before_attempting_restore() {
        let journal = tempfile::tempdir().unwrap();
        let unavailable_bytes = u64::MAX - RESTORE_RESERVE_BYTES;
        let segment = journal.path().join("chronicle/20260112/120000_012");
        fs::create_dir_all(&segment).unwrap();
        let file = OffloadFile {
            name: "large.webm".into(),
            bytes: unavailable_bytes,
            sha256: digest(b"large"),
        };
        append_offload_event(
            journal.path(),
            "20260112",
            "_default",
            "120000_012",
            "snapshot",
            &[file],
            1,
        )
        .unwrap();
        upsert_offload(
            journal.path(),
            &Target {
                day: "20260112".into(),
                stream: "_default".into(),
                dir: "120000_012".into(),
            },
            vec!["large.webm".into()],
            unavailable_bytes,
            "restic-snapshot:snapshot".into(),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
        )
        .unwrap();
        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260112",
        );

        assert_eq!(result.status, "refused");
        assert_eq!(result.reason.as_deref(), Some("insufficient_free_space"));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn missing_segment_reports_the_reference_reason_code() {
        let journal = tempfile::tempdir().unwrap();
        let segment = marked_segment(journal.path(), "20260113", "130000_013", b"missing");
        fs::remove_dir_all(segment).unwrap();
        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260113",
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.reason.as_deref(), Some("segment_missing"));
    }

    #[test]
    fn configured_operated_restore_without_rclone_reports_the_reference_reason_code() {
        let journal = tempfile::tempdir().unwrap();
        marked_segment(journal.path(), "20260114", "140000_014", b"operated");
        generate_and_store_keys(journal.path()).unwrap();
        set_enabled(journal.path(), true).unwrap();
        set_mode(journal.path(), "operated").unwrap();
        save_hosted_binding(
            journal.path(),
            &HostedBinding {
                broker_endpoint: "https://broker.example".into(),
                account_id: "account".into(),
                instance_id: "instance".into(),
                bucket: "bucket".into(),
                prefix: "prefix".into(),
                broker_token: "fixture-token".into(),
            },
        )
        .unwrap();
        let runner = empty_runner();
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = restore_offload_day(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            "20260114",
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.reason.as_deref(), Some("rclone_unavailable"));
        assert!(runner.calls.borrow().is_empty());
    }
}
