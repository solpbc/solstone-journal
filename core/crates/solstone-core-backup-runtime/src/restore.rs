// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whole-journal restore and its sealed attempt record.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_backup::{
    Destination, assemble_backend_env, get_backup_config, parse_recovery_key, set_destination,
    set_recovery_key, set_recovery_key_confirmed,
};

use crate::engine::{BackupServices, Clock, RestoreRecorder};
use crate::restore_catalog::{CatalogError, select_journal_snapshot};
use crate::runner::{ResticResult, reason_for_returncode, run_restic, select_summary};

pub const RESTORE_LIST_TIMEOUT_SECONDS: u64 = 5 * 60;
pub const RESTORE_TIMEOUT_SECONDS: u64 = 48 * 60 * 60;
pub const RESTORE_CHECK_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;

pub const RESTORE_REASON_INVALID_KEY: &str = "invalid_key";
pub const RESTORE_REASON_RESTIC_UNAVAILABLE: &str = "restic_unavailable";
pub const RESTORE_REASON_DESTINATION_INVALID: &str = "destination_invalid";
pub const RESTORE_REASON_SNAPSHOT_LIST_IO_FAILED: &str = "snapshot_list_io_failed";
pub const RESTORE_REASON_SNAPSHOT_LIST_FAILED: &str = "snapshot_list_failed";
pub const RESTORE_REASON_SNAPSHOT_CATALOG_INVALID: &str = "snapshot_catalog_invalid";
pub const RESTORE_REASON_JOURNAL_SNAPSHOT_NOT_FOUND: &str = "journal_snapshot_not_found";
pub const RESTORE_REASON_SNAPSHOT_SELECTION_AMBIGUOUS: &str = "snapshot_selection_ambiguous";
pub const RESTORE_REASON_RESTORE_IO_FAILED: &str = "restore_io_failed";
pub const RESTORE_REASON_RESTORE_FAILED: &str = "restore_failed";
pub const RESTORE_REASON_RESTORE_SUMMARY_MISSING: &str = "restore_summary_missing";
pub const RESTORE_REASON_INTEGRITY_CHECK_IO_FAILED: &str = "integrity_check_io_failed";
pub const RESTORE_REASON_INTEGRITY_UNVERIFIED: &str = "integrity_unverified";
pub const RESTORE_REASON_INTEGRITY_FAILED: &str = "integrity_failed";
pub const RESTORE_REASON_BODY_REBUILD_FAILED: &str = "body_rebuild_failed";
pub const RESTORE_REASON_DESTINATION_PUBLISH_FAILED: &str = "destination_publish_failed";
pub const RESTORE_REASON_RECOVERY_KEY_PUBLISH_FAILED: &str = "recovery_key_publish_failed";
pub const RESTORE_REASON_RECOVERY_CONFIRMATION_PUBLISH_FAILED: &str =
    "recovery_confirmation_publish_failed";
pub const RESTORE_REASON_FULL_SCAN_FAILED: &str = "full_scan_failed";
pub const RESTORE_REASON_RESTORE_RECORD_FAILED: &str = "restore_record_failed";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDraft {
    pub status: String,
    pub reason_code: Option<String>,
    pub integrity_ok: bool,
    pub resumable: bool,
    pub files_expected: Option<u64>,
    pub files_restored: Option<u64>,
    pub bytes_expected: Option<u64>,
    pub bytes_restored: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::manual_non_exhaustive)] // Private ZST deliberately prevents external literals.
pub struct RestoreOutcome {
    pub status: String,
    pub reason_code: Option<String>,
    pub recording_failure: Option<String>,
    pub integrity_ok: bool,
    pub resumable: bool,
    pub files_expected: Option<u64>,
    pub files_restored: Option<u64>,
    pub bytes_expected: Option<u64>,
    pub bytes_restored: Option<u64>,
    _sealed: (),
}

