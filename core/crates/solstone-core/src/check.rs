// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use solstone_core_check::{
    build_check_report, exit_code, gather_host_inputs, human_host_report, json_output,
};

pub(super) fn run(json: bool) -> u8 {
    let journal = super::resolve_process_journal_path()
        .map(|line| line.path)
        .unwrap_or_else(|_| PathBuf::from("journal"));
    if json {
        let report = build_check_report(&gather_host_inputs(&journal, env!("CARGO_PKG_VERSION")));
        print!("{}", json_output(&report));
        exit_code(&report)
    } else {
        let locale =
            solstone_core_cli_copy::terminal_locale().unwrap_or(solstone_core_cli_copy::ENGLISH);
        let (output, status) = human_host_report(locale, &journal, env!("CARGO_PKG_VERSION"));
        print!("{output}");
        status
    }
}
