// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::copy::Output;
use std::path::Path;

use solstone_core_backup_runtime::BackupServices;

use crate::schedule_sync::{render_list_in, render_summary_in, schedules_path, sync_in};
use crate::{CliRun, HealthServices, MaintenanceServices};

pub const USAGE: &str = "usage: solstone journal maintenance <command> [options]\n";

pub(crate) fn run(
    copy: Output<'_>,
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices>,
) -> CliRun {
    let args = normalize_global_flags(args);
    if has_help(&args) {
        return success(usage_for_scope(copy, &args));
    }
    let Some((command, rest)) = args.split_first() else {
        return usage_failure(copy.line("journal.cli.maintenance.missing.root", []));
    };
    match command.as_str() {
        "list" if no_positionals(rest) => list(copy, journal, services),
        "sync" if no_positionals(rest) => sync_schedules(copy, journal, services),
        "run" => run_routine(
            copy,
            rest,
            journal,
            services,
            backup_services,
            health_services,
        ),
        _ => usage_failure(copy.line(
            "journal.cli.maintenance.usage.unknown",
            [("arguments", &args.join(" "))],
        )),
    }
}

fn normalize_global_flags(args: &[String]) -> Vec<String> {
    let mut first_command = 0;
    while first_command < args.len()
        && matches!(
            args[first_command].as_str(),
            "-v" | "--verbose" | "-d" | "--debug"
        )
    {
        first_command += 1;
    }
    args[first_command..].to_vec()
}

fn split_terminator(args: &[String]) -> (&[String], &[String]) {
    match args.iter().position(|argument| argument == "--") {
        Some(index) => (&args[..index], &args[index + 1..]),
        None => (args, &[]),
    }
}

fn no_positionals(args: &[String]) -> bool {
    let (options, positionals) = split_terminator(args);
    options.is_empty() && positionals.is_empty()
}

fn has_help(args: &[String]) -> bool {
    let (options, _) = split_terminator(args);
    options
        .iter()
        .any(|argument| matches!(argument.as_str(), "-h" | "--help"))
}

fn usage_for_scope(copy: Output<'_>, args: &[String]) -> String {
    match args.first().map(String::as_str) {
        Some("list") => copy.line("journal.cli.maintenance.help.list", []),
        Some("sync") => copy.line("journal.cli.maintenance.help.sync", []),
        Some("run") => copy.line("journal.cli.maintenance.help.run", []),
        _ => copy.line("journal.cli.maintenance.help.root", []),
    }
}

fn list(copy: Output<'_>, journal: &Path, services: &MaintenanceServices<'_>) -> CliRun {
    let path = schedules_path(journal);
    match render_list_in(copy, &path, services.routines) {
        Ok(stdout) => success(stdout),
        Err(error) => schedule_error(copy, &path, error),
    }
}

fn sync_schedules(copy: Output<'_>, journal: &Path, services: &MaintenanceServices<'_>) -> CliRun {
    let path = schedules_path(journal);
    match sync_in(copy, &path, services.routines) {
        Ok(summary) => success(render_summary_in(copy, &summary)),
        Err(error) => schedule_error(copy, &path, error),
    }
}

