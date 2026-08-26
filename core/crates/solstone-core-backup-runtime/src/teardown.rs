// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backup teardown. Remote work is deliberately committed before local cleanup.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use solstone_core_backup::{
    assemble_backend_env, clear_backup_config, delete_hosted_binding, get_backup_config,
    get_destination, get_keys, load_hosted_binding,
};

use crate::engine::BackupServices;
use crate::hosted_runtime::fetch_hosted_credentials;
use crate::runner::{reason_for_returncode, run_restic};
use crate::s3_wipe::{S3Credentials, wipe_prefix};

pub const TEARDOWN_TIMEOUT_SECONDS: u64 = 2 * 60 * 60;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeardownResult {
    pub status: String,
    pub reason_code: Option<String>,
}
fn failure(reason: impl Into<String>) -> TeardownResult {
    TeardownResult {
        status: "error".into(),
        reason_code: Some(reason.into()),
    }
}
fn backend(
    destination: &solstone_core_backup::Destination,
) -> Option<BTreeMap<String, Option<String>>> {
    assemble_backend_env(destination).ok().map(|env| {
        env.into_iter()
            .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
            .collect()
    })
}
fn snapshot_ids(parsed: Option<&Value>) -> Option<Vec<String>> {
    parsed?
        .as_array()?
        .iter()
        .map(|item| {
            item.get("id")?
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

/// Turn off and delete the configured backup. Confirmation belongs to the CLI.
pub fn teardown_backup(journal: &Path, services: &BackupServices<'_>) -> TeardownResult {
    let config = match get_backup_config(journal) {
        Ok(config) => config,
        Err(_) => return failure("failed"),
    };
    if config.get("mode") == Some(&Value::String("operated".into())) {
        let Some(binding) = load_hosted_binding(journal) else {
            return TeardownResult {
                status: "skipped".into(),
                reason_code: None,
            };
        };
        let credentials = match fetch_hosted_credentials(
            services.http,
            &binding,
            "maintenance",
            services.version,
        ) {
            Ok(credentials) => credentials,
            Err(error) if error.reason_code == "binding_superseded" => {
                if clear_backup_config(journal).is_err() {
                    return failure("failed");
                }
                if delete_hosted_binding(journal).is_err() {
                    return failure("failed");
                }
                return TeardownResult {
                    status: "cleared_superseded".into(),
                    reason_code: Some("binding_superseded".into()),
                };
            }
            Err(error) => return failure(error.reason_code),
        };
        let result = wipe_prefix(
            services.http,
            &S3Credentials {
                endpoint: credentials.endpoint,
                access_key_id: credentials.access_key_id,
                secret_access_key: credentials.secret_access_key,
                session_token: credentials.session_token,
                region: "auto".into(),
            },
            &binding.bucket,
            &binding.prefix,
            &amz_date(services.clock.now_unix()),
        );
        if result.status != "ok" {
            return failure(result.reason_code.unwrap_or_else(|| "failed".into()));
        }
        if clear_backup_config(journal).is_err() {
            return failure("failed");
        }
        if delete_hosted_binding(journal).is_err() {
            return failure("failed");
        }
        return TeardownResult {
            status: "ok".into(),
            reason_code: None,
        };
    }
    let (Some(keys), Some(destination)) = (
        get_keys(journal).ok().flatten(),
        get_destination(journal).ok().flatten(),
    ) else {
        return TeardownResult {
            status: "skipped".into(),
            reason_code: None,
        };
    };
    let Some(env) = backend(&destination) else {
        return failure("failed");
    };
    let restic_path = match services.restic_path() {
        Ok(path) => path,
        Err(reason) => return failure(reason),
    };
    let snapshot = match run_restic(
        services.runner,
        &["snapshots".into()],
        &destination.repository,
        &keys.daily_key,
        restic_path,
        Some(&env),
        true,
        None,
        Some(Duration::from_secs(TEARDOWN_TIMEOUT_SECONDS)),
        &[],
    ) {
        Ok(result) => result,
        Err(_) => return failure("failed"),
    };
    if snapshot.returncode != 0 {
        return failure(reason_for_returncode(snapshot.returncode));
    }
    let Some(ids) = snapshot_ids(snapshot.json.as_ref()) else {
        return failure("failed");
    };
    if !ids.is_empty() {
        let mut args = vec!["forget".into()];
        args.extend(ids);
        args.push("--prune".into());
        let forget = match run_restic(
            services.runner,
            &args,
            &destination.repository,
            &keys.daily_key,
            restic_path,
            Some(&env),
            false,
            None,
            Some(Duration::from_secs(TEARDOWN_TIMEOUT_SECONDS)),
            &[],
        ) {
            Ok(result) => result,
            Err(_) => return failure("failed"),
        };
        if forget.returncode != 0 {
            return failure(reason_for_returncode(forget.returncode));
        }
    }
    if clear_backup_config(journal).is_err() {
        return failure("failed");
    };
    TeardownResult {
        status: "ok".into(),
        reason_code: None,
    }
}

fn amz_date(seconds: i64) -> String {
    // UTC civil conversion, avoiding an extra runtime dependency.
    let days = seconds.div_euclid(86_400);
    let seconds = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Clock, JournalMaintenance};
    use crate::hosted_runtime::{HttpError, HttpRequest, HttpResponse};
    use crate::runner::{ToolOutput, ToolRequest, ToolRunner};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    struct Script {
        outputs: RefCell<VecDeque<ToolOutput>>,
        commands: RefCell<Vec<Vec<String>>>,
    }
    impl ToolRunner for Script {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.commands.borrow_mut().push(
                request
                    .argv
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect(),
            );
            Ok(self.outputs.borrow_mut().pop_front().expect("output"))
        }
    }
    struct Http;
    impl crate::hosted_runtime::HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("BYO teardown does not use broker")
        }
    }
    struct BrokerFailure {
        calls: RefCell<u64>,
    }
    impl crate::hosted_runtime::HttpTransport for BrokerFailure {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            *self.calls.borrow_mut() += 1;
            Err(HttpError::Timeout)
        }
    }
    struct BrokerResponse {
        response: Result<HttpResponse, HttpError>,
        calls: RefCell<u64>,
    }
    impl crate::hosted_runtime::HttpTransport for BrokerResponse {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            *self.calls.borrow_mut() += 1;
            self.response.clone()
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
    struct Maintenance;
    impl JournalMaintenance for Maintenance {
        fn rebuild_body_history(
            &self,
            _: &Path,
        ) -> Result<(), crate::engine::JournalMaintenanceError> {
            Ok(())
        }
        fn full_scan(&self, _: &Path) -> Result<(), crate::engine::JournalMaintenanceError> {
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
    fn broker_failure_services<'a>(
        runner: &'a Script,
        http: &'a BrokerFailure,
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
    fn broker_response_services<'a>(
        runner: &'a Script,
        http: &'a BrokerResponse,
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
    fn configured_byo() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        let destination = solstone_core_backup::Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: serde_json::json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .unwrap()
                .clone(),
        };
        solstone_core_backup::set_destination(journal.path(), &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        journal
    }
    #[test]
    fn refuses_malformed_snapshot_records() {
        assert_eq!(
            snapshot_ids(Some(&serde_json::json!([{"id":"ok"},{"id":""}]))),
            None
        );
        assert_eq!(snapshot_ids(Some(&serde_json::json!([]))), Some(vec![]));
    }
    #[test]
    fn formats_epoch_as_utc() {
        assert_eq!(amz_date(0), "19700101T000000Z");
    }
    #[test]
    fn absent_restic_path_reports_existing_unavailable_reason_for_byo_teardown() {
        let journal = configured_byo();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.restic_path = None;

        let result = teardown_backup(journal.path(), &services);

        assert_eq!(result.status, "error");
        assert_eq!(result.reason_code.as_deref(), Some("restic_unavailable"));
        assert!(runner.commands.borrow().is_empty());
    }
    #[test]
    fn absent_restic_path_does_not_change_teardown_skip_path() {
        let journal = tempfile::tempdir().unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.restic_path = None;

        let result = teardown_backup(journal.path(), &services);

        assert_eq!(result.status, "skipped");
        assert_eq!(result.reason_code, None);
        assert!(runner.commands.borrow().is_empty());
    }
    #[test]
    fn byo_forgets_then_clears_without_deleting_hosted_binding() {
        let journal = tempfile::tempdir().unwrap();
        let destination = solstone_core_backup::Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: serde_json::json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .unwrap()
                .clone(),
        };
        solstone_core_backup::set_destination(journal.path(), &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        let binding = solstone_core_backup::HostedBinding {
            broker_endpoint: "https://broker".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            broker_token: "token".into(),
        };
        solstone_core_backup::save_hosted_binding(journal.path(), &binding).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(0, "[{\"id\":\"snap\"}]"),
                output(0, ""),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        assert_eq!(
            teardown_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance)
            )
            .status,
            "ok"
        );
        assert_eq!(
            runner.commands.borrow()[1],
            vec!["forget", "snap", "--prune"]
        );
        assert_eq!(
            solstone_core_backup::load_hosted_binding(journal.path()),
            Some(binding)
        );
        assert!(
            !solstone_core_backup::get_backup_config(journal.path()).unwrap()["enabled"]
                .as_bool()
                .unwrap()
        );
    }
    #[test]
    fn byo_malformed_listing_never_attempts_forget_and_keeps_config() {
        let journal = configured_byo();
        let before = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([output(
                0,
                "[{\"id\":\"ok\"},{\"id\":\"\"}]",
            )])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        assert_eq!(
            teardown_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance)
            )
            .status,
            "error"
        );
        assert_eq!(runner.commands.borrow().len(), 1);
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            before
        );
    }
    #[test]
    fn byo_listing_failure_keeps_config() {
        let journal = configured_byo();
        let before = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([output(12, "")])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        assert_eq!(
            teardown_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance)
            )
            .status,
            "error"
        );
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            before
        );
    }
    #[test]
    fn byo_forget_failure_keeps_local_config_after_remote_work_begins() {
        let journal = configured_byo();
        let before = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(0, "[{\"id\":\"snap\"}]"),
                output(12, ""),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;

        assert_eq!(
            teardown_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance)
            )
            .status,
            "error"
        );
        assert_eq!(
            runner.commands.borrow()[1],
            vec!["forget", "snap", "--prune"]
        );
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            before
        );
    }
    #[cfg(unix)]
    #[test]
    fn byo_clear_failure_keeps_config_after_successful_forget() {
        let journal = configured_byo();
        let config_dir = journal.path().join("config");
        let before = std::fs::read(config_dir.join("journal.json")).unwrap();
        std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(0, "[{\"id\":\"snap\"}]"),
                output(0, ""),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = TestClock;
        let maintenance = Maintenance;
        let result = teardown_backup(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );
        std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(result.status, "error");
        assert_eq!(
            runner.commands.borrow()[1],
            vec!["forget", "snap", "--prune"]
        );
        assert_eq!(
            std::fs::read(config_dir.join("journal.json")).unwrap(),
            before
        );
    }
    #[test]
    fn operated_broker_failure_keeps_binding_config_prefix_and_never_runs_restic() {
        let journal = tempfile::tempdir().unwrap();
        solstone_core_backup::set_mode(journal.path(), "operated").unwrap();
        let binding = solstone_core_backup::HostedBinding {
            broker_endpoint: "https://broker".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "a/b/".into(),
            broker_token: "token".into(),
        };
        solstone_core_backup::save_hosted_binding(journal.path(), &binding).unwrap();
        let config = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        let binding_bytes =
            std::fs::read(journal.path().join("backup/hosted/binding.json")).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let http = BrokerFailure {
            calls: RefCell::new(0),
        };
        let clock = TestClock;
        let maintenance = Maintenance;

        assert_eq!(
            teardown_backup(
                journal.path(),
                &broker_failure_services(&runner, &http, &clock, &maintenance)
            )
            .reason_code,
            Some("broker_unreachable".into())
        );
        assert_eq!(*http.calls.borrow(), 1);
        assert!(runner.commands.borrow().is_empty());
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            config
        );
        assert_eq!(
            std::fs::read(journal.path().join("backup/hosted/binding.json")).unwrap(),
            binding_bytes
        );
    }
    #[test]
    fn operated_superseded_binding_clears_local_state_without_wiping() {
        let journal = tempfile::tempdir().unwrap();
        solstone_core_backup::set_mode(journal.path(), "operated").unwrap();
        let binding = solstone_core_backup::HostedBinding {
            broker_endpoint: "https://broker".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "a/b/".into(),
            broker_token: "token".into(),
        };
        solstone_core_backup::save_hosted_binding(journal.path(), &binding).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let http = BrokerResponse {
            response: Ok(HttpResponse {
                status: 401,
                headers: vec![],
                body: br#"{"error":"binding_superseded"}"#.to_vec(),
            }),
            calls: RefCell::new(0),
        };
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = teardown_backup(
            journal.path(),
            &broker_response_services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "cleared_superseded");
        assert_eq!(result.reason_code, Some("binding_superseded".into()));
        assert_eq!(*http.calls.borrow(), 1);
        assert!(runner.commands.borrow().is_empty());
        let config = solstone_core_backup::get_backup_config(journal.path()).unwrap();
        assert_eq!(config["enabled"], Value::Bool(false));
        assert_eq!(config["mode"], Value::String("byo".into()));
        assert_eq!(
            solstone_core_backup::load_hosted_binding(journal.path()),
            None
        );
    }
    #[test]
    fn operated_invalid_binding_keeps_binding_config_prefix_and_never_runs_restic() {
        let journal = tempfile::tempdir().unwrap();
        solstone_core_backup::set_mode(journal.path(), "operated").unwrap();
        let binding = solstone_core_backup::HostedBinding {
            broker_endpoint: "https://broker".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "a/b/".into(),
            broker_token: "token".into(),
        };
        solstone_core_backup::save_hosted_binding(journal.path(), &binding).unwrap();
        let config = std::fs::read(journal.path().join("config/journal.json")).unwrap();
        let binding_bytes =
            std::fs::read(journal.path().join("backup/hosted/binding.json")).unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let http = BrokerResponse {
            response: Ok(HttpResponse {
                status: 401,
                headers: vec![],
                body: br#"{"error":"invalid_token"}"#.to_vec(),
            }),
            calls: RefCell::new(0),
        };
        let clock = TestClock;
        let maintenance = Maintenance;

        let result = teardown_backup(
            journal.path(),
            &broker_response_services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.reason_code, Some("binding_invalid".into()));
        assert_eq!(*http.calls.borrow(), 1);
        assert!(runner.commands.borrow().is_empty());
        assert_eq!(
            std::fs::read(journal.path().join("config/journal.json")).unwrap(),
            config
        );
        assert_eq!(
            std::fs::read(journal.path().join("backup/hosted/binding.json")).unwrap(),
            binding_bytes
        );
    }
}
