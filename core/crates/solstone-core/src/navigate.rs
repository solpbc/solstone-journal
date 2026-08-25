// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `journal navigate` orchestration.
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

/// Matches `callosum_send`'s default, deliberately not rescan's one-second timeout.
const NAVIGATE_SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(path: String) -> ExitCode {
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

    let mut extra = Map::new();
    extra.insert("path".to_owned(), Value::String(path.clone()));
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

    print_navigation(&path);
    ExitCode::SUCCESS
}

fn print_navigation(path: &str) {
    println!("Navigate: {path}");
}

#[cfg(test)]
mod tests {
    use super::NAVIGATE_SOCKET_TIMEOUT;
    use std::time::Duration;

    #[test]
    fn navigate_socket_timeout_matches_the_reference_default() {
        assert_eq!(NAVIGATE_SOCKET_TIMEOUT, Duration::from_secs(2));
    }
}
