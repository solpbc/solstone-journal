// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::command::{CommandContext, CommandOutput};

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    super::apps_network_native_command_rs::status(ctx)
}
