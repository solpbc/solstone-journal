// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::command::{CommandContext, CommandOutput};

#[must_use]
pub fn navigate(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    CommandOutput::failure(
        solstone_core_cli_copy::render_for(locale, "journal.cli.sol.moved.navigate", &[]),
        2,
    )
}
