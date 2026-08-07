// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub const ROOT_COMMANDS: &[&str] = &[
    "--path", "path", "status", "root", "doctor", "check", "contract", "notify",
];
pub const JOURNAL_COMMAND_COUNT: usize =
    ROOT_COMMANDS.len() + solstone_core_sol::JOURNAL_HOST_COMMAND_COUNT;

pub(crate) fn known_token(value: &str) -> Option<&'static str> {
    if let Some(&token) = ROOT_COMMANDS.iter().find(|&&token| token == value) {
        return Some(token);
    }
    solstone_core_sol::JOURNAL_HOST_COMMANDS
        .binary_search(&value)
        .ok()
        .map(|index| solstone_core_sol::JOURNAL_HOST_COMMANDS[index])
}
