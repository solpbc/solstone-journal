// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;

use chrono::Utc;
use serde_json::{Map, json};
use solstone_core_brain::{
    InspectionStatus, brain_state_path, inspect_brain_state, read_journal_config,
    resolve_configured_journal,
};
use solstone_core_journal::{discover_home, read_config_journal};

const EXIT_USAGE: u8 = 64;

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_USAGE)
        }
        Err(CliError::Runtime(message)) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), CliError> {
    let journal = parse_arguments(arguments)?;
    let config = read_journal_config(&journal)
        .map_err(|error| CliError::Runtime(format!("failed to read journal config: {error}")))?
        .config
        .unwrap_or_else(Map::new);
    let inspection = inspect_brain_state(&journal, &config, Utc::now());
    let status = match inspection.status {
        InspectionStatus::Ok => "ok",
        InspectionStatus::Corrupt => "corrupt",
        InspectionStatus::Unavailable => "unavailable",
    };
    let projection = inspection.projection;
    let record_path = absolute_path(brain_state_path(&journal))?;
    let body = json!({
        "status": status,
        "record_path": record_path,
        "projection": {
            "aggregate_state": projection.aggregate_state,
            "reason_code": projection.reason_code,
            "active_lane": projection.active_lane,
            "active_provider": projection.active_provider,
            "active_model": projection.active_model,
            "fingerprint_sha256": projection.fingerprint_sha256,
            "runtime_transition_in_progress": projection.runtime_transition_in_progress,
        },
        "error": inspection.error,
    });
    println!(
        "{}",
        serde_json::to_string(&body)
            .map_err(|error| CliError::Runtime(format!("failed to encode inspection: {error}")))?
    );
    Ok(())
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<PathBuf, CliError> {
    let mut arguments = arguments.into_iter();
    let Some(verb) = arguments.next() else {
        return Err(usage("missing inspect verb"));
    };
    if verb != "inspect" {
        return Err(usage("expected inspect verb"));
    }
    let mut journal = None;
    while let Some(argument) = arguments.next() {
        if argument == "--journal" {
            let Some(path) = arguments.next() else {
                return Err(usage("--journal requires a path"));
            };
            if journal.replace(PathBuf::from(path)).is_some() {
                return Err(usage("--journal was provided more than once"));
            }
        } else {
            return Err(usage(&format!(
                "unknown argument: {}",
                argument.to_string_lossy()
            )));
        }
    }
    journal.map_or_else(resolve_default_journal, Ok)
}

fn resolve_default_journal() -> Result<PathBuf, CliError> {
    let env_journal = env::var_os("SOLSTONE_JOURNAL");
    if let Some(path) = env_journal
        .as_deref()
        .filter(|value| *value != OsStr::new(""))
    {
        return Ok(PathBuf::from(path));
    }
    let home = discover_binary_home()?;
    let config_journal = read_config_journal(&home)
        .map_err(|_| CliError::Runtime("journal config is not valid UTF-8".to_owned()))?;
    Ok(resolve_configured_journal(
        env_journal.as_deref(),
        config_journal.as_deref(),
        None,
        &home,
    )
    .path)
}

fn discover_binary_home() -> Result<PathBuf, CliError> {
    let home_env = env::var_os("HOME");
    if let Some(home) = home_env.as_deref() {
        return discover_home(Some(home), None)
            .map_err(|_| CliError::Runtime("could not determine home directory".to_owned()));
    }
    let fallback = env::home_dir();
    discover_home(None, fallback.as_deref())
        .map_err(|_| CliError::Runtime("could not determine home directory".to_owned()))
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        return Ok(path);
    }
    env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| CliError::Runtime(format!("failed to resolve record path: {error}")))
}

enum CliError {
    Usage(String),
    Runtime(String),
}

fn usage(message: &str) -> CliError {
    CliError::Usage(format!(
        "{message}\nUsage: solstone-brain inspect [--journal <path>]"
    ))
}