/// Record exactly one whole-journal restore attempt, then seal the result.
pub fn publish_restore_outcome(
    journal: &Path,
    clock: &dyn Clock,
    recorder: &dyn RestoreRecorder,
    draft: RestoreDraft,
) -> RestoreOutcome {
    let record_result = recorder.record(
        journal,
        &draft.status,
        json!(clock.now_unix()),
        option_string_value(draft.reason_code.as_deref()),
        "journal",
        Value::Null,
        json!(0),
        json!(0),
        option_number_value(draft.files_expected),
        option_number_value(draft.files_restored),
        option_number_value(draft.bytes_expected),
        option_number_value(draft.bytes_restored),
    );
    let recording_failure = record_result
        .err()
        .map(|_| RESTORE_REASON_RESTORE_RECORD_FAILED.to_owned());
    let (status, reason_code) = match recording_failure.as_deref() {
        None => (draft.status, draft.reason_code),
        Some(_) if draft.status == "ok" => (
            "error".to_owned(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED.to_owned()),
        ),
        Some(_) => ("error".to_owned(), draft.reason_code),
    };
    RestoreOutcome {
        status,
        reason_code,
        recording_failure,
        integrity_ok: draft.integrity_ok,
        resumable: draft.resumable,
        files_expected: draft.files_expected,
        files_restored: draft.files_restored,
        bytes_expected: draft.bytes_expected,
        bytes_restored: draft.bytes_restored,
        _sealed: (),
    }
}

fn option_string_value(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_owned()))
}

fn option_number_value(value: Option<u64>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

#[derive(Clone, Copy, Default)]
struct RestoreCounters {
    files_expected: Option<u64>,
    files_restored: Option<u64>,
    bytes_expected: Option<u64>,
    bytes_restored: Option<u64>,
}

impl RestoreCounters {
    fn into_draft(
        self,
        status: &str,
        reason_code: Option<String>,
        integrity_ok: bool,
        resumable: bool,
    ) -> RestoreDraft {
        RestoreDraft {
            status: status.to_owned(),
            reason_code,
            integrity_ok,
            resumable,
            files_expected: self.files_expected,
            files_restored: self.files_restored,
            bytes_expected: self.bytes_expected,
            bytes_restored: self.bytes_restored,
        }
    }
}

fn error_draft(reason_code: &str) -> RestoreDraft {
    RestoreCounters::default().into_draft("error", Some(reason_code.to_owned()), false, false)
}

fn backend(destination: &Destination) -> Result<BTreeMap<String, Option<String>>, &'static str> {
    assemble_backend_env(destination)
        .map(|env| {
            env.into_iter()
                .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
                .collect()
        })
        .map_err(|_| RESTORE_REASON_DESTINATION_INVALID)
}

#[allow(clippy::too_many_arguments)] // Mirrors the shared restic invocation boundary.
fn restic(
    services: &BackupServices<'_>,
    args: Vec<String>,
    destination: &Destination,
    key: &str,
    env: &BTreeMap<String, Option<String>>,
    json_output: bool,
    timeout: u64,
    runner_error_reason: &'static str,
) -> Result<ResticResult, &'static str> {
    let restic_path = services
        .restic_path()
        .map_err(|_| RESTORE_REASON_RESTIC_UNAVAILABLE)?;
    run_restic(
        services.runner,
        &args,
        &destination.repository,
        key,
        restic_path,
        Some(env),
        json_output,
        None,
        Some(Duration::from_secs(timeout)),
        &[],
    )
    .map_err(|_| runner_error_reason)
}

fn recognized_returncode_reason(returncode: i32) -> Option<&'static str> {
    match returncode {
        3 | 10 | 11 | 12 | 124 => Some(reason_for_returncode(returncode)),
        _ => None,
    }
}

fn restore_counters(parsed: Option<&Value>) -> Option<RestoreCounters> {
    let summary = select_summary(parsed?)?;
    let files_expected = summary_counter(summary, "total_files")?;
    let files_restored = summary_counter(summary, "files_restored")?;
    let bytes_expected = summary_counter(summary, "total_bytes")?;
    let bytes_restored = summary_counter(summary, "bytes_restored")?;
    Some(RestoreCounters {
        files_expected: Some(files_expected),
        files_restored: Some(files_restored),
        bytes_expected: Some(bytes_expected),
        bytes_restored: Some(bytes_restored),
    })
}

fn summary_counter(summary: &Map<String, Value>, field: &str) -> Option<u64> {
    // Restic omits zero-valued summary counters, but a present counter must be an unsigned integer.
    match summary.get(field) {
        None => Some(0),
        Some(value) => value.as_u64(),
    }
}

