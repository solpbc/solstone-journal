// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::process;

use solstone_core_depict::{DepictError, USAGE, error_json_line, parse_args, run};

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
            eprintln!("journal process admission failed: {error}");
            process::exit(EXIT_HOSTED_LAUNCH_ADMISSION_REFUSED);
        }
    };
    install_logger();
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{}", solstone_core_depict::USAGE);
        return;
    }
    match parse_args(&args).and_then(run) {
        Ok(_) => {}
        Err(DepictError::Help) => print!("{USAGE}"),
        Err(error) => {
            eprintln!("{}", error_json_line(&error));
            process::exit(error.exit_code());
        }
    }
}
