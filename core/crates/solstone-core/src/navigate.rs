// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `journal navigate` orchestration.
//!
//! ## Intentional reference divergences
//!
//! The reference rejects an option after the positional
//! (`navigate /home --facet work` -> `No such command '--facet'`, exit 2)
//! because `invoke_without_command=True` treats the trailing token as a
//! subcommand name. Option-before-positional already works. The native rebuild
//! accepts either position; only the after-positional form changes.
//!
//! The reference's `callosum_send` swallows every exception and the caller
//! ignores the return, so with the supervisor gate passing but the socket
//! absent, stale, or refusing, the owner is told `Navigate: ...` and exits 0
//! for a message never sent. The native rebuild exits 69 instead. The
//! journal-down case is already handled by the supervisor gate and is not
//! covered by this divergence.
//!
//! A successful socket write means the navigation request was sent to
//! Callosum; it does not claim that navigation happened. Callosum fans out
//! only to connected clients, and a first write to a closed peer typically succeeds.

use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_transcribe::require_solstone;

use crate::{
    EXIT_TEMPFAIL, EXIT_UNAVAILABLE, eprint_journal_path_error, resolve_process_journal_path,
};

/// Match the reference's `typer.Exit(1)` for a syntactically valid empty request.
/// This is deliberately distinct from the supervisor gate's own exit-1 path.
const EXIT_MISSING_REQUEST: u8 = 1;
/// Matches `callosum_send`'s default, deliberately not rescan's one-second timeout.
const NAVIGATE_SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(path: Option<String>, facet: Option<String>) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    if let Err(error) = require_solstone(&journal.path) {
        if let Some(message) = error.message() {
            eprintln!("{message}");
        }
        return ExitCode::from(error.exit_code() as u8);
    }

    // Python's `if not path and not facet` treats supplied empty strings as absent.
    if is_empty(&path) && is_empty(&facet) {
        eprintln!("Error: provide a path and/or --facet");
        return ExitCode::from(EXIT_MISSING_REQUEST);
    }

    let mut extra = Map::new();
    if let Some(path) = &path {
        extra.insert("path".to_owned(), Value::String(path.clone()));
    }
    if let Some(facet) = &facet {
        extra.insert("facet".to_owned(), Value::String(facet.clone()));
    }
    let envelope = CallosumEnvelope {
        tract: "navigate".to_owned(),
        event: "request".to_owned(),
        ts: None,
        extra,
    };
    let mut line = match serde_json::to_string(&envelope) {
        Ok(line) => line,
        Err(error) => {
            eprintln!("journal navigate: error: failed to encode Callosum request: {error}");
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    line.push('\n');

    let socket = journal.path.join("health").join("callosum.sock");
    let sender = CallosumOneShotSender::new(&socket, NAVIGATE_SOCKET_TIMEOUT);
    if sender.send_line(&line).is_err() {
        eprintln!(
            "journal navigate: error: Callosum socket unavailable: {}",
            socket.display()
        );
        return ExitCode::from(EXIT_UNAVAILABLE);
    }

    print_navigation(&path, &facet);
    ExitCode::SUCCESS
}

fn is_empty(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(str::is_empty)
}

fn print_navigation(path: &Option<String>, facet: &Option<String>) {
    match (
        path.as_deref().filter(|value| !value.is_empty()),
        facet.as_deref().filter(|value| !value.is_empty()),
    ) {
        (Some(path), Some(facet)) => println!("Navigate: {path} [{facet}]"),
        (Some(path), None) => println!("Navigate: {path}"),
        (None, Some(facet)) => println!("Navigate: [{facet}]"),
        (None, None) => unreachable!("empty navigation requests return before sending"),
    }
}

#[cfg(test)]
mod tests {
    use super::{EXIT_MISSING_REQUEST, NAVIGATE_SOCKET_TIMEOUT};
    use solstone_core_transcribe::CliError;
    use std::time::Duration;

    #[test]
    fn navigate_exit_codes_are_distinct() {
        let usage = CliError::Usage {
            message: String::new(),
        }
        .exit_code() as u8;
        let supervisor_unavailable = CliError::SupervisorUnavailable.exit_code() as u8;
        let supervisor_spawned = CliError::SupervisorSpawnedUnavailable.exit_code() as u8;

        assert_eq!(EXIT_MISSING_REQUEST, supervisor_unavailable);
        assert_ne!(crate::EXIT_UNAVAILABLE, EXIT_MISSING_REQUEST);
        assert_ne!(crate::EXIT_UNAVAILABLE, usage);
        assert_ne!(crate::EXIT_UNAVAILABLE, supervisor_spawned);
        assert_ne!(EXIT_MISSING_REQUEST, usage);
        assert_ne!(EXIT_MISSING_REQUEST, supervisor_spawned);
        assert_ne!(usage, supervisor_spawned);
    }

    #[test]
    fn navigate_socket_timeout_matches_the_reference_default() {
        assert_eq!(NAVIGATE_SOCKET_TIMEOUT, Duration::from_secs(2));
    }
}
