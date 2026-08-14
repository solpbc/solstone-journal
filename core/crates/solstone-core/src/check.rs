// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use solstone_core_check::{
    build_check_report, exit_code, gather_host_inputs, human_output, json_output,
};

pub(super) fn run(json: bool) -> u8 {
    let journal = super::resolve_process_journal_path()
        .map(|line| line.path)
        .unwrap_or_else(|_| PathBuf::from("journal"));
    let report = build_check_report(&gather_host_inputs(&journal, env!("CARGO_PKG_VERSION")));
    if json {
        print!("{}", json_output(&report));
    } else {
        print!("{}", human_output(&report));
    }
    exit_code(&report)
}
