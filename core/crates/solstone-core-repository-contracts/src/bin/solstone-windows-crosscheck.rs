// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_repository_contracts::windows_crosscheck;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("solstone-windows-crosscheck: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let config = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "usage: solstone-windows-crosscheck CONFIG".to_owned())?;
    if args.next().is_some() {
        return Err("usage: solstone-windows-crosscheck CONFIG".to_owned());
    }
    let repo =
        std::env::current_dir().map_err(|error| format!("read current directory: {error}"))?;
    windows_crosscheck::run(&repo, &config)
}
