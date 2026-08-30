// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process implementations of the backup-owned maintenance routines.

use std::path::Path;

use solstone_core_backup_runtime::{
    BackupResult, BackupServices, PruneResult, VerificationResult, prepare, run_prune,
    run_verification,
};
use solstone_core_offload::{format_offload_result, run_offload};

use crate::CliRun;

pub(crate) fn run(
    id: &str,
    args: &[String],
    journal: &Path,
    services: &BackupServices<'_>,
) -> CliRun {
    match id {
        "backup:run" if args.is_empty() => {
            backup_run_result(match prepare(journal, services.clock) {
                Ok(capability) => capability.execute(services),
                Err(result) => result,
            })
        }
        "backup:prune" if args.is_empty() => backup_prune(journal, services),
        "backup:verify" if args.is_empty() => backup_verify(journal, services),
        "backup:offload" => backup_offload(args, journal, services),
        _ => usage_error(id, args),
    }
}

pub(crate) fn backup_run_result(result: BackupResult) -> CliRun {
    let line = match result.status.as_str() {
        "ok" => format!(
            "backup: ok snapshot_id={}",
            result.snapshot_id.as_deref().unwrap_or("None")
        ),
        "skipped" => "backup: skipped".to_owned(),
        _ => format!(
            "backup: error reason={}",
            result.error_reason.as_deref().unwrap_or("None")
        ),
    };
    success(line)
}

fn backup_prune(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    backup_prune_result(run_prune(journal, services))
}

fn backup_prune_result(result: PruneResult) -> CliRun {
    let line = match result.status.as_str() {
        "ok" => "backup prune: ok".to_owned(),
        "skipped" => "backup prune: skipped".to_owned(),
        _ => format!(
            "backup prune: error reason={}",
            result.error_reason.as_deref().unwrap_or("None")
        ),
    };
    success(line)
}

fn backup_verify(journal: &Path, services: &BackupServices<'_>) -> CliRun {
    backup_verify_result(run_verification(journal, services, services.clock))
}

fn backup_verify_result(result: VerificationResult) -> CliRun {
    let line = match result.status.as_str() {
        "ok" => format!(
            "backup verify: ok subset={}",
            result.checked_subset.as_deref().unwrap_or("None")
        ),
        "skipped" => "backup verify: skipped".to_owned(),
        _ => format!(
            "backup verify: error reason={}",
            result.reason.as_deref().unwrap_or("None")
        ),
    };
    success(line)
}

fn backup_offload(args: &[String], journal: &Path, services: &BackupServices<'_>) -> CliRun {
    if args.iter().any(|argument| argument != "--dry-run") {
        return usage_error("backup:offload", args);
    }
    let dry_run = args.iter().any(|argument| argument == "--dry-run");
    success(format_offload_result(&run_offload(
        journal, services, dry_run,
    )))
}