fn finish(
    journal: &Path,
    clock: &dyn Clock,
    recorder: &dyn RestoreRecorder,
    draft: RestoreDraft,
) -> RestoreOutcome {
    publish_restore_outcome(journal, clock, recorder, draft)
}

/// Restore a BYO repository. The three config setters remain purposefully independent.
pub fn restore_journal(
    journal: &Path,
    services: &BackupServices<'_>,
    recorder: &dyn RestoreRecorder,
    destination: Destination,
    entered_recovery_key: &str,
) -> RestoreOutcome {
    let canonical = match parse_recovery_key(entered_recovery_key) {
        Ok(key) => key,
        Err(_) => {
            return finish(
                journal,
                services.clock,
                recorder,
                error_draft(RESTORE_REASON_INVALID_KEY),
            );
        }
    };
    let env = match backend(&destination) {
        Ok(env) => env,
        Err(reason) => return finish(journal, services.clock, recorder, error_draft(reason)),
    };
    let snapshots = match restic(
        services,
        vec!["snapshots".into()],
        &destination,
        &canonical,
        &env,
        true,
        RESTORE_LIST_TIMEOUT_SECONDS,
        // This opaque code intentionally covers every RunnerError variant, not only I/O.
        RESTORE_REASON_SNAPSHOT_LIST_IO_FAILED,
    ) {
        Ok(output) => output,
        Err(reason) => return finish(journal, services.clock, recorder, error_draft(reason)),
    };
    if snapshots.returncode != 0 {
        let reason = recognized_returncode_reason(snapshots.returncode)
            .unwrap_or(RESTORE_REASON_SNAPSHOT_LIST_FAILED);
        return finish(journal, services.clock, recorder, error_draft(reason));
    }
    let snapshot = match select_journal_snapshot(snapshots.json.as_ref()) {
        Ok(snapshot) => snapshot,
        Err(CatalogError::Invalid) => {
            return finish(
                journal,
                services.clock,
                recorder,
                error_draft(RESTORE_REASON_SNAPSHOT_CATALOG_INVALID),
            );
        }
        Err(CatalogError::NotFound) => {
            return finish(
                journal,
                services.clock,
                recorder,
                error_draft(RESTORE_REASON_JOURNAL_SNAPSHOT_NOT_FOUND),
            );
        }
        Err(CatalogError::Ambiguous) => {
            return finish(
                journal,
                services.clock,
                recorder,
                error_draft(RESTORE_REASON_SNAPSHOT_SELECTION_AMBIGUOUS),
            );
        }
    };
    let restored = match restic(
        services,
        vec![
            "restore".into(),
            format!("{}:{}", snapshot.id, snapshot.path),
            "--target".into(),
            journal.display().to_string(),
        ],
        &destination,
        &canonical,
        &env,
        true,
        RESTORE_TIMEOUT_SECONDS,
        // This opaque code intentionally covers every RunnerError variant, not only I/O.
        RESTORE_REASON_RESTORE_IO_FAILED,
    ) {
        Ok(output) => output,
        Err(reason) => return finish(journal, services.clock, recorder, error_draft(reason)),
    };
    if restored.returncode != 0 {
        let reason = recognized_returncode_reason(restored.returncode)
            .unwrap_or(RESTORE_REASON_RESTORE_FAILED);
        return finish(journal, services.clock, recorder, error_draft(reason));
    }
    let (counters, summary_missing) = match restore_counters(restored.json.as_ref()) {
        Some(counters) => (counters, false),
        None => (RestoreCounters::default(), true),
    };
    let check = match restic(
        services,
        vec!["check".into()],
        &destination,
        &canonical,
        &env,
        false,
        RESTORE_CHECK_TIMEOUT_SECONDS,
        // This opaque code intentionally covers every RunnerError variant, not only I/O.
        RESTORE_REASON_INTEGRITY_CHECK_IO_FAILED,
    ) {
        Ok(output) => output,
        Err(reason) => {
            return finish(
                journal,
                services.clock,
                recorder,
                counters.into_draft("error", Some(reason.to_owned()), false, false),
            );
        }
    };
    let (status, reason_code, integrity_ok) = if check.returncode == 0 {
        if summary_missing {
            (
                "degraded",
                Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING.to_owned()),
                true,
            )
        } else {
            ("ok", None, true)
        }
    } else if matches!(check.returncode, 11 | 124) {
        (
            "degraded",
            Some(RESTORE_REASON_INTEGRITY_UNVERIFIED.to_owned()),
            false,
        )
    } else {
        (
            "degraded",
            Some(RESTORE_REASON_INTEGRITY_FAILED.to_owned()),
            false,
        )
    };
    if services
        .journal_maintenance
        .rebuild_body_history(journal)
        .is_err()
    {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_BODY_REBUILD_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    if set_destination(journal, &destination).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_DESTINATION_PUBLISH_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    if set_recovery_key(journal, &canonical).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_RECOVERY_KEY_PUBLISH_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    if set_recovery_key_confirmed(journal, true).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_RECOVERY_CONFIRMATION_PUBLISH_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    let resumable = get_backup_config(journal)
        .ok()
        .and_then(|config| {
            config
                .get("daily_key")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|key| !key.is_empty());
    if services.journal_maintenance.full_scan(journal).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_FULL_SCAN_FAILED.to_owned()),
                integrity_ok,
                resumable,
            ),
        );
    }
    finish(
        journal,
        services.clock,
        recorder,
        counters.into_draft(status, reason_code, integrity_ok, resumable),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{JournalMaintenance, JournalMaintenanceError, NativeRestoreRecorder};
    use crate::hosted_runtime::{HttpError, HttpRequest, HttpResponse, HttpTransport};
    use crate::runner::{ToolOutput, ToolRequest, ToolRunner};
    use crate::test_support::RestoreRecorderSpy;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;

    const SNAPSHOT_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Script {
        outputs: RefCell<VecDeque<ToolOutput>>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Script {
        fn new(outputs: Vec<ToolOutput>) -> Self {
            Self {
                outputs: RefCell::new(outputs.into()),
                calls: RefCell::new(vec![]),
            }
        }
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
                .expect("script output"))
        }
    }

    struct Http;
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("whole-journal restore does not use hosted HTTP")
        }
    }

    struct TestClock;
    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            50
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

    struct Recorder {
        fail: bool,
        calls: RefCell<Vec<(String, Value, Value, Value, Value)>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                fail: false,
                calls: RefCell::new(vec![]),
            }
        }

        fn failing() -> Self {
            Self {
                fail: true,
                calls: RefCell::new(vec![]),
            }
        }
    }

    impl RestoreRecorder for Recorder {
        fn record(
            &self,
            _: &Path,
            status: &str,
            _: Value,
            reason: Value,
            scope: &str,
            day: Value,
            segments_selected: Value,
            segments_restored: Value,
            files_expected: Value,
            files_restored: Value,
            bytes_expected: Value,
            bytes_restored: Value,
        ) -> Result<(), solstone_core_backup::BackupError> {
            assert_eq!(scope, "journal");
            assert_eq!(day, Value::Null);
            assert_eq!(segments_selected, json!(0));
            assert_eq!(segments_restored, json!(0));
            self.calls.borrow_mut().push((
                status.to_owned(),
                reason,
                files_expected,
                files_restored,
                json!([bytes_expected, bytes_restored]),
            ));
            if self.fail {
                Err(solstone_core_backup::BackupError::InvalidRestoreStatus)
            } else {
                Ok(())
            }
        }
    }

    fn output(returncode: i32, stdout: &str) -> ToolOutput {
        ToolOutput {
            returncode,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
        }
    }

    fn catalog() -> String {
        format!(
            "[{{\"id\":\"{SNAPSHOT_ID}\",\"time\":\"2026-01-01T00:00:00.000000000+00:00\",\"paths\":[\"/journal\"]}}]"
        )
    }

    fn summary() -> &'static str {
        "[{\"message_type\":\"summary\",\"total_files\":3,\"files_restored\":2,\"total_bytes\":9,\"bytes_restored\":8}]"
    }

    fn destination() -> Destination {
        Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .expect("credentials")
                .clone(),
        }
    }

    fn services<'a>(
        runner: &'a dyn ToolRunner,
        clock: &'a dyn Clock,
        maintenance: &'a dyn JournalMaintenance,
    ) -> BackupServices<'a> {
        static HTTP: Http = Http;
        BackupServices {
            runner,
            http: &HTTP,
            clock,
            restic_path: Some(Path::new("/fixture/restic")),
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }

    fn restore_with_summary(summary: &str) -> (RestoreOutcome, RestoreRecorderSpy) {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = Script::new(vec![
            output(0, &catalog()),
            output(0, summary),
            output(0, ""),
        ]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = RestoreRecorderSpy::new();
        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            &stored.recovery_key,
        );

        (outcome, recorder)
    }

    fn assert_recorded_counters(
        recorder: &RestoreRecorderSpy,
        status: &str,
        files_expected: Value,
        files_restored: Value,
        bytes_expected: Value,
        bytes_restored: Value,
    ) {
        let calls = recorder.calls();
        assert_eq!(calls.len(), 1);
        let record = &calls[0];
        assert_eq!(record.status, status);
        assert_eq!(record.files_expected, files_expected);
        assert_eq!(record.files_restored, files_restored);
        assert_eq!(record.bytes_expected, bytes_expected);
        assert_eq!(record.bytes_restored, bytes_restored);
    }

    #[test]
    fn absent_restic_path_returns_existing_unavailable_reason_and_records_once() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = Script::new(vec![]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = Recorder::new();
        let mut runtime = services(&runner, &clock, &maintenance);
        runtime.restic_path = None;

        let outcome = restore_journal(
            journal.path(),
            &runtime,
            &recorder,
            destination(),
            &stored.recovery_key,
        );

        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTIC_UNAVAILABLE)
        );
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(recorder.calls.borrow().len(), 1);
    }

    #[test]
    fn invalid_key_records_once_without_a_runner_call() {
        let journal = tempfile::tempdir().expect("journal");
        let runner = Script::new(vec![]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = Recorder::new();

        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            "not a recovery key",
        );

        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_INVALID_KEY)
        );
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(recorder.calls.borrow().len(), 1);
    }

    #[test]
    fn selects_full_snapshot_id_and_publishes_all_summary_counters() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = crate::test_support::ArgvResticFixture::new(
            catalog(),
            output(0, summary()),
            output(0, ""),
        );
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = RestoreRecorderSpy::new();

        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            &stored.recovery_key,
        );

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.files_expected, Some(3));
        assert_eq!(outcome.files_restored, Some(2));
        assert_eq!(outcome.bytes_expected, Some(9));
        assert_eq!(outcome.bytes_restored, Some(8));
        assert_eq!(
            runner.calls()[0],
            vec!["snapshots".to_owned(), "--json".to_owned()]
        );
        assert_eq!(
            runner.calls()[1][..2],
            ["restore".to_owned(), format!("{SNAPSHOT_ID}:/journal")]
        );
        assert_recorded_counters(&recorder, "ok", json!(3), json!(2), json!(9), json!(8));
        assert!(runner.refusals().is_empty());
    }

    #[test]
    fn summary_omits_one_zero_counter_without_losing_other_counters() {
        let (outcome, recorder) = restore_with_summary(
            r#"[{"message_type":"summary","files_restored":2,"total_bytes":9,"bytes_restored":8}]"#,
        );

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.reason_code, None);
        assert_eq!(outcome.files_expected, Some(0));
        assert_eq!(outcome.files_restored, Some(2));
        assert_eq!(outcome.bytes_expected, Some(9));
        assert_eq!(outcome.bytes_restored, Some(8));
        assert_recorded_counters(&recorder, "ok", json!(0), json!(2), json!(9), json!(8));
    }

    #[test]
    fn summary_omits_all_zero_counters() {
        let (outcome, recorder) = restore_with_summary(r#"[{"message_type":"summary"}]"#);

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.reason_code, None);
        assert_eq!(outcome.files_expected, Some(0));
        assert_eq!(outcome.files_restored, Some(0));
        assert_eq!(outcome.bytes_expected, Some(0));
        assert_eq!(outcome.bytes_restored, Some(0));
        assert_recorded_counters(&recorder, "ok", json!(0), json!(0), json!(0), json!(0));
    }

    #[test]
    fn malformed_summary_counter_discards_all_summary_counters() {
        let (outcome, recorder) = restore_with_summary(
            r#"[{"message_type":"summary","total_files":12.0,"files_restored":2,"total_bytes":9,"bytes_restored":8}]"#,
        );

        assert_eq!(outcome.status, "degraded");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING)
        );
        assert!(outcome.integrity_ok);
        assert_eq!(outcome.files_expected, None);
        assert_eq!(outcome.files_restored, None);
        assert_eq!(outcome.bytes_expected, None);
        assert_eq!(outcome.bytes_restored, None);
        assert_recorded_counters(
            &recorder,
            "degraded",
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        );
    }

    #[test]
    fn absent_or_non_summary_output_is_degraded_with_null_counters() {
        for output in ["[]", r#"{"message_type":"status"}"#] {
            let (outcome, recorder) = restore_with_summary(output);

            assert_eq!(outcome.status, "degraded");
            assert_eq!(
                outcome.reason_code.as_deref(),
                Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING)
            );
            assert_eq!(outcome.files_expected, None);
            assert_eq!(outcome.files_restored, None);
            assert_eq!(outcome.bytes_expected, None);
            assert_eq!(outcome.bytes_restored, None);
            assert_recorded_counters(
                &recorder,
                "degraded",
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            );
        }
    }

    #[test]
    fn failure_replaces_a_prior_successful_last_restore_without_stale_fields() {
        let journal = tempfile::tempdir().expect("journal");
        solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        solstone_core_backup::record_restore_result(
            journal.path(),
            "ok",
            json!(1),
            Value::Null,
            "journal",
            Value::Null,
            json!(0),
            json!(0),
            json!(7),
            json!(7),
            json!(99),
            json!(99),
        )
        .expect("prior result");
        let runner = Script::new(vec![]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = NativeRestoreRecorder;

        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            "invalid",
        );

        assert_eq!(outcome.status, "error");
        let last = solstone_core_backup::get_backup_config(journal.path()).expect("config");
        let last = &last["last_restore"];
        assert_eq!(last["status"], "error");
        assert_eq!(last["reason"], RESTORE_REASON_INVALID_KEY);
        assert_eq!(last["scope"], "journal");
        assert_eq!(last["day"], Value::Null);
        assert_eq!(last["segments_selected"], 0);
        assert_eq!(last["segments_restored"], 0);
        assert_eq!(last["files_expected"], Value::Null);
        assert_eq!(last["files_restored"], Value::Null);
        assert_eq!(last["bytes_expected"], Value::Null);
        assert_eq!(last["bytes_restored"], Value::Null);
    }

    #[test]
    fn recording_failure_is_a_single_attempt_and_preserves_the_runtime_outcome() {
        let clock = TestClock;
        let recorder = Recorder::failing();
        let outcome = publish_restore_outcome(
            Path::new("/journal"),
            &clock,
            &recorder,
            RestoreDraft {
                status: "degraded".into(),
                reason_code: Some(RESTORE_REASON_INTEGRITY_UNVERIFIED.into()),
                integrity_ok: false,
                resumable: true,
                files_expected: Some(3),
                files_restored: Some(2),
                bytes_expected: Some(9),
                bytes_restored: Some(8),
            },
        );

        assert_eq!(recorder.calls.borrow().len(), 1);
        assert_eq!(outcome.status, "error");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_INTEGRITY_UNVERIFIED)
        );
        assert_eq!(
            outcome.recording_failure.as_deref(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
        );
        assert_eq!(outcome.bytes_restored, Some(8));
    }

    #[test]
    fn recording_failure_reclassifies_an_otherwise_successful_restore() {
        let clock = TestClock;
        let recorder = Recorder::failing();
        let outcome = publish_restore_outcome(
            Path::new("/journal"),
            &clock,
            &recorder,
            RestoreDraft {
                status: "ok".into(),
                reason_code: None,
                integrity_ok: true,
                resumable: true,
                files_expected: Some(3),
                files_restored: Some(3),
                bytes_expected: Some(9),
                bytes_restored: Some(9),
            },
        );

        assert_eq!(recorder.calls.borrow().len(), 1);
        assert_eq!(outcome.status, "error");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
        );
        assert_eq!(
            outcome.recording_failure.as_deref(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
        );
        assert_eq!(outcome.files_restored, Some(3));
    }
}
