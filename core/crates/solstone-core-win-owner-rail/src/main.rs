// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Command boundary for the Windows ordinary-owner scheduled-task rail.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run(std::env::args_os().skip(1).collect()) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "win-owner-rail: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("solstone-core-win-owner-rail is only runnable on Windows");
    std::process::exit(1);
}
