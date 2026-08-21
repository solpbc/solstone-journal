// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whole-journal restore with the intentionally non-transactional publication order.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use solstone_core_backup::{
    Destination, assemble_backend_env, get_backup_config, parse_recovery_key, set_destination,
    set_recovery_key, set_recovery_key_confirmed,
};

use crate::engine::BackupServices;
use crate::runner::{reason_for_returncode, run_restic, select_summary};

pub const RESTORE_LIST_TIMEOUT_SECONDS: u64 = 5 * 60;
pub const RESTORE_TIMEOUT_SECONDS: u64 = 48 * 60 * 60;
pub const RESTORE_CHECK_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreResult {
    pub status: String,
    pub reason_code: Option<String>,
    pub integrity_ok: bool,
    pub resumable: bool,
    pub bytes_restored: Option<u64>,
}
fn failure(reason: impl Into<String>) -> RestoreResult {
    RestoreResult {
        status: "error".into(),
        reason_code: Some(reason.into()),
        integrity_ok: false,
        resumable: false,
        bytes_restored: None,
    }
}
fn backend(destination: &Destination) -> Result<BTreeMap<String, Option<String>>, RestoreResult> {
    assemble_backend_env(destination)
        .map(|env| {
            env.into_iter()
                .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
                .collect()
        })
        .map_err(|_| failure("failed"))
}
fn original_path(parsed: Option<&Value>) -> Option<&str> {
    parsed?
        .as_array()?
        .first()?
        .get("paths")?
        .as_array()?
        .first()?
        .as_str()
        .filter(|path| !path.is_empty())
}
fn restored_bytes(parsed: Option<&Value>) -> Option<u64> {
    select_summary(parsed?)?.get("bytes_restored")?.as_u64()
}
fn restic(
    services: &BackupServices<'_>,
    args: Vec<String>,
    destination: &Destination,
    key: &str,
    env: &BTreeMap<String, Option<String>>,
    json: bool,
    timeout: u64,
) -> Result<crate::runner::ResticResult, RestoreResult> {
    let restic_path = services.restic_path().map_err(failure)?;
    run_restic(
        services.runner,
        &args,
        &destination.repository,
        key,
        restic_path,
        Some(env),
        json,
        None,
        Some(Duration::from_secs(timeout)),
        &[],
    )
    .map_err(|_| failure("failed"))
}

