// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::manifest::{ROOT_COMMANDS, UNAVAILABLE_LOCAL_PATHS, process_command_tokens};

pub const JOURNAL_USAGE: &str = "Usage: journal <command> [args...]\n";

#[must_use]
pub fn render_help() -> String {
    let mut output = String::from(
        "journal - native journal command root (solstone)\n\nUsage: journal <command> [args...]\n\nLocal commands:\n",
    );
    for command in ROOT_COMMANDS {
        output.push_str(&format!("  {command}\n"));
    }
    output.push_str("\nProcess commands:\n");
    for command in process_command_tokens() {
        output.push_str(&format!("  {command}\n"));
    }
    output.push_str("\nUnavailable local commands:\n");
    for path in UNAVAILABLE_LOCAL_PATHS {
        output.push_str(&format!("  {} {}\n", path.group, path.leaf));
    }
    output.push_str("\nOptions:\n  -h, --help    Show this help\n  -V, --version Show version\n  -v, --verbose Enable verbose mode\n");
    output
}

#[must_use]
pub fn version_line() -> String {
    format!("journal (solstone) {}\n", env!("CARGO_PKG_VERSION"))
}

#[must_use]
pub fn unavailable_message(token: &str) -> String {
    format!(
        "'{token}' is not available yet in the native journal command root (journal_command_unavailable).\n"
    )
}
