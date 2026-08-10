// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cogitate::{AccessTierError, capabilities_for_access_tier};

use crate::{
    EMIT_FINAL_TOOL, FINISH_TOOL, GLOB_TOOL, GREP_SEARCH_TOOL, LIST_DIRECTORY_TOOL, READ_FILE_TOOL,
    ToolSpec, sol_tool,
};

pub const KNOWN_TOOL_NAMES: [&str; 7] = [
    "sol",
    "glob",
    "grep_search",
    "list_directory",
    "read_file",
    "emit_final",
    "finish",
];

/// Bind only the tier's explicit tools. `sol` may persist domain state through
/// policy-gated `sol call` verbs; there is no general-purpose write tool.
pub fn bound_tools(
    access_tier: &str,
    expects_emit_final: bool,
) -> Result<Vec<&'static ToolSpec>, AccessTierError> {
    let capabilities = capabilities_for_access_tier(access_tier)?;
    let mut tools = Vec::new();
    if capabilities.sol {
        tools.push(sol_tool());
    }
    if capabilities.reads {
        tools.extend([
            &READ_FILE_TOOL,
            &LIST_DIRECTORY_TOOL,
            &GLOB_TOOL,
            &GREP_SEARCH_TOOL,
        ]);
    }
    if expects_emit_final {
        tools.push(&EMIT_FINAL_TOOL);
    } else {
        tools.push(&FINISH_TOOL);
    }
    Ok(tools)
}
