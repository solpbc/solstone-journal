// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `journal settings` operations.
//!
//! ## Reference behavior
//!
//! Python's argparse help and argument-error paths exit before `setup_cli()`
//! reaches `get_journal()`. Successfully parsed fallback and status paths do
//! reach it, and therefore create the journal root. Keep that branch-specific
//! creation boundary: help and parse errors do not create a journal; the three
//! runtime variants do.
//!
//! Python's port reader lets an `IsADirectoryError` escape when
//! `health/convey.port` is a directory. Native reports every non-NotFound I/O
//! error cleanly with a non-zero exit instead. That is an intentional native
//! improvement, not a port-default fallback.

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use solstone_core_cli::{SettingsCommand, SettingsConveyCommand};
use solstone_core_journal::ensure_journal_dir_with_label;

use crate::{EXIT_TEMPFAIL, eprint_journal_path_error, resolve_process_journal_path};

// Machine-wide default loopback port, shared across logins. A second copy on
// this port, including one started under another login, must fail the bind
// loudly rather than isolate per user.
const DEFAULT_SERVICE_PORT: i64 = 5015;
const EXIT_FAILURE: u8 = 1;

pub(crate) fn run(command: SettingsCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    if let Err(error) = ensure_journal_dir_with_label(&journal.path, journal.label) {
        eprintln!("{error}");
        return ExitCode::from(EXIT_FAILURE);
    }

    match command {
        SettingsCommand::RootFallbackHelp => {
            print!("{}", solstone_core_cli::SETTINGS_HELP);
            ExitCode::from(EXIT_FAILURE)
        }
        SettingsCommand::Convey(SettingsConveyCommand::FallbackHelp) => {
            print!("{}", solstone_core_cli::SETTINGS_CONVEY_HELP);
            ExitCode::from(EXIT_FAILURE)
        }
        SettingsCommand::Convey(SettingsConveyCommand::Status { json }) => {
            print_status(&journal.path, json)
        }
    }
}

fn print_status(journal: &Path, json: bool) -> ExitCode {
    let port_path = journal.join("health/convey.port");
    let port = match read_convey_port(&port_path) {
        Ok(port) => convey_port_or_default(port),
        Err(error) => {
            eprintln!("Error: {}: {error}", port_path.display());
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    if json {
        println!("{{\n  \"dashboard_url\": \"http://localhost:{port}\"\n}}");
    } else {
        println!(
            "convey\n  bind:              127.0.0.1:{port}\n  dashboard url:     http://localhost:{port}"
        );
    }
    ExitCode::SUCCESS
}

fn read_convey_port(path: &Path) -> Result<Option<i64>, io::Error> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(parse_python_int(&text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn parse_python_int(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut normalized = String::with_capacity(text.len());
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        normalized.push(char::from(bytes[0]));
        index = 1;
    }
    if index == bytes.len() {
        return None;
    }

    let mut previous_was_digit = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_digit() {
            normalized.push(char::from(byte));
            previous_was_digit = true;
        } else if byte == b'_'
            && previous_was_digit
            && bytes
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_digit())
        {
            previous_was_digit = false;
        } else {
            return None;
        }
        index += 1;
    }
    if !previous_was_digit {
        return None;
    }
    normalized.parse().ok()
}

fn convey_port_or_default(port: Option<i64>) -> i64 {
    match port {
        Some(port) if port != 0 => port,
        Some(_) | None => DEFAULT_SERVICE_PORT,
    }
}

#[cfg(test)]
mod tests {
    use super::{convey_port_or_default, parse_python_int};

    #[test]
    fn parses_reference_port_values() {
        assert_eq!(parse_python_int("5051"), Some(5051));
        assert_eq!(parse_python_int("0"), Some(0));
        assert_eq!(parse_python_int("70000"), Some(70000));
        assert_eq!(parse_python_int("-1"), Some(-1));
        assert_eq!(parse_python_int("5_051"), Some(5051));
        assert_eq!(parse_python_int(""), None);
        assert_eq!(parse_python_int("abc"), None);
    }

    #[test]
    fn rejects_invalid_underscore_placement() {
        for value in ["_5051", "5051_", "5__051", "+_5051", "-_"] {
            assert_eq!(parse_python_int(value), None, "{value}");
        }
    }

    #[test]
    fn defaults_like_python_or() {
        assert_eq!(convey_port_or_default(None), 5015);
        assert_eq!(convey_port_or_default(Some(0)), 5015);
        assert_eq!(convey_port_or_default(Some(5051)), 5051);
        assert_eq!(convey_port_or_default(Some(70000)), 70000);
        assert_eq!(convey_port_or_default(Some(-1)), -1);
    }
}
