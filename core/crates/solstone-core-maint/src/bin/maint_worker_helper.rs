// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let journal = env::var_os("SOLSTONE_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let result = solstone_core_maint::worker::run(&args, &journal);
    for line in result.stdout {
        println!("{line}");
    }
    eprint!("{}", result.stderr);
    ExitCode::from(result.exit_code as u8)
}
