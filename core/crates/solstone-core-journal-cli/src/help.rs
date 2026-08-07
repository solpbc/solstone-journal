// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::manifest::ROOT_COMMANDS;

pub const JOURNAL_USAGE: &str = "Usage: journal <command> [args...]\n";

#[must_use]
pub fn render_help() -> String {
    let mut output = String::from(
        "journal - native journal command root (solstone)\n\nUsage: journal <command> [args...]\n\nRoot commands:\n",
    );
    for command in ROOT_COMMANDS {
        output.push_str(&format!("  {command}\n"));
    }
    output.push_str("\nService commands:\n");
    for command in solstone_core_sol::JOURNAL_HOST_COMMANDS {
        output.push_str(&format!("  {command}\n"));
    }
    output.push_str("\nOptions:\n  -h, --help    Show this help\n  --version     Show version\n");
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
