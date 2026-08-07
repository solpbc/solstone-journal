// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use crate::Outcome;
use crate::layout::{inspect_journal_days, resolve_current_journal, resolve_project_root};

const EXIT_IOERR: u8 = 74;
const EXIT_TEMPFAIL: u8 = 75;

pub(crate) fn path() -> Outcome {
    let journal = match resolve_current_journal() {
        Ok(journal) => journal,
        Err(error) => return resolution_failure(error),
    };
    let Some(path) = journal.path.to_str() else {
        return Outcome::LocalFailure {
            stdout: String::new(),
            stderr:
                "native journal path resolution failed: resolved journal path is not valid UTF-8\n"
                    .to_string(),
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
            stderr: "native journal status failed: resolved journal path is not valid UTF-8\n"
                .to_string(),
            exit: EXIT_TEMPFAIL,
        };
    };

    let exists = match fs::metadata(&journal.path) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => {
            return Outcome::LocalFailure {
                stdout: String::new(),
                stderr: format!(
                    "native journal status failed: journal root is not a directory: {path}\n"
                ),
                exit: EXIT_IOERR,
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Outcome::LocalFailure {
                stdout: String::new(),
                stderr: format!(
                    "native journal status failed: could not inspect {path}: {error}\n"
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
                    "native journal status failed: could not inspect {path}: {error}\n"
                ),
                exit: EXIT_TEMPFAIL,
            };
        }
    };
    let source = journal.source;
    Outcome::LocalSuccess {
        stdout: format!(
            "Journal: {path}\nSource: {source}\nExists: {}\nDays: {days}\n",
            if exists { "yes" } else { "no" }
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
            stderr: format!("{error}\n"),
            exit: EXIT_TEMPFAIL,
        },
    }
}

pub(crate) fn resolution_failure(error: impl std::fmt::Display) -> Outcome {
    Outcome::LocalFailure {
        stdout: String::new(),
        stderr: format!("native journal resolution failed: {error}\n"),
        exit: EXIT_TEMPFAIL,
    }
}
