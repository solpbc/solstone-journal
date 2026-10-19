// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    let code = i18n_catalog::cli::run(std::env::args_os());
    std::process::exit(code);
}
