// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use crate::Outcome;
use crate::layout::{inspect_journal_days, resolve_current_journal, resolve_project_root};

const EXIT_IOERR: u8 = 74;
const EXIT_TEMPFAIL: u8 = 75;

pub(crate) fn is_help_only(args: &[std::ffi::OsString]) -> bool {
    args.len() == 1 && (args[0] == "--help" || args[0] == "-h")
}

pub(crate) fn path() -> Outcome {
    let journal = match resolve_current_journal() {
        Ok(journal) => journal,
        Err(error) => return resolution_failure(error),
    };
    let Some(path) = journal.path.to_str() else {
        return Outcome::LocalFailure {
            stdout: String::new(),
            stderr: format!(
                "{}\n",
                solstone_core_cli_copy::render_for(
                    crate::terminal_language(),
                    "journal.cli.journal.host.path.utf8",
                    &[]
                )
            ),
            exit: EXIT_TEMPFAIL,
        };
    };
    Outcome::LocalSuccess {
        stdout: format!("{path}\n"),
        stderr: String::new(),
    }
}

pub(crate) fn status() -> Outcome {
    let journal = match resolve_current_journal() {
        Ok(journal) => journal,
        Err(error) => return resolution_failure(error),
    };
    let Some(path) = journal.path.to_str() else {
        return Outcome::LocalFailure {
            stdout: String::new(),
            stderr: format!(
                "{}\n",
                solstone_core_cli_copy::render_for(
                    crate::terminal_language(),
                    "journal.cli.journal.host.status.utf8",
                    &[]
                )
            ),
            exit: EXIT_TEMPFAIL,
        };
    };

    let exists = match fs::metadata(&journal.path) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => {
            return Outcome::LocalFailure {
                stdout: String::new(),
                stderr: format!(
                    "{}\n",
                    solstone_core_cli_copy::display_for(
                        crate::terminal_language(),
                        "journal.cli.journal.host.status.directory",
                        [("path", &path)]
                    )
                ),
                exit: EXIT_IOERR,
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Outcome::LocalFailure {
                stdout: String::new(),
                stderr: format!(
                    "{}\n",
                    solstone_core_cli_copy::display_for(
                        crate::terminal_language(),
                        "journal.cli.journal.host.status.inspect",
                        [("path", &path), ("error", &error)]
                    )
                ),
                exit: EXIT_TEMPFAIL,
            };
        }
    };
    let days = match inspect_journal_days(&journal.path) {
        Ok(Some(days)) => days,
        Ok(None) => 0,
        Err(error) => {
            return Outcome::LocalFailure {
                stdout: String::new(),
                stderr: format!(
                    "{}\n",
                    solstone_core_cli_copy::display_for(
                        crate::terminal_language(),
                        "journal.cli.journal.host.status.inspect",
                        [("path", &path), ("error", &error)]
                    )
                ),
                exit: EXIT_TEMPFAIL,
            };
        }
    };
    let source = journal.source;
    Outcome::LocalSuccess {
        stdout: format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                crate::terminal_language(),
                if exists {
                    "journal.cli.journal.host.status.exists"
                } else {
                    "journal.cli.journal.host.status.absent"
                },
                [("path", &path), ("source", &source), ("days", &days)],
            )
        ),
        stderr: String::new(),
    }
}

pub(crate) fn root() -> Outcome {
    match resolve_project_root() {
        Ok(root) => Outcome::LocalSuccess {
            stdout: format!("{}\n", root.display()),
            stderr: String::new(),
        },
        Err(error) => Outcome::LocalFailure {
            stdout: String::new(),
            stderr: format!("{}\n", error.render(crate::terminal_language())),
            exit: EXIT_TEMPFAIL,
        },
    }
}

pub(crate) fn resolution_failure(error: crate::layout::JournalResolutionError) -> Outcome {
    Outcome::LocalFailure {
        stdout: String::new(),
        stderr: format!("{}\n", error.native_message(crate::terminal_language())),
        exit: EXIT_TEMPFAIL,
    }
}