/// Restore a BYO repository. The three config setters are purposefully independent.
pub fn restore_journal(
    journal: &Path,
    services: &BackupServices<'_>,
    destination: Destination,
    entered_recovery_key: &str,
) -> RestoreResult {
    let canonical = match parse_recovery_key(entered_recovery_key) {
        Ok(key) => key,
        Err(_) => return failure("invalid_key"),
    };
    let env = match backend(&destination) {
        Ok(env) => env,
        Err(result) => return result,
    };
    let snapshots = match restic(
        services,
        vec!["snapshots".into(), "latest".into()],
        &destination,
        &canonical,
        &env,
        true,
        RESTORE_LIST_TIMEOUT_SECONDS,
    ) {
        Ok(output) => output,
        Err(result) => return result,
    };
    if snapshots.returncode != 0 {
        return failure(reason_for_returncode(snapshots.returncode));
    }
    let Some(path) = original_path(snapshots.json.as_ref()) else {
        return failure("failed");
    };
    let restored = match restic(
        services,
        vec![
            "restore".into(),
            format!("latest:{path}"),
            "--target".into(),
            journal.display().to_string(),
        ],
        &destination,
        &canonical,
        &env,
        true,
        RESTORE_TIMEOUT_SECONDS,
    ) {
        Ok(output) => output,
        Err(result) => return result,
    };
    if restored.returncode != 0 {
        return failure(reason_for_returncode(restored.returncode));
    }
    let bytes = restored_bytes(restored.json.as_ref());
    let check = match restic(
        services,
        vec!["check".into()],
        &destination,
        &canonical,
        &env,
        false,
        RESTORE_CHECK_TIMEOUT_SECONDS,
    ) {
        Ok(output) => output,
        Err(result) => return result,
    };
    let (status, reason, integrity_ok) = if check.returncode == 0 {
        ("ok", None, true)
    } else if matches!(check.returncode, 11 | 124) {
        ("degraded", Some("integrity_unverified".into()), false)
    } else {
        ("degraded", Some("integrity_failed".into()), false)
    };
    if services
        .journal_maintenance
        .rebuild_body_history(journal)
        .is_err()
    {
        return failure("body_rebuild_failed");
    }
    // Preserve the Python failure boundaries: each publication is a separate mutation.
    if set_destination(journal, &destination).is_err() {
        return failure("failed");
    }
    if set_recovery_key(journal, &canonical).is_err() {
        return failure("failed");
    }
    if set_recovery_key_confirmed(journal, true).is_err() {
        return failure("failed");
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
        return failure("failed");
    }
    RestoreResult {
        status: status.into(),
        reason_code: reason,
        integrity_ok,
        resumable,
        bytes_restored: bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Clock, JournalMaintenance};
    use crate::hosted_runtime::{HttpError, HttpRequest, HttpResponse, HttpTransport};
    use crate::runner::{ToolOutput, ToolRequest, ToolRunner};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;

    struct Script(RefCell<VecDeque<ToolOutput>>);
    impl ToolRunner for Script {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Ok(self.0.borrow_mut().pop_front().expect("output"))
        }
    }
    struct Http;
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("restore does not use broker")
        }
    }
    struct TestClock;
    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            0
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    struct Maintenance(RefCell<Vec<&'static str>>);
    impl JournalMaintenance for Maintenance {
        fn rebuild_body_history(
            &self,
            _: &Path,
        ) -> Result<(), crate::engine::JournalMaintenanceError> {
            self.0.borrow_mut().push("rebuild");
            Ok(())
        }
        fn full_scan(&self, _: &Path) -> Result<(), crate::engine::JournalMaintenanceError> {
            self.0.borrow_mut().push("scan");
            Ok(())
        }
    }
    fn output(code: i32, stdout: &str) -> ToolOutput {
        ToolOutput {
            returncode: code,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
        }
    }
    fn services<'a>(
        runner: &'a Script,
        http: &'a Http,
        clock: &'a TestClock,
        maintenance: &'a dyn JournalMaintenance,
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
    fn destination() -> Destination {
        Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: serde_json::json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .unwrap()
                .clone(),
        }
    }
    #[test]
    fn parses_only_a_summary_integer_byte_count() {
        assert_eq!(
            restored_bytes(Some(
                &serde_json::json!([{"message_type":"summary","bytes_restored":12}])
            )),
            Some(12)
        );
        assert_eq!(
            restored_bytes(Some(
                &serde_json::json!({"message_type":"summary","bytes_restored":12.0})
            )),
            None
        );
    }
    #[test]
    fn selects_first_snapshot_path() {
        assert_eq!(
            original_path(Some(&serde_json::json!([{ "paths":["/journal"] }]))),
            Some("/journal")
        );
        assert_eq!(original_path(Some(&serde_json::json!([]))), None);
    }
    #[test]
    fn absent_restic_path_returns_existing_unavailable_reason_without_runner_call() {
        let journal = tempfile::tempdir().unwrap();
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        let runner = Script(RefCell::new(VecDeque::new()));
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance(RefCell::new(vec![]));
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.restic_path = None;

        let result = restore_journal(
            journal.path(),
            &services,
            destination(),
            &stored.recovery_key,
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.reason_code.as_deref(), Some("restic_unavailable"));
        assert!(runner.0.borrow().is_empty());
    }
    #[test]
    fn integrity_unverified_still_rebuilds_persists_and_scans_without_offload_restore() {
        let journal = tempfile::tempdir().unwrap();
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        let prior_last_restore =
            solstone_core_backup::get_backup_config(journal.path()).unwrap()["last_restore"]
                .clone();
        let runner = Script(RefCell::new(VecDeque::from([
            output(0, "[{\"paths\":[\"/original\"]}]"),
            output(0, "[{\"message_type\":\"summary\",\"bytes_restored\":12}]"),
            output(11, ""),
        ])));
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance(RefCell::new(vec![]));
        let result = restore_journal(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            destination(),
            &stored.recovery_key,
        );
        assert_eq!(result.status, "degraded");
        assert_eq!(result.reason_code, Some("integrity_unverified".into()));
        assert_eq!(result.bytes_restored, Some(12));
        assert_eq!(*maintenance.0.borrow(), vec!["rebuild", "scan"]);
        let config = solstone_core_backup::get_backup_config(journal.path()).unwrap();
        assert_eq!(config["destination"]["repository"], "repo");
        assert_eq!(config["confirmed_recovery_key"], true);
        assert_eq!(config["last_restore"], prior_last_restore);
    }

    struct FailingRebuild(RefCell<Vec<&'static str>>);
    impl JournalMaintenance for FailingRebuild {
        fn rebuild_body_history(
            &self,
            _: &Path,
        ) -> Result<(), crate::engine::JournalMaintenanceError> {
            self.0.borrow_mut().push("rebuild");
            Err(crate::engine::JournalMaintenanceError)
        }
        fn full_scan(&self, _: &Path) -> Result<(), crate::engine::JournalMaintenanceError> {
            self.0.borrow_mut().push("scan");
            Ok(())
        }
    }

    #[test]
    fn rebuild_failure_short_circuits_before_any_publication() {
        let journal = tempfile::tempdir().unwrap();
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        let before = solstone_core_backup::get_backup_config(journal.path()).unwrap();
        let runner = Script(RefCell::new(VecDeque::from([
            output(0, "[{\"paths\":[\"/original\"]}]"),
            output(0, "[{\"message_type\":\"summary\",\"bytes_restored\":12}]"),
            output(0, ""),
        ])));
        let http = Http;
        let clock = TestClock;
        let maintenance = FailingRebuild(RefCell::new(vec![]));
        let result = restore_journal(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            destination(),
            &stored.recovery_key,
        );
        assert_eq!(result.status, "error");
        assert_eq!(result.reason_code, Some("body_rebuild_failed".into()));
        assert!(!result.integrity_ok);
        assert_eq!(result.bytes_restored, None);
        assert_eq!(*maintenance.0.borrow(), vec!["rebuild"]);
        let after = solstone_core_backup::get_backup_config(journal.path()).unwrap();
        assert_eq!(after.get("destination"), before.get("destination"));
        assert_eq!(
            after.get("confirmed_recovery_key"),
            before.get("confirmed_recovery_key")
        );
    }
}