fn run_routine(
    copy: Output<'_>,
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices>,
) -> CliRun {
    let Some((id, routine_args)) = args.split_first() else {
        return usage_failure(copy.line("journal.cli.maintenance.missing.run", []));
    };
    let forwarded = routine_args
        .strip_prefix(&["--".to_owned()])
        .unwrap_or(routine_args);
    let Some(_routine) = services.routines.iter().find(|routine| routine.id == id) else {
        return CliRun {
            stdout: String::new(),
            stderr: copy.line("journal.cli.maintenance.routine.unknown", [("id", &id)]),
            exit_code: 1,
        };
    };
    let _speaker_priority = if id.starts_with("speakers:")
        && solstone_core_entity::faceprints::enabled(journal).unwrap_or(false)
    {
        match solstone_core_segment::face_backfill::speaker_priority_lease(journal, id) {
            Ok(lease) => lease,
            Err(error) => {
                log::warn!(
                    "{}",
                    copy.message("journal.cli.maintenance.face.priority", [("error", &error)])
                );
                None
            }
        }
    } else {
        None
    };
    if matches!(
        id.as_str(),
        "backup:run" | "backup:prune" | "backup:erase" | "backup:verify" | "backup:offload"
    ) {
        return match backup_services {
            Some(backup_services) => {
                crate::bodies::backup::run(copy, id, forwarded, journal, backup_services)
            }
            None => CliRun {
                stdout: String::new(),
                stderr: copy.line("journal.cli.maintenance.backup.unavailable", []),
                exit_code: 1,
            },
        };
    }
    if matches!(id.as_str(), "faces:reconcile" | "faces:backfill") {
        if !forwarded.is_empty() {
            return usage_failure(copy.line("journal.cli.maintenance.usage.face", []));
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        return success("{}\n".into());
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let daily = id == "faces:reconcile";
            #[cfg(windows)]
            let result = crate::bodies::faces::run_with_generation(
                journal,
                daily,
                services.discovery_generation,
            );
            #[cfg(not(windows))]
            let result = if daily {
                crate::bodies::faces::reconcile(journal)
            } else {
                crate::bodies::faces::backfill(journal)
            };
            return match result {
                Ok(stats) => CliRun {
                    stdout: format!(
                        "{}\n",
                        serde_json::to_string(&stats).expect("face statistics serialize")
                    ),
                    stderr: String::new(),
                    exit_code: i32::from(stats.run_failed()),
                },
                Err(error) => CliRun {
                    stdout: String::new(),
                    stderr: copy.line(
                        "journal.cli.maintenance.face.reconciliation",
                        [("error", &error)],
                    ),
                    exit_code: 1,
                },
            };
        }
    }
    if id == "speakers:discover-voices" {
        #[cfg(windows)]
        let Some(generation) = services.discovery_generation else {
            return CliRun {
                stdout: String::new(),
                stderr: copy.line("journal.cli.maintenance.speakers.not.admitted", []),
                exit_code: 1,
            };
        };
        return crate::bodies::speakers::discovery(
            copy,
            forwarded,
            journal,
            #[cfg(windows)]
            generation,
        );
    }
    if id == "speakers:candidate-pair-suggestions" {
        return crate::bodies::speakers::candidate_pairs(copy, forwarded, journal);
    }
    if id == "speakers:name-variants" {
        return crate::bodies::speakers::name_variants(copy, forwarded, journal);
    }
    if id == "speakers:consolidate-pool" {
        return crate::bodies::speakers::consolidate(copy, forwarded, journal);
    }
    if id == "speakers:backfill-pool" {
        return crate::bodies::speakers::backfill_pool(copy, forwarded, journal);
    }
    if id == "speakers:backfill-labels" {
        return crate::bodies::speakers::backfill_labels(copy, forwarded, journal);
    }
    if id == "trash:erase" {
        return crate::bodies::trash::erase_in(copy, forwarded, journal);
    }
    if id == "retention:owner-voice-originals" {
        return match health_services {
            Some(health_services) => crate::bodies::retention::owner_voice_originals(
                copy,
                forwarded,
                journal,
                health_services,
            ),
            None => CliRun {
                stdout: String::new(),
                stderr: copy.line("journal.cli.maintenance.health.unavailable", []),
                exit_code: 1,
            },
        };
    }
    if matches!(id.as_str(), "health:mark-raw" | "health:prune-logs") {
        return match health_services {
            Some(health_services) => crate::bodies::health::run_checked_in(
                copy,
                id,
                forwarded,
                journal,
                health_services,
                &|| Ok(()),
            ),
            None => CliRun {
                stdout: String::new(),
                stderr: copy.line("journal.cli.maintenance.health.unavailable", []),
                exit_code: 1,
            },
        };
    }
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "{}\n",
            copy.counted(
                "journal.cli.maintenance.routine.unimplemented",
                [("count_quantity", forwarded.len() as u64)],
                [("id", &id), ("count", &forwarded.len())]
            )
        ),
        exit_code: 1,
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn schedule_error(copy: Output<'_>, path: &Path, error: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: copy.line(
            "journal.cli.maintenance.schedule.error",
            [("path", &path.display()), ("error", &error)],
        ),
        exit_code: 1,
    }
}

