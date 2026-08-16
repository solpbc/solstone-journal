// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use solstone_core_distribution::discover_and_validate_inventory;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("validate") => {
            let start = args
                .next()
                .map(PathBuf::from)
                .or_else(|| env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            match discover_and_validate_inventory(&start) {
                Ok(inventory) => {
                    println!(
                        "inventory ok: product={} entries={} denies={}",
                        inventory.product,
                        inventory.entry.len(),
                        inventory.deny.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("help" | "--help" | "-h") | None => {
            println!("usage: solstone-distribution <validate|help> [START_DIR]");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command {other:?}");
            eprintln!("usage: solstone-distribution <validate|help> [START_DIR]");
            ExitCode::from(2)
        }
    }
}