fn success(line: String) -> CliRun {
    CliRun {
        stdout: format!("{line}\n"),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn usage_error(id: &str, args: &[String]) -> CliRun {
    let options = if id == "backup:offload" {
        " [-h] [--dry-run]"
    } else {
        " [-h]"
    };
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "usage: journal maintenance run {id}{options}\njournal maintenance run {id}: error: unrecognized arguments: {}\n",
            args.join(" ")
        ),
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    use std::path::Path;

    use serde_json::{Map as JsonMap, Value};
    use solstone_core_backup::{
        Destination, generate_and_store_keys, set_destination, set_enabled, set_offload,
    };
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    use solstone_core_backup_runtime::{
        BackupServices, Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };

    use super::run;

    struct FixtureRunner(RefCell<VecDeque<ToolOutput>>);

    impl ToolRunner for FixtureRunner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Ok(self.0.borrow_mut().pop_front().expect("fixture command"))
        }
    }

    struct UnusedHttp;

    impl HttpTransport for UnusedHttp {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("BYO fixtures must not use the broker")
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_unix(&self) -> i64 {
            0
        }

        fn iso_week(&self) -> u8 {
            2
        }
    }

    struct UnusedRestoreHooks;

    impl JournalMaintenance for UnusedRestoreHooks {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("backup routines do not restore")
        }

        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("backup routines do not restore")
        }
    }

    fn services<'a>(
        runner: &'a FixtureRunner,
        http: &'a UnusedHttp,
        clock: &'a FixedClock,
        hooks: &'a UnusedRestoreHooks,
    ) -> BackupServices<'a> {
        BackupServices {
            runner,
            http,
            clock,
            restic_path: Some(Path::new("/fixture/bin/restic")),
            rclone_path: None,
            version: "test",
            journal_maintenance: hooks,
        }
    }

    fn configured_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().expect("journal");
        set_destination(
            journal.path(),
            &Destination {
                repository: "s3:bucket/prefix".to_owned(),
                backend: "s3".to_owned(),
                credentials: JsonMap::from_iter([
                    (
                        "access_key_id".to_owned(),
                        Value::String("access".to_owned()),
                    ),
                    (
                        "secret_access_key".to_owned(),
                        Value::String("secret".to_owned()),
                    ),
                ]),
            },
        )
        .expect("destination");
        generate_and_store_keys(journal.path()).expect("keys");
        set_enabled(journal.path(), true).expect("enabled");
        journal
    }

    fn output(returncode: i32, stdout: &[u8]) -> ToolOutput {
        ToolOutput {
            returncode,
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn backup_prune_and_verify_use_injected_services_for_success_and_failure() {
        let journal = configured_journal();
        let http = UnusedHttp;
        let clock = FixedClock;
        let hooks = UnusedRestoreHooks;
        let prune_runner = FixtureRunner(RefCell::new(VecDeque::from([
            output(0, b""),
            output(0, b""),
        ])));
        assert_eq!(
            run(
                "backup:prune",
                &[],
                journal.path(),
                &services(&prune_runner, &http, &clock, &hooks),
            )
            .stdout,
            "backup prune: ok\n"
        );
        let prune_failed_runner = FixtureRunner(RefCell::new(VecDeque::from([
            output(0, b""),
            output(1, b""),
        ])));
        assert_eq!(
            run(
                "backup:prune",
                &[],
                journal.path(),
                &services(&prune_failed_runner, &http, &clock, &hooks),
            )
            .stdout,
            "backup prune: error reason=failed\n"
        );

        let verify_runner = FixtureRunner(RefCell::new(VecDeque::from([output(0, b"")])));
        assert_eq!(
            run(
                "backup:verify",
                &[],
                journal.path(),
                &services(&verify_runner, &http, &clock, &hooks),
            )
            .stdout,
            "backup verify: ok subset=2/52\n"
        );
        let verify_failed_runner = FixtureRunner(RefCell::new(VecDeque::from([output(1, b"")])));
        assert_eq!(
            run(
                "backup:verify",
                &[],
                journal.path(),
                &services(&verify_failed_runner, &http, &clock, &hooks),
            )
            .stdout,
            "backup verify: error reason=integrity_failed\n"
        );
    }

    #[test]
    fn backup_run_uses_admission_result_and_rejects_arguments() {
        let journal = configured_journal();
        let http = UnusedHttp;
        let clock = FixedClock;
        let hooks = UnusedRestoreHooks;
        let success_runner = FixtureRunner(RefCell::new(VecDeque::from([
            output(0, b""),
            output(
                0,
                b"{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}\n",
            ),
        ])));
        let success = run(
            "backup:run",
            &[],
            journal.path(),
            &services(&success_runner, &http, &clock, &hooks),
        );
        assert_eq!(success.stdout, "backup: ok snapshot_id=snap\n");
        assert_eq!(success.exit_code, 0);

        let skipped_journal = tempfile::tempdir().expect("unconfigured journal");
        let skipped_runner = FixtureRunner(RefCell::new(VecDeque::new()));
        let skipped_services = services(&skipped_runner, &http, &clock, &hooks);
        let skipped = run("backup:run", &[], skipped_journal.path(), &skipped_services);
        assert_eq!(skipped.stdout, "backup: skipped\n");
        assert_eq!(skipped.exit_code, 0);

        let invalid = run(
            "backup:run",
            &["unexpected".to_owned()],
            skipped_journal.path(),
            &skipped_services,
        );
        assert_eq!(invalid.exit_code, 2);
        assert!(
            invalid
                .stderr
                .starts_with("usage: journal maintenance run backup:run [-h]")
        );
    }

    #[test]
    fn backup_offload_allows_only_dry_run_and_uses_injected_services() {
        let journal = configured_journal();
        set_offload(
            journal.path(),
            &JsonMap::from_iter([
                ("enabled".to_owned(), Value::Bool(true)),
                ("budget_bytes".to_owned(), Value::Null),
                ("floor_bytes".to_owned(), Value::Null),
            ]),
        )
        .expect("offload");
        let runner = FixtureRunner(RefCell::new(VecDeque::new()));
        let http = UnusedHttp;
        let clock = FixedClock;
        let hooks = UnusedRestoreHooks;
        let service = services(&runner, &http, &clock, &hooks);
        let dry = run(
            "backup:offload",
            &["--dry-run".to_owned()],
            journal.path(),
            &service,
        );
        assert_eq!(dry.exit_code, 0);
        assert_eq!(
            dry.stdout,
            "backup offload: stalled reason=backup_failing dry_run=true\n"
        );

        let invalid = run(
            "backup:offload",
            &["--other".to_owned()],
            journal.path(),
            &service,
        );
        assert_eq!(invalid.exit_code, 2);
        assert!(
            invalid
                .stderr
                .starts_with("usage: journal maintenance run backup:offload")
        );
    }
}
