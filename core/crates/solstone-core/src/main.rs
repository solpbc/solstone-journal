// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::ExitCode;
use std::{env, ffi::OsStr, path::PathBuf};

use chrono::Local;
use solstone_core_cli::{
    Command, IndexerCommand, IndexerCountsOptions, IndexerOptions, IndexerQueryOptions,
    IndexerReadOptions, IndexerSearchOptions, JournalPathOptions, ServiceOptions, SplCommand,
    USAGE, evaluate_args, version_line,
};
use solstone_core_indexer_query::{
    IndexAccessError, Order, SearchRequest, agents, coverage, search, search_counts,
};
use solstone_core_indexer_store::db::reset_index;
use solstone_core_indexer_store::scan::{
    RescanFileStatus, rebuild_edges, rescan_file, scan_journal,
};
use solstone_core_journal::{
    ConfigError, HomeError, Source, discover_home, ensure_journal_dir_with_label,
    read_config_journal, resolve_journal_path,
};

const EXIT_USAGE: u8 = 64;
const EXIT_UNAVAILABLE: u8 = 69;
const EXIT_TEMPFAIL: u8 = 75;
const ZERO_EDGE_HINT: &str = "Zero edges indexed: edges are talent-derived, and the --rescan-full edge phase remains modification-time incremental — run journal indexer --rebuild-edges to force full edge re-extraction.";
const SOL_IDENTITY_TOKEN: &str = "__solstone_identity=sol";
const SOLSTONE_IDENTITY_TOKEN: &str = "__solstone_identity=solstone";

struct JournalPathLine {
    label: &'static str,
    path: PathBuf,
}

enum JournalPathError {
    Config(ConfigError),
    Home(HomeError),
    Create(solstone_core_journal::EnsureJournalDirError),
}

