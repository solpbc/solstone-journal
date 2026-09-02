// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("sign") => solstone_core_distribution::cli_sign::run(
            &args.collect::<Vec<_>>(),
            "usage: solstone-distribution-fixture sign DIRECTORY",
        ),
        _ => {
            eprintln!("usage: solstone-distribution-fixture sign DIRECTORY");
            ExitCode::from(2)
        }
    }
}
