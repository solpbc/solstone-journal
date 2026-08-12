// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    std::process::exit(solstone_core_pdf::entrypoint(std::env::args_os().skip(1)));
}
