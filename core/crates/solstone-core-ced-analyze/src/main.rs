// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::io::{self, Read};
use std::process;

use solstone_core_ced_analyze::{
    Command, error_json_line, error_line_for_analyze_error, error_line_for_usage, evaluate_args,
    run_classify_request, run_probe_request,
};

const EXIT_USAGE: i32 = 64;
const EXIT_UNAVAILABLE: i32 = 69;
const EXIT_TEMPFAIL: i32 = 75;

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let command = match evaluate_args(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{}", error_line_for_usage(&error));
            process::exit(EXIT_USAGE);
        }
    };

    let mut input = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut input) {
        eprintln!(
            "{}",
            error_json_line("internal-error", &format!("failed to read stdin: {error}"))
        );
        process::exit(EXIT_TEMPFAIL);
    }

    let result = match command {
        Command::Run => run_classify_request(&input),
        Command::Probe => run_probe_request(&input),
    };
    match result {
        Ok(response) => {
            println!(
                "{}",
                serde_json::to_string(&response).expect("response JSON serialization")
            );
        }
        Err(error) => {
            eprintln!("{}", error_line_for_analyze_error(&error));
            process::exit(match error.exit_code() {
                EXIT_USAGE => EXIT_USAGE,
                EXIT_UNAVAILABLE => EXIT_UNAVAILABLE,
                _ => EXIT_TEMPFAIL,
            });
        }
    }
}
