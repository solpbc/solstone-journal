// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[path = "../../../solstone-core-journal-cli/src/processes.rs"]
mod production_processes;

fn spec() -> &'static production_processes::ProcessSpec {
    production_processes::PROCESS_SPECS
        .iter()
        .filter(|spec| {
            production_processes::NATIVE_PROCESS_SPECS
                .iter()
                .all(|native| native.token != spec.token)
        })
        .min_by_key(|spec| spec.token)
        .expect("at least one process token remains routed through Python")
}

pub(crate) fn token() -> &'static str {
    spec().token
}

pub(crate) fn module() -> &'static str {
    spec().module
}

pub(crate) fn preset_argv() -> &'static [&'static str] {
    spec().preset_argv
}
