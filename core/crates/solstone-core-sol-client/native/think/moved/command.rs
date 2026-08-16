// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::command::{CommandContext, CommandOutput};

#[must_use]
pub fn identity(_ctx: CommandContext<'_>) -> CommandOutput {
    moved("identity")
}

#[must_use]
pub fn navigate(_ctx: CommandContext<'_>) -> CommandOutput {
    moved("navigate")
}

fn moved(name: &str) -> CommandOutput {
    CommandOutput::failure(
        format!("Moved to `journal {name}` — run that instead.\n"),
        2,
    )
}
