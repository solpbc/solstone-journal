// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::process::ExitCode;

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

fn main() -> ExitCode {
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
