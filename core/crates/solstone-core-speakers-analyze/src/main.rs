// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::io::{self, Read};
use std::path::Path;
use std::process;

use solstone_core_speakers_analyze::{
    error_json_line, error_line_for_analyze_error, error_line_for_usage, evaluate_args,
    run_command_request,
};

const EXIT_USAGE: i32 = 64;
const EXIT_UNAVAILABLE: i32 = 69;
const EXIT_TEMPFAIL: i32 = 75;

fn main() {
    if env::var_os(solstone_core_system::lifecycle::HOSTED_GENERATION_ENV).is_some() {
        let Some(journal) = env::var_os("SOLSTONE_JOURNAL") else {
            eprintln!("hosted speakers analysis child is missing SOLSTONE_JOURNAL");
            process::exit(EXIT_TEMPFAIL);
        };
        if let Err(error) =
            solstone_core_system::lifecycle::acknowledge_hosted_child_admission(Path::new(&journal))
        {
            eprintln!("hosted speakers analysis admission failed: {error}");
            process::exit(EXIT_TEMPFAIL);
        }
    }
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

    match run_command_request(command, &input) {
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
