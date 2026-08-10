// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod bus;
mod runtime;
mod shutdown;
mod status;
mod tick;

use std::process::ExitCode;

use solstone_core_cli::SupervisorOptions;
use solstone_core_system::lifecycle::{LifecycleError, SupervisorLifecycle};

pub(crate) fn run(options: SupervisorOptions) -> ExitCode {
    let journal = match super::resolve_journal_config_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => {
            super::eprint_journal_path_error(error);
            return ExitCode::from(super::EXIT_TEMPFAIL);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("supervisor runtime unavailable: {error}");
            return ExitCode::from(super::EXIT_TEMPFAIL);
        }
    };
    let lifecycle = match SupervisorLifecycle::boot(&journal) {
        Ok(lifecycle) => lifecycle,
        Err(LifecycleError::SyncConflict(_)) => {
            eprintln!("supervisor: refusing boot, live foreign writer detected");
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("supervisor failed to boot: {error}");
            return ExitCode::from(super::EXIT_TEMPFAIL);
        }
    };
    let outcome = match runtime.block_on(runtime::boot_and_tick(lifecycle, journal)) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("supervisor failed to boot: {error}");
            return ExitCode::from(super::EXIT_TEMPFAIL);
        }
    };
    let mut driver = outcome.state.into_shutdown_driver(&runtime);
    if let Err(error) =
        outcome
            .lifecycle
            .shutdown(&mut driver, outcome.regime, outcome.sync_conflict)
    {
        eprintln!("supervisor shutdown failed: {error}");
    }
    if outcome.sync_conflict {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}
