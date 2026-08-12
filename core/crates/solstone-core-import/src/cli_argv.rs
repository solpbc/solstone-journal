// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal importer argv parsing and dispatch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::NaiveDateTime;
use serde_json::json;
use solstone_core_segment::{
    SUPERVISOR_MESSAGE, SupervisorRefusal, require_solstone, require_solstone_with,
};

use crate::cli_journal_source;
use crate::cli_render;
use crate::connect::{OuraConnectRequest, connect_oura};
use crate::detect::OURA_SYNC_REMEDY;

/// Observable result of a library-hosted journal importer invocation.
#[derive(Debug, Eq, PartialEq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run the importer grammar with the process environment and local supervisor probe.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    let parsed = match parse_arguments(args) {
        Ok(ParsedCommand::Help) => return success(cli_render::HELP.to_owned()),
        Ok(parsed) => parsed,
        Err(arguments) => return argparse_error(arguments),
    };
    run_after_parse(parsed, journal_path, || require_solstone(journal_path))
}

/// Run the importer grammar with injectable environment and supervisor seams.
pub fn run_cli_with<E, C>(
    args: &[String],
    journal_path: &Path,
    lookup_env: E,
    connectivity: C,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    let parsed = match parse_arguments(args) {
        Ok(ParsedCommand::Help) => return success(cli_render::HELP.to_owned()),
        Ok(parsed) => parsed,
        Err(arguments) => return argparse_error(arguments),
    };
    run_after_parse(parsed, journal_path, || {
        require_solstone_with(lookup_env, connectivity)
    })
}

fn run_after_parse(
    parsed: ParsedCommand,
    journal_path: &Path,
    preflight: impl FnOnce() -> Result<(), SupervisorRefusal>,
) -> CliRun {
    match preflight() {
        Ok(()) => {}
        Err(SupervisorRefusal::SpawnedUnavailable) => return failure("", "", 75),
        Err(SupervisorRefusal::Unavailable) => {
            return failure("", &format!("{SUPERVISOR_MESSAGE}\n"), 1);
        }
    }

    match parsed {
        ParsedCommand::Help => unreachable!("help returns before supervisor preflight"),
        ParsedCommand::ListImporters { json } => success(cli_render::importers(json)),
        ParsedCommand::Backends => success(cli_render::backends()),
        ParsedCommand::Connect { backend } => run_connect(&backend, journal_path),
        ParsedCommand::Sync { backend, options } => run_sync(&backend, &options, journal_path),
        ParsedCommand::Import(options) => run_import(options, journal_path),
    }
}

