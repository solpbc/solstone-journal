// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::manifest::{LOCAL_PATHS, ROOT_COMMANDS, process_command_tokens};

pub const JOURNAL_USAGE: &str = "Usage: solstone journal <command> [args...]\n";

#[must_use]
pub fn render_help() -> String {
    render_help_for(solstone_core_cli_copy::ENGLISH)
}

pub(crate) fn render_help_for(locale: &str) -> String {
    let commands = ROOT_COMMANDS
        .iter()
        .map(|name| format!("  {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    let processes = process_command_tokens()
        .map(|name| format!("  {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    let local = LOCAL_PATHS
        .iter()
        .map(|path| format!("  {} {}", path.group, path.leaf))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n",
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.help.overview",
            [
                ("commands", &commands),
                ("processes", &processes),
                ("local", &local)
            ]
        )
    )
}

#[must_use]
pub fn version_line() -> String {
    format!("journal (solstone) {}\n", env!("CARGO_PKG_VERSION"))
}
