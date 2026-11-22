// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::process;

use solstone_core_depict::{DepictError, error_json_line, parse_args, run_in};

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

/// The same refusal code `solstone-core` returns when its launcher's admission
/// does not complete.
#[cfg(windows)]
const EXIT_HOSTED_LAUNCH_ADMISSION_REFUSED: i32 = 78;

fn main() {
    // A hosted `journal depict` forwards here through a launch that waits for
    // this process to connect and acknowledge. Admission belongs to process
    // entry, before any work, and the admission is held until exit.
    #[cfg(windows)]
    let _admitted = match solstone_core_system::process::receive_windows_launch() {
        Ok(admitted) => admitted,
        Err(error) => {
            let locale = solstone_core_cli_copy::terminal_locale()
                .unwrap_or(solstone_core_cli_copy::ENGLISH);
            let detail = solstone_core_cli_copy::MessageDetail::from(error)
                .context("journal.cli.depict.admission");
            eprintln!("{}", detail.render(locale));
            process::exit(EXIT_HOSTED_LAUNCH_ADMISSION_REFUSED);
        }
    };
    install_logger();
    let locale =
        solstone_core_cli_copy::terminal_locale().unwrap_or(solstone_core_cli_copy::ENGLISH);
    let args: Vec<_> = env::args_os().skip(1).collect();
    match parse_args(&args).and_then(|args| run_in(locale, args)) {
        Ok(_) => {}
        Err(DepictError::Help) => println!(
            "{}",
            solstone_core_cli_copy::render_for(locale, "journal.cli.depict.help", &[])
        ),
        Err(error) => {
            eprintln!("{}", error_json_line(&error));
            process::exit(error.exit_code());
        }
    }
}
