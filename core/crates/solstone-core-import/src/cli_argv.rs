// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal importer argv parsing; the real handler signature is defined by a later wave.

use std::path::Path;

use solstone_core_segment::{
    SUPERVISOR_MESSAGE, SupervisorRefusal, require_solstone, require_solstone_with,
};

use crate::ImportError;

const USAGE: &str = "usage: journal importer [-h] [options] media [timestamp]";

/// Observable result of a library-hosted journal importer invocation.
#[derive(Debug, Eq, PartialEq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run the importer grammar with the process environment and local supervisor probe.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    let parsed = match parse_arguments(args) {
        Ok(ParsedCommand::Help) => return success(USAGE.to_owned() + "\n"),
        Ok(parsed) => parsed,
        Err(arguments) => return argparse_error(arguments),
    };
    run_after_parse(parsed, || require_solstone(journal_path))
}

/// Run the importer grammar with injectable environment and supervisor seams.
pub fn run_cli_with<E, C>(
    args: &[String],
    _journal_path: &Path,
    lookup_env: E,
    connectivity: C,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    let parsed = match parse_arguments(args) {
        Ok(ParsedCommand::Help) => return success(USAGE.to_owned() + "\n"),
        Ok(parsed) => parsed,
        Err(arguments) => return argparse_error(arguments),
    };

    run_after_parse(parsed, || require_solstone_with(lookup_env, connectivity))
}

fn run_after_parse(
    parsed: ParsedCommand,
    preflight: impl FnOnce() -> Result<(), SupervisorRefusal>,
) -> CliRun {
    // The grammar fixture says `--nonsense` reaching the parser yields exit 2, a MATCH rather
    // than a divergence, even though the retained Python owner route gates before parsing.
    match preflight() {
        Ok(()) => {}
        Err(SupervisorRefusal::SpawnedUnavailable) => return failure("", "", 75),
        Err(SupervisorRefusal::Unavailable) => {
            return failure("", &format!("{SUPERVISOR_MESSAGE}\n"), 1);
        }
    }

    match parsed {
        ParsedCommand::Import { media, timestamp } => {
            let _ = (media, timestamp);
            failure(
                "",
                &format!("{}\n", ImportError::Unimplemented { module: "cli_argv" }),
                1,
            )
        }
        ParsedCommand::Help => unreachable!("help returns before supervisor preflight"),
    }
}

/// Reserved argv seam; its real handler signature is defined by a later wave.
pub fn reserved_seam() -> Result<(), ImportError> {
    Err(ImportError::Unimplemented { module: "cli_argv" })
}

enum ParsedCommand {
    Help,
    Import {
        media: String,
        timestamp: Option<String>,
    },
}

fn parse_arguments(args: &[String]) -> Result<ParsedCommand, String> {
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "--" {
            positionals.extend(args[index + 1..].iter().cloned());
            break;
        }
        if argument == "-h" || argument == "--help" {
            return Ok(ParsedCommand::Help);
        }
        if is_flag(argument) {
            index += 1;
            continue;
        }
        if takes_value(argument) {
            let attached_value = argument.contains('=');
            index += 1;
            if !attached_value {
                if args.get(index).is_none() {
                    return Err(format!("argument {argument}: expected one argument"));
                }
                index += 1;
            }
            continue;
        }
        if argument == "--auto" {
            if args
                .get(index + 1)
                .is_some_and(|value| !value.starts_with('-'))
            {
                index += 1;
            }
            index += 1;
            continue;
        }
        if argument.starts_with('-') {
            return Err(format!("unrecognized arguments: {argument}"));
        }
        positionals.push(argument.clone());
        index += 1;
    }

    match positionals.as_slice() {
        [media] => Ok(ParsedCommand::Import {
            media: media.clone(),
            timestamp: None,
        }),
        [media, timestamp] => Ok(ParsedCommand::Import {
            media: media.clone(),
            timestamp: Some(timestamp.clone()),
        }),
        [] => Err("the following arguments are required: media".to_owned()),
        _ => Err(format!(
            "unrecognized arguments: {}",
            positionals[2..].join(" ")
        )),
    }
}

fn is_flag(argument: &str) -> bool {
    matches!(
        argument,
        "--force"
            | "--dry-run"
            | "--confirm-body-save"
            | "--confirm-health-save"
            | "--with-day-summaries"
            | "--deterministic-only"
            | "--backends"
            | "--save"
            | "--scheduled"
            | "--list-importers"
            | "--json"
            | "-v"
            | "--verbose"
            | "-d"
            | "--debug"
    )
}

fn takes_value(argument: &str) -> bool {
    matches!(
        argument,
        "--timestamp"
            | "--facet"
            | "--setting"
            | "--source"
            | "--sync"
            | "--path"
            | "--window-days"
            | "--connect"
            | "--date-from"
            | "--date-to"
    ) || argument.split_once('=').is_some_and(|(name, _)| {
        matches!(
            name,
            "--timestamp"
                | "--facet"
                | "--setting"
                | "--source"
                | "--sync"
                | "--path"
                | "--window-days"
                | "--connect"
                | "--date-from"
                | "--date-to"
                | "--auto"
        )
    })
}

fn argparse_error(arguments: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{USAGE}\njournal importer: error: {arguments}\n"),
        exit_code: 2,
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(stdout: &str, stderr: &str, exit_code: i32) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
    }
}
