// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::manifest::{LOCAL_PATHS, ROOT_COMMANDS, process_command_tokens};

pub const JOURNAL_USAGE: &str = "Usage: solstone journal <command> [args...]\n";

#[must_use]
pub fn render_help() -> String {
    let mut output = String::from(
        "solstone journal - the journal on this computer\n\nUsage: solstone journal <command> [args...]\n\n'journal <command>' runs the same commands.\n\nLocal commands:\n",
    );
    for command in ROOT_COMMANDS {
        output.push_str(&format!("  {command}\n"));
    }
    output.push_str("\nProcess commands:\n");
    for command in process_command_tokens() {
        output.push_str(&format!("  {command}\n"));
    }
    output.push_str("\nJournal-local commands:\n");
    for path in LOCAL_PATHS {
        output.push_str(&format!("  {} {}\n", path.group, path.leaf));
    }
    output.push_str("\nOptions:\n  -h, --help    Show this help\n  -V, --version Show version\n  -v, --verbose Enable verbose mode\n");
    output
}

#[must_use]
pub fn version_line() -> String {
    format!("journal (solstone) {}\n", env!("CARGO_PKG_VERSION"))
}