fn main() -> ExitCode {
    let mut args: Vec<_> = env::args_os().skip(1).collect();
    if let Some(identity) = sol_identity_from_first_arg(&args) {
        args.remove(0);
        return solstone_core_sol::run(identity, args);
    }
    match evaluate_args(&args) {
        Ok(Command::Version) => {
            print!("{}", version_line(env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Ok(Command::JournalPath(options)) => match run_journal_path(options) {
            Ok(line) => {
                println!("{}\t{}", line.label, line.path.display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprint_journal_path_error(error);
                ExitCode::from(EXIT_TEMPFAIL)
            }
        },
        Ok(Command::Indexer(command)) => run_indexer(*command),
        Ok(Command::Spl(command)) => run_spl_process(command),
        Err(_) => {
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn run_spl_process(command: SplCommand) -> ExitCode {
    match command {
        SplCommand::Service(options) => run_spl_service(options),
    }
}

fn run_spl_service(options: ServiceOptions) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let verbosity = solstone_core_spl::Verbosity::from_flags(options.verbose, options.debug);
    match solstone_core_spl::run_native_service(journal.path, verbosity) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spl service failed: {}", error.class());
            ExitCode::from(EXIT_TEMPFAIL)
        }
    }
}

fn sol_identity_from_first_arg(args: &[std::ffi::OsString]) -> Option<&'static str> {
    match args.first().and_then(|arg| arg.to_str()) {
        Some(SOL_IDENTITY_TOKEN) => Some("sol"),
        Some(SOLSTONE_IDENTITY_TOKEN) => Some("solstone"),
        _ => None,
    }
}

fn run_journal_path(options: JournalPathOptions) -> Result<JournalPathLine, JournalPathError> {
    let line = if let Some(path) = options.journal_override {
        JournalPathLine {
            label: "cli",
            path: PathBuf::from(path),
        }
    } else {
        resolve_process_journal_path()?
    };

    if options.create {
        ensure_journal_dir_with_label(&line.path, line.label).map_err(JournalPathError::Create)?;
    }

    Ok(line)
}

fn run_indexer(command: IndexerCommand) -> ExitCode {
    match command {
        IndexerCommand::Maintenance(options) => run_indexer_maintenance(options),
        IndexerCommand::Search(options) => run_indexer_search(options),
        IndexerCommand::Counts(options) => run_indexer_counts(options),
        IndexerCommand::Agents(options) => run_indexer_agents(options),
        IndexerCommand::Coverage(options) => run_indexer_coverage(options),
    }
}

fn run_indexer_maintenance(options: IndexerOptions) -> ExitCode {
    if !options.reset
        && !options.rebuild_edges
        && !options.rescan
        && !options.rescan_full
        && options.rescan_file.is_none()
    {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let line = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };

    if options.reset
        && let Err(error) = reset_index(&line.path)
    {
        eprintln!("indexer reset failed: {error}");
        return ExitCode::from(EXIT_TEMPFAIL);
    }

    if options.rebuild_edges {
        match rebuild_edges(&line.path) {
            Ok(report) => {
                for warning in report.warnings {
                    eprintln!("warning: {warning}");
                }
                if report.failed > 0 {
                    return ExitCode::from(EXIT_TEMPFAIL);
                }
            }
            Err(error) => {
                eprintln!("indexer edge rebuild failed: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    }

    if let Some(path) = options.rescan_file {
        match rescan_file(&line.path, &PathBuf::from(path)) {
            Ok(RescanFileStatus::Indexed { warnings }) => {
                for warning in warnings {
                    eprintln!("warning: {warning}");
                }
                return ExitCode::SUCCESS;
            }
            Ok(RescanFileStatus::Declined) => {
                eprintln!("indexer declined unsupported file");
                return ExitCode::from(EXIT_UNAVAILABLE);
            }
            Err(error) => {
                eprintln!("indexer rescan-file failed: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    }

    if options.rescan || options.rescan_full {
        match scan_journal(&line.path, options.rescan_full) {
            Ok(report) => {
                for warning in report.warnings {
                    eprintln!("warning: {warning}");
                }
                let should_emit_zero_edge_hint = options.rescan_full
                    && !options.rebuild_edges
                    && !options.reset
                    && report.edge_rows_inserted == 0;
                if should_emit_zero_edge_hint {
                    println!("{ZERO_EDGE_HINT}");
                }
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("indexer scan failed: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    }

    ExitCode::SUCCESS
}

fn run_indexer_search(options: IndexerSearchOptions) -> ExitCode {
    let json = options.query.json;
    let request = match search_request(
        &options.query,
        options.limit,
        options.offset,
        options.counts,
        &options.order,
    ) {
        Ok(request) => request,
        Err(()) => return print_usage_error(),
    };
    let journal = match resolve_indexer_journal_path(options.query.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match search(&journal, &request, Local::now().date_naive()) {
        Ok(response) => {
            if json {
                print_json(&response);
            } else {
                println!("{} result(s)", response.results.len());
                for hit in response.results {
                    println!("{}\t{}", hit.id, hit.text);
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, json),
    }
}

fn run_indexer_counts(options: IndexerCountsOptions) -> ExitCode {
    let json = options.query.json;
    let request = match search_request(&options.query, 10, 0, false, "relevance") {
        Ok(request) => request,
        Err(()) => return print_usage_error(),
    };
    let journal = match resolve_indexer_journal_path(options.query.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match search_counts(&journal, &request, Local::now().date_naive()) {
        Ok(response) => {
            if json {
                print_json(&response);
            } else {
                println!("{} matching chunk(s)", response.total);
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, json),
    }
}

fn run_indexer_agents(options: IndexerReadOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match agents(&journal) {
        Ok(response) => {
            if options.json {
                print_json(&response);
            } else {
                for agent in response {
                    println!("{agent}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, options.json),
    }
}

fn run_indexer_coverage(options: IndexerReadOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match coverage(&journal) {
        Ok(response) => {
            if options.json {
                print_json(&response);
            } else if let (Some(start), Some(end)) = (response.start, response.end) {
                println!("available: {start} through {end}");
            } else {
                println!("no dated chunks");
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_index_access_error(error, options.json),
    }
}

fn search_request(
    options: &IndexerQueryOptions,
    limit: usize,
    offset: usize,
    counts: bool,
    order: &str,
) -> Result<SearchRequest, ()> {
    let order = match order {
        "relevance" => Order::Relevance,
        "recency" => Order::Recency,
        "reranked" => Order::Reranked,
        _ => return Err(()),
    };
    let mut request =
        SearchRequest::new(options.query.clone().unwrap_or_default(), order).map_err(|_| ())?;
    request.limit = limit;
    request.offset = offset;
    request.day = options.day.clone();
    request.day_from = options.day_from.clone();
    request.day_to = options.day_to.clone();
    request.facet = options.facet.clone();
    request.agent = options.agent.clone();
    request.stream = options.stream.clone();
    request.time_bucket = options.time_bucket.clone();
    request.relax = options.relax;
    request.counts = counts;
    Ok(request)
}

fn resolve_indexer_journal_path(
    journal_override: Option<std::ffi::OsString>,
) -> Result<JournalPathLine, JournalPathError> {
    if let Some(path) = journal_override {
        Ok(JournalPathLine {
            label: "cli",
            path: PathBuf::from(path),
        })
    } else {
        resolve_process_journal_path()
    }
}

fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string(value).expect("query responses serialize to JSON")
    );
}

fn print_index_access_error(error: IndexAccessError, json: bool) -> ExitCode {
    if json {
        print_json(&serde_json::json!({
            "error": {"reason": error.reason(), "message": error.to_string()}
        }));
    } else {
        eprintln!("Error: {error}");
    }
    let exit = if error.reason() == "index_locked" {
        EXIT_TEMPFAIL
    } else {
        EXIT_UNAVAILABLE
    };
    ExitCode::from(exit)
}

fn print_usage_error() -> ExitCode {
    eprint!("{USAGE}");
    ExitCode::from(EXIT_USAGE)
}

fn print_journal_error(error: JournalPathError) -> ExitCode {
    eprint_journal_path_error(error);
    ExitCode::from(EXIT_TEMPFAIL)
}

fn resolve_process_journal_path() -> Result<JournalPathLine, JournalPathError> {
    let env_journal = env::var_os("SOLSTONE_JOURNAL");
    if let Some(path) = env_journal
        .as_deref()
        .filter(|value| *value != OsStr::new(""))
    {
        return Ok(JournalPathLine {
            label: "env",
            path: PathBuf::from(path),
        });
    }

    let home = discover_binary_home().map_err(JournalPathError::Home)?;
    let config_journal = read_config_journal(&home).map_err(JournalPathError::Config)?;
    let resolved = resolve_journal_path(
        env_journal.as_deref(),
        config_journal.as_deref(),
        None,
        &home,
    );
    Ok(JournalPathLine {
        label: match resolved.source {
            Source::Env => "env",
            Source::Config => "config",
            Source::Source => "source",
            Source::Default => "default",
        },
        path: resolved.path,
    })
}

fn discover_binary_home() -> Result<PathBuf, HomeError> {
    let home_env = env::var_os("HOME");
    if let Some(home) = home_env.as_deref() {
        return discover_home(Some(home), None);
    }
    let fallback = env::home_dir();
    discover_home(None, fallback.as_deref())
}

fn eprint_journal_path_error(error: JournalPathError) {
    match error {
        JournalPathError::Config(ConfigError::Decode) => {
            eprintln!("journal-path failed: config is not valid UTF-8")
        }
        JournalPathError::Home(HomeError::Unavailable) => {
            eprintln!("journal-path failed: could not determine home directory")
        }
        JournalPathError::Create(error) => eprintln!("{error}"),
    }
}
