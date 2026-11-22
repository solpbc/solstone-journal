// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process implementations of the backup-owned maintenance routines.

use crate::copy::Output;
use std::path::Path;

use solstone_core_backup_runtime::{
    BackupResult, BackupServices, PruneResult, VerificationResult, prepare, run_prune,
    run_verification,
};

use crate::CliRun;

pub(crate) fn run(
    copy: Output<'_>,
    id: &str,
    args: &[String],
    journal: &Path,
    services: &BackupServices<'_>,
) -> CliRun {
    match id {
        "backup:run" if args.is_empty() => backup_run_result(
            copy,
            match prepare(journal, services.clock) {
                Ok(capability) => capability.execute(services),
                Err(result) => result,
            },
        ),
        "backup:prune" if args.is_empty() => backup_prune(copy, journal, services),
        "backup:verify" if args.is_empty() => backup_verify(copy, journal, services),
        "backup:erase" if args.is_empty() => backup_erase(copy, journal, services),
        "backup:offload" => backup_offload(copy, args, journal, services),
        _ => usage_error(copy, id, args),
    }
}

pub(crate) fn backup_run_result(copy: Output<'_>, result: BackupResult) -> CliRun {
    let line = match result.status.as_str() {
        "ok" => copy.message(
            "journal.cli.maintenance.backup.ok",
            [("snapshot", &result.snapshot_id.as_deref().unwrap_or("None"))],
        ),
        "skipped" if result.detail.as_deref() == Some("paused") => {
            copy.message("journal.cli.maintenance.backup.paused", [])
        }
        "skipped" => copy.message("journal.cli.maintenance.backup.skipped", []),
        // ⛔ Not a retry candidate: the broker has told us, permanently, that
        // this device's binding was replaced by a newer one. Local settings
        // are already cleared by the time this line prints (engine.rs), so
        // the schedule stops asking a question with a known answer.
        "cleared_superseded" => copy.message("journal.cli.maintenance.backup.superseded", []),
        _ => {
            let reason = result.error_reason.as_deref().unwrap_or("None");
            if let Some(detail) = result.detail.as_deref() {
                copy.message(
                    "journal.cli.maintenance.backup.error.detail",
                    [("reason", &reason), ("detail", &detail)],
                )
            } else {
                copy.message(
                    "journal.cli.maintenance.backup.error",
                    [("reason", &reason)],
                )
            }
        }
    };
    routine_result(line, &result.status)
}

fn backup_prune(copy: Output<'_>, journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let result = run_prune(journal, services);
    backup_prune_result_for(
        copy,
        result,
        solstone_core_backup_runtime::backup_paused(journal),
    )
}

fn backup_erase(copy: Output<'_>, journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let report = solstone_core_offload::backup_erase::run(journal, services);
    let line = match &report.reason {
        Some(reason) => copy.message(
            "journal.cli.maintenance.backup.erase.reason",
            [
                ("state", &report.state),
                ("removed", &report.removed),
                ("shared", &report.shared),
                ("staying", &report.staying),
                ("unreached", &report.unreached),
                ("pending", &report.pending),
                ("reason", &reason),
            ],
        ),
        None => copy.message(
            "journal.cli.maintenance.backup.erase",
            [
                ("state", &report.state),
                ("removed", &report.removed),
                ("shared", &report.shared),
                ("staying", &report.staying),
                ("unreached", &report.unreached),
                ("pending", &report.pending),
            ],
        ),
    };
    let ok = matches!(
        report.state.as_str(),
        "nothing" | "done" | "idle" | "waiting" | "running"
    );
    CliRun {
        stdout: format!("{line}\n"),
        stderr: String::new(),
        exit_code: i32::from(!ok),
    }
}

fn backup_prune_result_for(copy: Output<'_>, result: PruneResult, paused: bool) -> CliRun {
    let line = match result.status.as_str() {
        "ok" => copy.message("journal.cli.maintenance.backup.prune.ok", []),
        "skipped" if paused => copy.message("journal.cli.maintenance.backup.prune.paused", []),
        "skipped" => copy.message("journal.cli.maintenance.backup.prune.skipped", []),
        _ => copy.message(
            "journal.cli.maintenance.backup.prune.error",
            [("reason", &result.error_reason.as_deref().unwrap_or("None"))],
        ),
    };
    routine_result(line, &result.status)
}

fn backup_verify(copy: Output<'_>, journal: &Path, services: &BackupServices<'_>) -> CliRun {
    let result = run_verification(journal, services, services.clock);
    backup_verify_result_for(
        copy,
        result,
        solstone_core_backup_runtime::backup_paused(journal),
    )
}

