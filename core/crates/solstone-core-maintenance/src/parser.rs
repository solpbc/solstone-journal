// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_backup_runtime::BackupServices;

use crate::schedule_sync::{render_list, render_summary, schedules_path, sync};
use crate::{CliRun, HealthServices, MaintenanceServices, TimelineServices};

pub const USAGE: &str = "usage: journal maintenance <command> [options]\n";
const LIST_USAGE: &str = "usage: journal maintenance list\n";
const SYNC_USAGE: &str = "usage: journal maintenance sync\n";
const RUN_USAGE: &str = "usage: journal maintenance run ID [ARGS...]\n";
const MIGRATE_USAGE: &str = "usage: journal maintenance migrate-timeline [--commit] [--limit N]\n";

pub(crate) fn run(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices<'_>>,
    timeline_services: Option<&TimelineServices<'_>>,
) -> CliRun {
    let args = normalize_global_flags(args);
    if has_help(&args) {
        return success(usage_for_scope(&args).to_owned());
    }
    let Some((command, rest)) = args.split_first() else {
        return usage_error(USAGE, "");
    };
    match command.as_str() {
        "list" if no_positionals(rest) => list(journal, services),
        "sync" if no_positionals(rest) => sync_schedules(journal, services),
        "run" => run_routine(
            rest,
            journal,
            services,
            backup_services,
            health_services,
            timeline_services,
        ),
        // Plan by default: it reports what a legacy-artifact migration would do and writes
        // nothing. `--commit` is required to write, because the failure mode of this command
        // is the loss of the owner's historical journal prose.
        "migrate-timeline" => migrate_timeline(rest, journal),
        _ => usage_error(USAGE, &args.join(" ")),
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

fn usage_for_scope(args: &[String]) -> &'static str {
    match args.first().map(String::as_str) {
        Some("list") => LIST_USAGE,
        Some("sync") => SYNC_USAGE,
        Some("run") => RUN_USAGE,
        Some("migrate-timeline") => MIGRATE_USAGE,
        _ => USAGE,
    }
}

fn migrate_timeline(args: &[String], journal: &Path) -> CliRun {
    let mut commit = false;
    let mut limit = None;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--commit" => commit = true,
            "--limit" => {
                let Some(value) = rest.next().and_then(|value| value.parse::<u64>().ok()) else {
                    return usage_error(MIGRATE_USAGE, "--limit needs a positive count");
                };
                if value == 0 {
                    return usage_error(MIGRATE_USAGE, "--limit needs a positive count");
                }
                limit = Some(value);
            }
            other => return usage_error(MIGRATE_USAGE, other),
        }
    }
    if commit {
        return success(crate::bodies::migrate::commit(journal, limit).render());
    }
    // `--limit` without `--commit` would read as though it bounded the survey, which it does
    // not: the plan always counts the whole corpus.
    if limit.is_some() {
        return usage_error(MIGRATE_USAGE, "--limit only applies with --commit");
    }
    success(crate::bodies::migrate::plan(journal).render())
}

fn list(journal: &Path, services: &MaintenanceServices<'_>) -> CliRun {
    let path = schedules_path(journal);
    match render_list(&path, services.routines) {
        Ok(stdout) => success(stdout),
        Err(error) => schedule_error(&path, error),
    }
}

fn sync_schedules(journal: &Path, services: &MaintenanceServices<'_>) -> CliRun {
    let path = schedules_path(journal);
    match sync(&path, services.routines) {
        Ok(summary) => success(render_summary(&summary)),
        Err(error) => schedule_error(&path, error),
    }
}

