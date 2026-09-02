// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use solstone_core_journal::{discover_home, read_config_journal, resolve_journal_path};
use solstone_core_transcribe::run_cli;

fn main() -> ExitCode {
    let journal = match resolve_journal() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let mut on_day = |day: &Path| {
        println!(
            "{}",
            day.file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default()
        );
    };
    match run_cli(env::args().skip(1), &journal, &mut on_day) {
        Ok(result) => {
            if let Some(summary) = result.summary {
                println!("{summary}");
            }
            if let Some(stderr) = result.stderr {
                eprint!("{stderr}");
            }
            ExitCode::from(result.exit_code as u8)
        }
        Err(error) => {
            if let Some(message) = error.message() {
                eprintln!("{message}");
            } else {
                eprintln!("{error}");
            }
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn resolve_journal() -> Result<PathBuf, String> {
    let home = discover_binary_home()?;
    let configured =
        read_config_journal(&home).map_err(|_| "journal config is not valid UTF-8".to_owned())?;
    Ok(resolve_journal_path(
        env::var_os("SOLSTONE_JOURNAL").as_deref(),
        configured.as_deref(),
        None,
        &home,
    )
    .path)
}

fn discover_binary_home() -> Result<PathBuf, String> {
    if let Some(home) = env::var_os("HOME") {
        return discover_home(Some(&home), None)
            .map_err(|_| "could not determine home directory".to_owned());
    }
    let fallback = env::home_dir();
    discover_home(None, fallback.as_deref())
        .map_err(|_| "could not determine home directory".to_owned())
}