fn backup_verify_result_for(copy: Output<'_>, result: VerificationResult, paused: bool) -> CliRun {
    let line = match result.status.as_str() {
        "ok" => copy.message(
            "journal.cli.maintenance.backup.verify.ok",
            [(
                "subset",
                &result.checked_subset.as_deref().unwrap_or("None"),
            )],
        ),
        "skipped" if paused => copy.message("journal.cli.maintenance.backup.verify.paused", []),
        "skipped" => copy.message("journal.cli.maintenance.backup.verify.skipped", []),
        _ => copy.message(
            "journal.cli.maintenance.backup.verify.error",
            [("reason", &result.reason.as_deref().unwrap_or("None"))],
        ),
    };
    routine_result(line, &result.status)
}

fn backup_offload(
    copy: Output<'_>,
    args: &[String],
    _journal: &Path,
    _services: &BackupServices<'_>,
) -> CliRun {
    if args.iter().any(|argument| argument != "--dry-run") {
        return usage_error(copy, "backup:offload", args);
    }
    let dry_run = args.iter().any(|argument| argument == "--dry-run");
    CliRun {
        stdout: format!(
            "offload_status=held reason={}\n",
            if dry_run {
                "dry_run_unavailable"
            } else {
                "owned_operation_required"
            }
        ),
        stderr: if dry_run {
            copy.line("journal.cli.backup.offload.preview.held", [])
        } else {
            copy.line("journal.cli.backup.offload.use.installed", [])
        },
        exit_code: 1,
    }
}

fn routine_result(line: String, status: &str) -> CliRun {
    CliRun {
        exit_code: i32::from(!matches!(status, "ok" | "skipped" | "cleared_superseded")),
        ..success(line)
    }
}

fn success(line: String) -> CliRun {
    CliRun {
        stdout: format!("{line}\n"),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn usage_error(copy: Output<'_>, id: &str, args: &[String]) -> CliRun {
    let stderr = if id == "backup:offload" {
        copy.line(
            "journal.cli.maintenance.backup.usage.offload",
            [("id", &id), ("args", &args.join(" "))],
        )
    } else {
        copy.line(
            "journal.cli.maintenance.backup.usage.routine",
            [("id", &id), ("args", &args.join(" "))],
        )
    };
    CliRun {
        stdout: String::new(),
        stderr,
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
        BackupResult, BackupServices, Clock, HttpRequest, HttpResponse, HttpTransport,
        JournalMaintenance, JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };

    use super::{Output, backup_run_result};

    fn run(
        id: &str,
        args: &[String],
        journal: &std::path::Path,
        services: &BackupServices<'_>,
    ) -> crate::CliRun {
        super::run(Output::canonical(), id, args, journal, services)
    }

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
            restic: Some((Path::new("/fixture/bin/restic")).into()),
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
    fn failed_backup_routines_report_failure_to_the_scheduler() {
        let journal = configured_journal();
        let http = UnusedHttp;
        let clock = FixedClock;
        let hooks = UnusedRestoreHooks;
        for id in ["backup:run", "backup:prune", "backup:verify"] {
            for code in [1, 11] {
                let mut outputs = VecDeque::new();
                if id != "backup:verify" {
                    outputs.push_back(output(0, b""));
                }
                outputs.push_back(output(code, b""));
                let runner = FixtureRunner(RefCell::new(outputs));
                let result = run(
                    id,
                    &[],
                    journal.path(),
                    &services(&runner, &http, &clock, &hooks),
                );
                assert_eq!(result.exit_code, 1, "{id}: {}", result.stdout);
                assert!(result.stdout.contains("error reason="));
                assert!(runner.0.borrow().is_empty());
            }
        }
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
                .starts_with("usage: solstone journal maintenance run backup:run [-h]")
        );
    }

    #[test]
    fn cleared_superseded_reports_success_not_a_scheduler_failure() {
        // The broker-to-clear mechanics are proven in
        // solstone-core-backup-runtime::engine; this is the CLI rendering
        // for the outcome that reaches the scheduler.
        let result = backup_run_result(
            Output::canonical(),
            BackupResult {
                status: "cleared_superseded".into(),
                snapshot_id: None,
                error_reason: Some("binding_superseded".into()),
                unreadable: None,
                detail: None,
            },
        );
        assert_eq!(
            result.stdout,
            "backup: claimed by another device; local settings were cleared. run \
             `solstone journal backup enable` to set up a new backup here.\n"
        );
        assert_eq!(
            result.exit_code, 0,
            "a resolved terminal state is not an ongoing scheduler failure"
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
        assert_eq!(dry.exit_code, 1);
        assert_eq!(
            dry.stdout,
            "offload_status=held reason=dry_run_unavailable\n"
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
                .starts_with("usage: solstone journal maintenance run backup:offload")
        );
    }
}
