// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[path = "../../../solstone-core-journal-cli/src/processes.rs"]
mod production_processes;

pub(crate) fn token() -> &'static str {
    production_processes::PROCESS_SPECS
        .iter()
        .filter(|spec| {
            production_processes::NATIVE_PROCESS_SPECS
                .iter()
                .all(|native| native.token != spec.token)
        })
        .map(|spec| spec.token)
        .min()
        .expect("at least one process token remains routed through Python")
}