fn usage_failure(stderr: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr,
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::{Output, run};
    use crate::MaintenanceServices;
    use crate::registry::routines;
    use std::path::Path;

    #[test]
    fn unknown_run_id_uses_the_reference_message_without_routine_execution() {
        let args = vec!["run".to_owned(), "other:missing".to_owned()];
        let result = run(
            Output::terminal(Some("en-US")),
            &args,
            Path::new("/unused"),
            &MaintenanceServices::new(routines()),
            None,
            None,
        );
        assert_eq!(result.exit_code, 1);
        assert_eq!(
            result.stderr,
            "Unknown maintenance routine: other:missing. Run `solstone journal maintenance list` to see available routines.\n"
        );
    }

    #[test]
    fn parser_keeps_unknown_flags_in_maintenance_owned_usage() {
        let args = vec!["--nonsense".to_owned()];
        let result = run(
            Output::terminal(Some("en-US")),
            &args,
            Path::new("/unused"),
            &MaintenanceServices::new(routines()),
            None,
            None,
        );
        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .starts_with("usage: solstone journal maintenance")
        );
    }

    #[test]
    fn custom_routine_keeps_its_description_and_supervised_argument_receipt() {
        use crate::registry::{Cadence, RoutineDescriptor};
        let custom = [RoutineDescriptor {
            id: "fixture:run",
            description: "description supplied by the caller",
            cadence: Cadence::Daily,
            max_runtime: None,
        }];
        let services = MaintenanceServices::new(&custom);
        let root = tempfile::tempdir().unwrap();
        let args = ["run", "fixture:run", "--", "owner text"].map(str::to_owned);
        let english = Output::terminal(Some(solstone_core_cli_copy::ENGLISH));
        for (copy, suffix) in [
            (english, "(1 forwarded argument).\n"),
            (Output::canonical(), "(1 forwarded argument(s)).\n"),
        ] {
            let result = run(copy, &args, root.path(), &services, None, None);
            assert_eq!(result.exit_code, 1);
            assert!(result.stderr.ends_with(suffix), "{}", result.stderr);
        }
        let result = run(
            english,
            &["list".into()],
            root.path(),
            &services,
            None,
            None,
        );
        assert!(result.stdout.contains(custom[0].description));
    }

    #[test]
    fn empty_registry_renders_the_reference_empty_message() {
        let args = vec!["list".to_owned()];
        let result = run(
            Output::terminal(Some("en-US")),
            &args,
            Path::new("/unused"),
            &MaintenanceServices::new(&[]),
            None,
            None,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "No maintenance routines found.\n");
    }

    #[test]
    fn missing_command_and_run_id_report_required_argument_instead_of_empty_unrecognized_args() {
        let bare = run(
            Output::terminal(Some("en-US")),
            &[],
            Path::new("/unused"),
            &MaintenanceServices::new(routines()),
            None,
            None,
        );
        assert_eq!(bare.exit_code, 2);
        assert_eq!(bare.stdout, "");
        assert_eq!(
            bare.stderr,
            "usage: solstone journal maintenance <command> [options]\nsolstone journal maintenance: error: the following arguments are required: command\n"
        );

        let bare_run = run(
            Output::terminal(Some("en-US")),
            &["run".to_owned()],
            Path::new("/unused"),
            &MaintenanceServices::new(routines()),
            None,
            None,
        );
        assert_eq!(bare_run.exit_code, 2);
        assert_eq!(bare_run.stdout, "");
        assert_eq!(
            bare_run.stderr,
            "usage: solstone journal maintenance run ID [ARGS...]\nsolstone journal maintenance run: error: the following arguments are required: ID\n"
        );
    }
}