fn run_connect(backend: &str, journal_path: &Path) -> CliRun {
    if backend != "oura" {
        return failure(
            "",
            &format!("Unknown connect backend: {backend}\nConnectable backends: oura\n"),
            1,
        );
    }
    match connect_oura(&OuraConnectRequest {
        journal_root: journal_path.to_path_buf(),
        timeout_seconds: 300,
    }) {
        Ok(outcome) => success(format!(
            "Oura authorization saved to journal config.\nAuthorized scopes: {}\n",
            outcome.report.scopes().join(" ")
        )),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn run_sync(backend: &str, options: &Options, journal_path: &Path) -> CliRun {
    match backend {
        "oura" => {
            let result = solstone_core_body_ingest::sync_oura(
                journal_path,
                &solstone_core_body_ingest::OuraSyncOptions {
                    save: options.save,
                    confirm_body_save: options.confirm_body_save,
                    scheduled: options.scheduled,
                    window_days: options.window_days,
                    today: None,
                },
            );
            match result {
                Ok(report) => success(format!(
                    "Oura body sync {}: rows={} days={} pages={}\n",
                    if options.save { "complete" } else { "preview" },
                    report.rows(),
                    report.days().len(),
                    report.pages()
                )),
                Err(error) => failure("", &format!("{error}\n"), 1),
            }
        }
        "plaud" | "obsidian" | "audio" => success(format!(
            "Syncing {backend} ({} mode)...\n",
            if options.save { "save" } else { "catalog" }
        )),
        _ => failure(
            "",
            &format!(
                "Unknown sync backend: {backend}\nAvailable backends: plaud, obsidian, audio, oura\n"
            ),
            1,
        ),
    }
}

fn run_import(options: Options, journal_path: &Path) -> CliRun {
    let Some(media) = options.media.as_deref() else {
        return argparse_error("the following arguments are required: media".to_owned());
    };
    if media == "journal-source" {
        return cli_journal_source::run_cli(&options.extra, journal_path);
    }
    let source = options.source.as_deref();
    if matches!(source, Some("chatgpt" | "claude" | "gemini" | "kindle")) {
        return failure(
            "",
            &format!(
                "import-sources: unimplemented: {}\n",
                source.expect("matched source")
            ),
            1,
        );
    }
    if source == Some("oura") {
        return failure("", &format!("{OURA_SYNC_REMEDY}\n"), 1);
    }
    if source == Some("apple_health") {
        return run_apple(media, &options, journal_path);
    }
    if let Some(source) = source {
        return success(format!("Import preview ready: {source}\n"));
    }
    if Path::new(media)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("m4a"))
    {
        return run_audio(media, &options, journal_path);
    }
    failure(
        "",
        "generic text import requires the native text dispatch adapter\n",
        1,
    )
}

fn run_audio(media: &str, options: &Options, journal_path: &Path) -> CliRun {
    if options.dry_run {
        return success("Audio import preview: no journal writes requested.\n".to_owned());
    }
    let Some(timestamp) = options.timestamp.as_deref() else {
        return failure("", "timestamp must be YYYYMMDD_HHMMSS format\n", 1);
    };
    let base_timestamp = match NaiveDateTime::parse_from_str(timestamp, "%Y%m%d_%H%M%S") {
        Ok(value) => value,
        Err(_) => return failure("", "timestamp must be YYYYMMDD_HHMMSS format\n", 1),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return failure("", &format!("audio import runtime failed: {error}\n"), 1),
    };
    let request = crate::AudioImportRequest {
        source_media: PathBuf::from(media),
        journal_root: journal_path.to_path_buf(),
        day: timestamp[..8].to_owned(),
        base_timestamp,
        import_id: timestamp.to_owned(),
        stream: "import.audio".to_owned(),
        facet: None,
        setting: None,
        wait_for_processing: false,
        stall_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(250),
    };
    match runtime.block_on(crate::import_audio(request)) {
        Ok(outcome) => success(format!("Generic audio import complete: {outcome:?}\n")),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

fn run_apple(media: &str, options: &Options, journal_path: &Path) -> CliRun {
    let result = if options.dry_run {
        solstone_core_body_ingest::preview_apple(
            Path::new(media),
            options.date_from.as_deref(),
            options.date_to.as_deref(),
        )
    } else {
        solstone_core_body_ingest::save_apple(
            Path::new(media),
            journal_path,
            &solstone_core_body_ingest::AppleImportOptions {
                date_from: options.date_from.clone(),
                date_to: options.date_to.clone(),
                confirm_body_save: options.confirm_body_save,
                force: options.force,
            },
        )
    };
    match result {
        Ok(report) if options.json => success(format!(
            "{}\n",
            json!({"schema":"solstone.body.ingest.result.v1", "source":"apple_health", "mode": if options.dry_run { "preview" } else { "save" }, "bundle_id":report.bundle_id(), "rows":report.rows(), "days":report.days(), "skipped":report.skipped()})
        )),
        Ok(report) => success(format!(
            "Apple Health {} complete.\n  Rows:                {}\n  Days:                {}\n",
            if options.dry_run { "preview" } else { "save" },
            report.rows(),
            report.days().len()
        )),
        Err(error) => failure("", &format!("{error}\n"), 1),
    }
}

#[derive(Default)]
struct Options {
    media: Option<String>,
    timestamp: Option<String>,
    source: Option<String>,
    sync: Option<String>,
    connect: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    path: Option<PathBuf>,
    window_days: Option<u64>,
    force: bool,
    dry_run: bool,
    confirm_body_save: bool,
    save: bool,
    scheduled: bool,
    list_importers: bool,
    backends: bool,
    json: bool,
    extra: Vec<String>,
}

enum ParsedCommand {
    Help,
    ListImporters { json: bool },
    Backends,
    Connect { backend: String },
    Sync { backend: String, options: Options },
    Import(Options),
}

fn parse_arguments(args: &[String]) -> Result<ParsedCommand, String> {
    let mut options = Options::default();
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "-h" || argument == "--help" {
            return Ok(ParsedCommand::Help);
        }
        if argument == "--" {
            positionals.extend(args[index + 1..].iter().cloned());
            break;
        }
        if let Some((name, value)) = argument.split_once('=') {
            assign_value(&mut options, name, value)?;
        } else if takes_value(argument) {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("argument {argument}: expected one argument"))?;
            assign_value(&mut options, argument, value)?;
            index += 1;
        } else if argument == "--auto" {
            if args
                .get(index + 1)
                .is_some_and(|value| !value.starts_with('-'))
            {
                index += 1;
            }
        } else if assign_flag(&mut options, argument) {
        } else if argument.starts_with('-') {
            return Err(format!("unrecognized arguments: {argument}"));
        } else {
            positionals.push(argument.clone());
        }
        index += 1;
    }
    if positionals
        .first()
        .is_some_and(|value| value == "journal-source")
    {
        options.media = Some("journal-source".to_owned());
        options.extra = positionals.into_iter().skip(1).collect();
        return Ok(ParsedCommand::Import(options));
    }
    match positionals.as_slice() {
        [] => {}
        [media] => options.media = Some(media.clone()),
        [media, timestamp] => {
            options.media = Some(media.clone());
            options.timestamp = Some(timestamp.clone());
        }
        [media, timestamp, extras @ ..] => {
            options.media = Some(media.clone());
            options.timestamp = Some(timestamp.clone());
            return Err(format!("unrecognized arguments: {}", extras.join(" ")));
        }
    }
    if options.list_importers {
        return Ok(ParsedCommand::ListImporters { json: options.json });
    }
    if options.backends {
        return Ok(ParsedCommand::Backends);
    }
    if let Some(backend) = options.connect.clone() {
        return Ok(ParsedCommand::Connect { backend });
    }
    if let Some(backend) = options.sync.clone() {
        return Ok(ParsedCommand::Sync { backend, options });
    }
    Ok(ParsedCommand::Import(options))
}

fn takes_value(argument: &str) -> bool {
    matches!(
        argument,
        "--timestamp"
            | "--facet"
            | "--setting"
            | "--source"
            | "--sync"
            | "--path"
            | "--window-days"
            | "--connect"
            | "--date-from"
            | "--date-to"
    )
}

fn assign_value(options: &mut Options, name: &str, value: &str) -> Result<(), String> {
    match name {
        "--timestamp" => options.timestamp = Some(value.to_owned()),
        "--source" => options.source = Some(value.to_owned()),
        "--sync" => options.sync = Some(value.to_owned()),
        "--connect" => options.connect = Some(value.to_owned()),
        "--date-from" => options.date_from = Some(value.to_owned()),
        "--date-to" => options.date_to = Some(value.to_owned()),
        "--path" => options.path = Some(PathBuf::from(value)),
        "--window-days" => {
            options.window_days = Some(
                value
                    .parse()
                    .map_err(|_| format!("argument --window-days: invalid int value: '{value}'"))?,
            )
        }
        "--facet" | "--setting" => {}
        _ => return Err(format!("unrecognized arguments: {name}={value}")),
    }
    Ok(())
}

fn assign_flag(options: &mut Options, argument: &str) -> bool {
    match argument {
        "--force" => options.force = true,
        "--dry-run" => options.dry_run = true,
        "--confirm-body-save" | "--confirm-health-save" => options.confirm_body_save = true,
        "--with-day-summaries" | "--deterministic-only" | "-v" | "--verbose" | "-d" | "--debug" => {
        }
        "--backends" => options.backends = true,
        "--save" => options.save = true,
        "--scheduled" => options.scheduled = true,
        "--list-importers" => options.list_importers = true,
        "--json" => options.json = true,
        _ => return false,
    }
    true
}

fn argparse_error(arguments: String) -> CliRun {
    failure(
        "",
        &format!(
            "usage: journal importer [-h] [options] media [timestamp]\njournal importer: error: {arguments}\n"
        ),
        2,
    )
}
fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}
fn failure(stdout: &str, stderr: &str, exit_code: i32) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
    }
}
