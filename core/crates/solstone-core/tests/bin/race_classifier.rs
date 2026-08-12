// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::process::ExitCode;

#[allow(dead_code)]
#[path = "../support/await_outcome.rs"]
mod await_outcome;
#[allow(dead_code)]
#[path = "../support/race_classification.rs"]
mod race_classification;

use race_classification::classify;

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let program = arguments.next().expect("program name");
    let Some(capture) = arguments.next() else {
        eprintln!(
            "usage: {} <capture-path> <cargo-exit-status>",
            program.to_string_lossy()
        );
        return ExitCode::from(2);
    };
    let Some(status) = arguments.next() else {
        eprintln!(
            "usage: {} <capture-path> <cargo-exit-status>",
            program.to_string_lossy()
        );
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!(
            "usage: {} <capture-path> <cargo-exit-status>",
            program.to_string_lossy()
        );
        return ExitCode::from(2);
    }

    let status = match status.to_string_lossy().parse::<i32>() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("race classifier: invalid cargo exit status: {error}");
            return ExitCode::from(2);
        }
    };
    let output = match fs::read_to_string(&capture) {
        Ok(output) => output,
        Err(error) => {
            eprintln!(
                "race classifier: read {}: {error}",
                capture.to_string_lossy()
            );
            return ExitCode::from(2);
        }
    };

    println!("{}", classify(status, &output).describe());
    ExitCode::SUCCESS
}
