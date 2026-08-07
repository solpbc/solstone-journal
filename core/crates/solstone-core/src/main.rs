// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::Duration;
use std::{env, ffi::OsStr, path::PathBuf};

use chrono::Local;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use solstone_core_cli::{
    BrainCommand, BrainRuntimeFailureOptions, Command, IndexerCommand, IndexerCountsOptions,
    IndexerOptions, IndexerQueryOptions, IndexerReadOptions, IndexerSearchOptions,
    JournalConfigCommand, JournalConfigCommitOptions, JournalConfigExpectArg,
    JournalConfigReadOptions, JournalPathOptions, LocalCommand, ServiceOptions, SplCommand, USAGE,
    evaluate_args, version_line,
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
use solstone_core_journal_config::{materialized_defaults, read_journal_config};
use solstone_core_journal_config_write::{
    CommitConfigError, ConfigExpectation, LockError, LockOptions, commit_journal_config,
};

const EXIT_USAGE: u8 = 64;
const EXIT_UNAVAILABLE: u8 = 69;
const EXIT_TEMPFAIL: u8 = 75;
const EXIT_DATAERR: u8 = 65;
const EXIT_CANTCREAT: u8 = 73;
const EXIT_IOERR: u8 = 74;
const MAX_JSON_STDIN_BYTES: usize = 1024 * 1024;
const MAX_LOCAL_STDIN_BYTES: usize = 1024 * 1024;
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
        Ok(Command::JournalConfig(command)) => run_journal_config(command),
        Ok(Command::Local(command)) => run_local(command),
        Ok(Command::Brain(command)) => run_brain(command),
        Ok(Command::Spl(command)) => run_spl_process(command),
        Err(_) => {
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn run_local(command: LocalCommand) -> ExitCode {
    match command {
        LocalCommand::ProbeNvidia => {
            let probe = solstone_core_local::probe_nvidia_gpu();
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            if serde_json::to_writer(&mut stdout, &probe).is_err() || writeln!(stdout).is_err() {
                return ExitCode::from(EXIT_IOERR);
            }
            ExitCode::SUCCESS
        }
        LocalCommand::Plan => run_local_json(solstone_core_local::plan),
        LocalCommand::Connect => run_local_json(solstone_core_local::connect),
    }
}

fn run_local_json<T, O>(operation: impl FnOnce(T) -> O) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    let input = match read_local_stdin() {
        Ok(input) => input,
        Err(LocalStdinError::Content) => {
            eprintln!("local command failed: stdin was not valid JSON within 1 MiB");
            return ExitCode::from(EXIT_USAGE);
        }
        Err(LocalStdinError::Io) => {
            eprintln!("local command failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &operation(input)).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

enum LocalStdinError {
    Content,
    Io,
}

fn read_local_stdin<T: DeserializeOwned>() -> Result<T, LocalStdinError> {
    let mut stdin = io::stdin().lock();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin.read(&mut chunk).map_err(|_| LocalStdinError::Io)?;
        if read == 0 {
            break;
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|next| next > MAX_LOCAL_STDIN_BYTES)
        {
            return Err(LocalStdinError::Content);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    serde_json::from_slice(&bytes).map_err(|_| LocalStdinError::Content)
}

fn run_journal_config(command: JournalConfigCommand) -> ExitCode {
    match command {
        JournalConfigCommand::Read(options) => run_journal_config_read(options),
        JournalConfigCommand::Commit(options) => run_journal_config_commit(options),
    }
}

fn run_brain(command: BrainCommand) -> ExitCode {
    match command {
        BrainCommand::RecordRuntimeFailure(options) => run_brain_runtime_failure(options),
        BrainCommand::RefreshSession(_) | BrainCommand::PrerequisiteRenewalSession(_) => {
            eprintln!("brain session verbs are not yet implemented");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}

struct RuntimeFailureRequest {
    reason_code: String,
    component: String,
    expected_fingerprint_sha256: String,
    diagnostic: Map<String, Value>,
    bundled_runtime_fingerprint_sha256: Option<String>,
}

fn run_brain_runtime_failure(options: BrainRuntimeFailureOptions) -> ExitCode {
    let request = match read_bounded_json_stdin().and_then(parse_runtime_failure_request) {
        Ok(request) => request,
        Err(JsonStdinError::Content) => {
            eprintln!(
                "brain record-runtime-failure failed: stdin was not a valid request JSON object within 1 MiB"
            );
            return ExitCode::from(EXIT_USAGE);
        }
        Err(JsonStdinError::Io) => {
            eprintln!("brain record-runtime-failure failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let result = solstone_core_brain::record_runtime_failure(
        &line.path,
        &request.reason_code,
        &request.component,
        &request.expected_fingerprint_sha256,
        request.diagnostic,
        chrono::Utc::now(),
        request.bundled_runtime_fingerprint_sha256,
    );
    let output = json!({
        "accepted": result.accepted,
        "record": result.record,
        "rejected_reason": result.rejected_reason,
        "error": result.error,
    });
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err() || stdout.write_all(b"\n").is_err() {
        eprintln!("brain record-runtime-failure failed: stdout I/O error");
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

fn parse_runtime_failure_request(
    request: Map<String, Value>,
) -> Result<RuntimeFailureRequest, JsonStdinError> {
    const FIELDS: [&str; 5] = [
        "reason_code",
        "component",
        "expected_fingerprint_sha256",
        "diagnostic",
        "bundled_runtime_fingerprint_sha256",
    ];
    if request.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err(JsonStdinError::Content);
    }
    let string = |field: &str| {
        request
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or(JsonStdinError::Content)
    };
    let diagnostic = match request.get("diagnostic") {
        None => Map::new(),
        Some(Value::Object(value)) => value.clone(),
        Some(_) => return Err(JsonStdinError::Content),
    };
    let bundled_runtime_fingerprint_sha256 = match request.get("bundled_runtime_fingerprint_sha256")
    {
        None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) => return Err(JsonStdinError::Content),
    };
    Ok(RuntimeFailureRequest {
        reason_code: string("reason_code")?,
        component: string("component")?,
        expected_fingerprint_sha256: string("expected_fingerprint_sha256")?,
        diagnostic,
        bundled_runtime_fingerprint_sha256,
    })
}

fn run_journal_config_read(options: JournalConfigReadOptions) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let read = match read_journal_config(&line.path) {
        Ok(read) => read,
        Err(error) => {
            eprintln!("journal-config read failed: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    let config = read.config.unwrap_or_else(materialized_defaults);
    let output = json!({
        "present": read.present,
        "sha256": read.sha256,
        "config": config,
    });
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err() || stdout.write_all(b"\n").is_err() {
        eprintln!("journal-config read failed: stdout I/O error");
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

fn run_journal_config_commit(options: JournalConfigCommitOptions) -> ExitCode {
    let replacement = match read_journal_config_stdin() {
        Ok(replacement) => replacement,
        Err(JsonStdinError::Content) => {
            eprintln!(
                "journal-config commit failed: stdin was not a valid JSON object within 1 MiB"
            );
            return ExitCode::from(EXIT_USAGE);
        }
        Err(JsonStdinError::Io) => {
            eprintln!("journal-config commit failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let expectation = match options.expect {
        JournalConfigExpectArg::Absent => ConfigExpectation::Absent,
        JournalConfigExpectArg::Sha256(fingerprint) => ConfigExpectation::Sha256(fingerprint),
    };
    let lock_options = LockOptions {
        timeout: options
            .lock_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| LockOptions::default().timeout),
        ..LockOptions::default()
    };
    match commit_journal_config(&line.path, expectation, &replacement, lock_options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let exit = commit_config_error_exit(&error);
            eprintln!("journal-config commit failed: {error}");
            ExitCode::from(exit)
        }
    }
}

fn resolve_journal_config_path(
    journal_override: Option<std::ffi::OsString>,
) -> Result<JournalPathLine, JournalPathError> {
    match journal_override {
        Some(path) => Ok(JournalPathLine {
            label: "cli",
            path: PathBuf::from(path),
        }),
        None => resolve_process_journal_path(),
    }
}

enum JsonStdinError {
    Content,
    Io,
}

fn read_journal_config_stdin() -> Result<Map<String, Value>, JsonStdinError> {
    read_bounded_json_stdin()
}

fn read_bounded_json_stdin() -> Result<Map<String, Value>, JsonStdinError> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin.read(&mut chunk).map_err(|_| JsonStdinError::Io)?;
        if read == 0 {
            break;
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|next| next > MAX_JSON_STDIN_BYTES)
        {
            return Err(JsonStdinError::Content);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let config = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or(JsonStdinError::Content)?;
    drop(bytes);
    Ok(config)
}

fn commit_config_error_exit(error: &CommitConfigError) -> u8 {
    match error {
        CommitConfigError::Conflict(_) => EXIT_DATAERR,
        CommitConfigError::Load(_) => EXIT_UNAVAILABLE,
        CommitConfigError::Write(_) => EXIT_CANTCREAT,
        CommitConfigError::Lock(LockError::Io { .. }) => EXIT_IOERR,
        CommitConfigError::Lock(LockError::Timeout(_)) => EXIT_TEMPFAIL,
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

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    use solstone_core_journal_config_write::{
        AtomicWriteError, ConfigConflict, ConfigExpectation, ConfigFingerprint, LockTimeout,
    };

    use super::*;

    fn test_path() -> PathBuf {
        PathBuf::from("/tmp/journal-config-test")
    }

    #[test]
    fn exit_code_constants_match_the_documented_contract() {
        assert_eq!(EXIT_USAGE, 64);
        assert_eq!(EXIT_DATAERR, 65);
        assert_eq!(EXIT_UNAVAILABLE, 69);
        assert_eq!(EXIT_CANTCREAT, 73);
        assert_eq!(EXIT_IOERR, 74);
        assert_eq!(EXIT_TEMPFAIL, 75);
    }

    #[test]
    fn commit_config_error_exit_maps_all_variants() {
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Conflict(ConfigConflict {
                expected: ConfigExpectation::Absent,
                actual: ConfigFingerprint::Absent,
            })),
            EXIT_DATAERR
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Load(
                solstone_core_journal_config::ConfigLoadError::Corrupt {
                    path: test_path(),
                    source: Box::new(io::Error::other("test")),
                },
            )),
            EXIT_UNAVAILABLE
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Write(AtomicWriteError::Io {
                path: test_path(),
                source: io::Error::other("test"),
            })),
            EXIT_CANTCREAT
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Lock(LockError::Io {
                path: test_path(),
                source: io::Error::other("test"),
            })),
            EXIT_IOERR
        );
        assert_eq!(
            commit_config_error_exit(&CommitConfigError::Lock(LockError::Timeout(LockTimeout {
                path: test_path(),
                timeout: Duration::from_millis(1),
            },))),
            EXIT_TEMPFAIL
        );
    }
}
