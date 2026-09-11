// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::process::ExitCode;

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

fn main() -> ExitCode {
    #[cfg(windows)]
    if env::args_os().all(|arg| arg.to_str().is_some()) {
        // The SDK reads Unicode args during construction. Leave non-Unicode
        // arguments to the journal's existing OsString boundary below.
        velopack::VelopackApp::build()
            .set_auto_apply_on_startup(false)
            .run();
    }
    install_logger();
    solstone_core_journal_cli::run(env::args_os().skip(1).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn logger_install_is_idempotent_and_defaults_to_warn() {
        super::install_logger();
        super::install_logger();
        if std::env::var("RUST_LOG").is_err() {
            assert!(log::max_level() >= log::LevelFilter::Warn);
        }
    }
}