fn run_routine(
    args: &[String],
    journal: &Path,
    services: &MaintenanceServices<'_>,
    backup_services: Option<&BackupServices<'_>>,
    health_services: Option<&HealthServices<'_>>,
    timeline_services: Option<&TimelineServices<'_>>,
) -> CliRun {
    let Some((id, routine_args)) = args.split_first() else {
        return usage_error(RUN_USAGE, "");
    };
    let forwarded = routine_args
        .strip_prefix(&["--".to_owned()])
        .unwrap_or(routine_args);
    let Some(_routine) = services.routines.iter().find(|routine| routine.id == id) else {
        return CliRun {
            stdout: String::new(),
            stderr: format!(
                "Unknown maintenance routine: {id}. Run `journal maintenance list` to see available routines.\n"
            ),
            exit_code: 1,
        };
    };
    if matches!(
        id.as_str(),
        "backup:run" | "backup:prune" | "backup:verify" | "backup:offload"
    ) {
        return match backup_services {
            Some(backup_services) => {
                crate::bodies::backup::run(id, forwarded, journal, backup_services)
            }
            None => CliRun {
                stdout: String::new(),
                stderr: "backup maintenance services are unavailable.\n".to_owned(),
                exit_code: 1,
            },
        };
    }
    if matches!(id.as_str(), "health:mark-raw" | "health:prune-logs") {
        return match health_services {
            Some(health_services) => {
                crate::bodies::health::run(id, forwarded, journal, health_services)
            }
            None => CliRun {
                stdout: String::new(),
                stderr: "health maintenance services are unavailable.\n".to_owned(),
                exit_code: 1,
            },
        };
    }
    if matches!(
        id.as_str(),
        "timeline:rollup" | "timeline:rollup-day" | "timeline:rollup-master"
    ) {
        return match timeline_services {
            Some(timeline_services) => {
                crate::bodies::timeline::run(id, forwarded, journal, timeline_services)
            }
            None => CliRun {
                stdout: String::new(),
                stderr: "timeline maintenance services are unavailable.\n".to_owned(),
                exit_code: 1,
            },
        };
    }
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "maintenance routine {id} is not yet implemented ({} forwarded argument(s)).\n",
            forwarded.len()
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

fn schedule_error(path: &Path, error: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "Error reading/updating {}: {error} (cause: {error})\n",
            path.display()
        ),
        exit_code: 1,
    }
}

fn usage_error(usage: &str, arguments: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{usage}journal maintenance: error: unrecognized arguments: {arguments}\n"),
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::MaintenanceServices;
    use crate::registry::routines;
    use std::path::Path;

    #[test]
    fn unknown_run_id_uses_the_reference_message_without_routine_execution() {
        let args = vec!["run".to_owned(), "other:missing".to_owned()];
        let result = run(
            &args,
            Path::new("/unused"),
            &MaintenanceServices::new(routines()),
            None,
            None,
            None,
        );
        assert_eq!(result.exit_code, 1);
        assert_eq!(
            result.stderr,
            "Unknown maintenance routine: other:missing. Run `journal maintenance list` to see available routines.\n"
        );
    }

    #[test]
    fn parser_keeps_unknown_flags_in_maintenance_owned_usage() {
        let args = vec!["--nonsense".to_owned()];
        let result = run(
            &args,
            Path::new("/unused"),
            &MaintenanceServices::new(routines()),
            None,
            None,
            None,
        );
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.starts_with("usage: journal maintenance"));
    }

    #[test]
    fn empty_registry_renders_the_reference_empty_message() {
        let args = vec!["list".to_owned()];
        let result = run(
            &args,
            Path::new("/unused"),
            &MaintenanceServices::new(&[]),
            None,
            None,
            None,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "No maintenance routines found.\n");
    }

    #[test]
    fn run_arguments_are_preserved_after_the_subcommand() {
        let args = vec![
            "-v".to_owned(),
            "run".to_owned(),
            "timeline:rollup-day".to_owned(),
            "--".to_owned(),
            "-v".to_owned(),
            "--dry-run".to_owned(),
        ];
        let result = run(
            &args,
            Path::new("/unused"),
            &MaintenanceServices::new(routines()),
            None,
            None,
            None,
        );
        assert_eq!(result.exit_code, 1);
        assert_eq!(
            result.stderr,
            "timeline maintenance services are unavailable.\n"
        );
    }
}
