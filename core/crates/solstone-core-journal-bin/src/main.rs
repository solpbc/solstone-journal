// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::ExitCode;

/// Private journal entry. The public command is `solstone journal`.
fn main() -> ExitCode {
    solstone_core_sol::process_main()
}
