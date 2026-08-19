// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    solstone_core_sol::run("solstone", env::args_os().skip(1).collect())
}
