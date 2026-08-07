// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};

pub const USAGE: &str = "Usage:\n  solstone-core --version\n  solstone-core journal-path [--journal PATH] [--create]\n  solstone-core indexer [--journal PATH] [--reset] [--rebuild-edges] [--rescan | --rescan-full | --rescan-file PATH]\n  solstone-core indexer search [QUERY] [--journal PATH] [--json] [--limit N] [--offset N] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax] [--counts] [--order relevance|recency]\n  solstone-core indexer counts [QUERY] [--journal PATH] [--json] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax]\n  solstone-core indexer agents [--journal PATH] [--json]\n  solstone-core indexer coverage [--journal PATH] [--json]\n  solstone-core journal-config read [--journal PATH]\n  solstone-core journal-config commit [--journal PATH] [--lock-timeout-ms N] --expect <fingerprint|absent>\n  solstone-core local probe-nvidia\n  solstone-core local plan\n  solstone-core local connect\n  solstone-core brain refresh --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256 | --expect-absent] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain prerequisite-renewal --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain record-runtime-failure [--journal PATH]\n  solstone-core brain inspect [--journal PATH] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain fingerprint\n  solstone-core spl service [-v | --verbose] [-d | --debug]\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Version,
    JournalPath(JournalPathOptions),
    Indexer(Box<IndexerCommand>),
    JournalConfig(JournalConfigCommand),
    Local(LocalCommand),
    Brain(BrainCommand),
    Spl(SplCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    ProbeNvidia,
    Plan,
    Connect,
    Install(InstallCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallCommand {
    PinsLocal,
    PathsLocal,
    FingerprintLocal,
    FingerprintMlx,
    VerifySha256,
    CudaTrust,
    ManifestVulkan,
    ManifestCuda,
    ManifestModel,
    InspectLocal,
    InspectMlx,
    ProbeBinary,
    RunLocal,
    RunMlx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainCommand {
    RefreshSession(BrainRefreshSessionOptions),
    PrerequisiteRenewalSession(BrainPrerequisiteRenewalSessionOptions),
    RecordRuntimeFailure(BrainRuntimeFailureOptions),
    Inspect(BrainInspectOptions),
    Fingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainRefreshSessionOptions {
    pub journal_override: Option<OsString>,
    pub run_id: Option<String>,
    pub expect: Option<BrainRefreshExpectArg>,
    pub bundled_runtime_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainRefreshExpectArg {
    Absent,
    Sha256(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainPrerequisiteRenewalSessionOptions {
    pub journal_override: Option<OsString>,
    pub run_id: Option<String>,
    pub expected_fingerprint_sha256: Option<String>,
    pub bundled_runtime_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainRuntimeFailureOptions {
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainInspectOptions {
    pub journal_override: Option<OsString>,
    pub bundled_runtime_fingerprint_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalConfigCommand {
    Read(JournalConfigReadOptions),
    Commit(JournalConfigCommitOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalConfigReadOptions {
    pub journal_override: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalConfigCommitOptions {
    pub journal_override: Option<OsString>,
    pub lock_timeout_ms: Option<u64>,
    pub expect: JournalConfigExpectArg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalConfigExpectArg {
    Absent,
    Sha256(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplCommand {
    Service(ServiceOptions),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOptions {
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPathOptions {
    pub journal_override: Option<OsString>,
    pub create: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerOptions {
    pub journal_override: Option<OsString>,
    pub reset: bool,
    pub rebuild_edges: bool,
    pub rescan: bool,
    pub rescan_full: bool,
    pub rescan_file: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexerCommand {
    Maintenance(IndexerOptions),
    Search(IndexerSearchOptions),
    Counts(IndexerCountsOptions),
    Agents(IndexerReadOptions),
    Coverage(IndexerReadOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerQueryOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
    pub query: Option<String>,
    pub day: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub facet: Option<String>,
    pub agent: Option<String>,
    pub stream: Option<String>,
    pub time_bucket: Option<String>,
    pub relax: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerSearchOptions {
    pub query: IndexerQueryOptions,
    pub limit: usize,
    pub offset: usize,
    pub counts: bool,
    pub order: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerCountsOptions {
    pub query: IndexerQueryOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerReadOptions {
    pub journal_override: Option<OsString>,
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageError;

pub fn evaluate_args(args: &[OsString]) -> Result<Command, UsageError> {
    match args {
        [flag] if flag == OsStr::new("--version") => Ok(Command::Version),
        [command, rest @ ..] if command == OsStr::new("journal-path") => {
            parse_journal_path(rest).map(Command::JournalPath)
        }
        [command, rest @ ..] if command == OsStr::new("indexer") => {
            parse_indexer(rest).map(|command| Command::Indexer(Box::new(command)))
        }
        [command, rest @ ..] if command == OsStr::new("journal-config") => {
            parse_journal_config(rest).map(Command::JournalConfig)
        }
        [command, rest @ ..] if command == OsStr::new("local") => {
            parse_local(rest).map(Command::Local)
        }
        [command, rest @ ..] if command == OsStr::new("brain") => {
            parse_brain(rest).map(Command::Brain)
        }
        [command, rest @ ..] if command == OsStr::new("spl") => parse_spl(rest).map(Command::Spl),
        _ => Err(UsageError),
    }
}

fn parse_local(args: &[OsString]) -> Result<LocalCommand, UsageError> {
    match args {
        [command] if command == OsStr::new("probe-nvidia") => Ok(LocalCommand::ProbeNvidia),
        [command] if command == OsStr::new("plan") => Ok(LocalCommand::Plan),
        [command] if command == OsStr::new("connect") => Ok(LocalCommand::Connect),
        [command, rest @ ..] if command == OsStr::new("install") => {
            parse_local_install(rest).map(LocalCommand::Install)
        }
        _ => Err(UsageError),
    }
}

fn parse_local_install(args: &[OsString]) -> Result<InstallCommand, UsageError> {
    match args {
        [one, two] if one == OsStr::new("pins") && two == OsStr::new("local") => {
            Ok(InstallCommand::PinsLocal)
        }
        [one, two] if one == OsStr::new("paths") && two == OsStr::new("local") => {
            Ok(InstallCommand::PathsLocal)
        }
        [one, two] if one == OsStr::new("fingerprint") && two == OsStr::new("local") => {
            Ok(InstallCommand::FingerprintLocal)
        }
        [one, two] if one == OsStr::new("fingerprint") && two == OsStr::new("mlx") => {
            Ok(InstallCommand::FingerprintMlx)
        }
        [one, two] if one == OsStr::new("verify") && two == OsStr::new("sha256") => {
            Ok(InstallCommand::VerifySha256)
        }
        [one, two] if one == OsStr::new("cuda") && two == OsStr::new("trust") => {
            Ok(InstallCommand::CudaTrust)
        }
        [one, two] if one == OsStr::new("manifest") && two == OsStr::new("vulkan") => {
            Ok(InstallCommand::ManifestVulkan)
        }
        [one, two] if one == OsStr::new("manifest") && two == OsStr::new("cuda") => {
            Ok(InstallCommand::ManifestCuda)
        }
        [one, two] if one == OsStr::new("manifest") && two == OsStr::new("model") => {
            Ok(InstallCommand::ManifestModel)
        }
        [one, two] if one == OsStr::new("inspect") && two == OsStr::new("local") => {
            Ok(InstallCommand::InspectLocal)
        }
        [one, two] if one == OsStr::new("inspect") && two == OsStr::new("mlx") => {
            Ok(InstallCommand::InspectMlx)
        }
        [one] if one == OsStr::new("probe-binary") => Ok(InstallCommand::ProbeBinary),
        [one, two] if one == OsStr::new("run") && two == OsStr::new("local") => {
            Ok(InstallCommand::RunLocal)
        }
        [one, two] if one == OsStr::new("run") && two == OsStr::new("mlx") => {
            Ok(InstallCommand::RunMlx)
        }
        _ => Err(UsageError),
    }
}

fn parse_journal_config(args: &[OsString]) -> Result<JournalConfigCommand, UsageError> {
    match args {
        [command, rest @ ..] if command == OsStr::new("read") => {
            parse_journal_config_read(rest).map(JournalConfigCommand::Read)
        }
        [command, rest @ ..] if command == OsStr::new("commit") => {
            parse_journal_config_commit(rest).map(JournalConfigCommand::Commit)
        }
        _ => Err(UsageError),
    }
}

fn parse_brain(args: &[OsString]) -> Result<BrainCommand, UsageError> {
    match args {
        [command, rest @ ..] if command == OsStr::new("refresh") => {
            parse_brain_refresh_session(rest).map(BrainCommand::RefreshSession)
        }
        [command, rest @ ..] if command == OsStr::new("prerequisite-renewal") => {
            parse_brain_prerequisite_renewal_session(rest)
                .map(BrainCommand::PrerequisiteRenewalSession)
        }
        [command, rest @ ..] if command == OsStr::new("record-runtime-failure") => {
            parse_brain_runtime_failure(rest).map(BrainCommand::RecordRuntimeFailure)
        }
        [command, rest @ ..] if command == OsStr::new("inspect") => {
            parse_brain_inspect(rest).map(BrainCommand::Inspect)
        }
        [command, rest @ ..] if command == OsStr::new("fingerprint") => {
            parse_brain_fingerprint(rest).map(|()| BrainCommand::Fingerprint)
        }
        _ => Err(UsageError),
    }
}

fn parse_brain_refresh_session(
    args: &[OsString],
) -> Result<BrainRefreshSessionOptions, UsageError> {
    let mut journal_override = None;
    let mut run_id = None;
    let mut expect = None;
    let mut bundled_runtime_fingerprint_sha256 = None;
    let mut session = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--session") {
            if session {
                return Err(UsageError);
            }
            session = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            journal_override = Some(brain_os_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--run-id") {
            if run_id.is_some() {
                return Err(UsageError);
            }
            run_id = Some(brain_string_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect-fingerprint") {
            if expect.is_some() {
                return Err(UsageError);
            }
            expect = Some(BrainRefreshExpectArg::Sha256(brain_sha256_value(
                args, index,
            )?));
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect-absent") {
            if expect.is_some() {
                return Err(UsageError);
            }
            expect = Some(BrainRefreshExpectArg::Absent);
            index += 1;
            continue;
        }
        if arg == OsStr::new("--bundled-runtime-fingerprint") {
            if bundled_runtime_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            bundled_runtime_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    if !session {
        return Err(UsageError);
    }
    Ok(BrainRefreshSessionOptions {
        journal_override,
        run_id,
        expect,
        bundled_runtime_fingerprint_sha256,
    })
}

fn parse_brain_prerequisite_renewal_session(
    args: &[OsString],
) -> Result<BrainPrerequisiteRenewalSessionOptions, UsageError> {
    let mut journal_override = None;
    let mut run_id = None;
    let mut expected_fingerprint_sha256 = None;
    let mut bundled_runtime_fingerprint_sha256 = None;
    let mut session = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--session") {
            if session {
                return Err(UsageError);
            }
            session = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            journal_override = Some(brain_os_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--run-id") {
            if run_id.is_some() {
                return Err(UsageError);
            }
            run_id = Some(brain_string_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect-fingerprint") {
            if expected_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            expected_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--bundled-runtime-fingerprint") {
            if bundled_runtime_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            bundled_runtime_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    if !session {
        return Err(UsageError);
    }
    Ok(BrainPrerequisiteRenewalSessionOptions {
        journal_override,
        run_id,
        expected_fingerprint_sha256,
        bundled_runtime_fingerprint_sha256,
    })
}

fn parse_brain_runtime_failure(
    args: &[OsString],
) -> Result<BrainRuntimeFailureOptions, UsageError> {
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        if args[index].as_os_str() != OsStr::new("--journal") || journal_override.is_some() {
            return Err(UsageError);
        }
        journal_override = Some(brain_os_value(args, index)?);
        index += 2;
    }
    Ok(BrainRuntimeFailureOptions { journal_override })
}

fn parse_brain_inspect(args: &[OsString]) -> Result<BrainInspectOptions, UsageError> {
    let mut journal_override = None;
    let mut bundled_runtime_fingerprint_sha256 = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            journal_override = Some(brain_os_value(args, index)?);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--bundled-runtime-fingerprint") {
            if bundled_runtime_fingerprint_sha256.is_some() {
                return Err(UsageError);
            }
            bundled_runtime_fingerprint_sha256 = Some(brain_sha256_value(args, index)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(BrainInspectOptions {
        journal_override,
        bundled_runtime_fingerprint_sha256,
    })
}

fn parse_brain_fingerprint(args: &[OsString]) -> Result<(), UsageError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(UsageError)
    }
}

fn brain_os_value(args: &[OsString], index: usize) -> Result<OsString, UsageError> {
    let value = args.get(index + 1).ok_or(UsageError)?;
    if value.to_str().is_some_and(|value| value.starts_with('-')) {
        return Err(UsageError);
    }
    Ok(value.clone())
}

fn brain_string_value(args: &[OsString], index: usize) -> Result<String, UsageError> {
    let value = brain_os_value(args, index)?;
    let value = value.into_string().map_err(|_| UsageError)?;
    if value.is_empty() {
        return Err(UsageError);
    }
    Ok(value)
}

fn brain_sha256_value(args: &[OsString], index: usize) -> Result<String, UsageError> {
    let value = brain_string_value(args, index)?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UsageError);
    }
    Ok(value)
}

fn parse_journal_config_read(args: &[OsString]) -> Result<JournalConfigReadOptions, UsageError> {
    let mut journal_override = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(JournalConfigReadOptions { journal_override })
}

fn parse_journal_config_commit(
    args: &[OsString],
) -> Result<JournalConfigCommitOptions, UsageError> {
    let mut journal_override = None;
    let mut lock_timeout_ms = None;
    let mut expect = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        if arg == OsStr::new("--lock-timeout-ms") {
            if lock_timeout_ms.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            let value = value.to_str().ok_or(UsageError)?;
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(UsageError);
            }
            let timeout = value.parse::<u64>().map_err(|_| UsageError)?;
            if timeout == 0 {
                return Err(UsageError);
            }
            lock_timeout_ms = Some(timeout);
            index += 2;
            continue;
        }
        if arg == OsStr::new("--expect") {
            if expect.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_journal_config_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            expect = Some(parse_journal_config_expect(value)?);
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(JournalConfigCommitOptions {
        journal_override,
        lock_timeout_ms,
        expect: expect.ok_or(UsageError)?,
    })
}

fn parse_journal_config_expect(value: &OsString) -> Result<JournalConfigExpectArg, UsageError> {
    if value == OsStr::new("absent") {
        return Ok(JournalConfigExpectArg::Absent);
    }
    let value = value.to_str().ok_or(UsageError)?;
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(UsageError);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UsageError);
    }
    Ok(JournalConfigExpectArg::Sha256(value.to_owned()))
}

fn is_journal_config_flag(value: &OsStr) -> bool {
    matches!(
        value.to_str(),
        Some("--journal" | "--lock-timeout-ms" | "--expect")
    )
}

fn parse_spl(args: &[OsString]) -> Result<SplCommand, UsageError> {
    match args {
        [command, rest @ ..] if command == OsStr::new("service") => {
            parse_service(rest).map(SplCommand::Service)
        }
        _ => Err(UsageError),
    }
}

fn parse_service(args: &[OsString]) -> Result<ServiceOptions, UsageError> {
    let mut verbose = false;
    let mut debug = false;
    for argument in args {
        let argument = argument.as_os_str();
        if argument == OsStr::new("-v") || argument == OsStr::new("--verbose") {
            if verbose {
                return Err(UsageError);
            }
            verbose = true;
            continue;
        }
        if argument == OsStr::new("-d") || argument == OsStr::new("--debug") {
            if debug {
                return Err(UsageError);
            }
            debug = true;
            continue;
        }
        return Err(UsageError);
    }
    Ok(ServiceOptions { verbose, debug })
}

fn parse_journal_path(args: &[OsString]) -> Result<JournalPathOptions, UsageError> {
    let mut journal_override = None;
    let mut create = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--create") {
            if create {
                return Err(UsageError);
            }
            create = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if value == OsStr::new("--create") || value == OsStr::new("--journal") {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(JournalPathOptions {
        journal_override,
        create,
    })
}

fn parse_indexer(args: &[OsString]) -> Result<IndexerCommand, UsageError> {
    match args {
        [verb, rest @ ..] if verb == OsStr::new("search") => {
            parse_indexer_search(rest).map(IndexerCommand::Search)
        }
        [verb, rest @ ..] if verb == OsStr::new("counts") => {
            parse_indexer_counts(rest).map(IndexerCommand::Counts)
        }
        [verb, rest @ ..] if verb == OsStr::new("agents") => {
            parse_indexer_read(rest).map(IndexerCommand::Agents)
        }
        [verb, rest @ ..] if verb == OsStr::new("coverage") => {
            parse_indexer_read(rest).map(IndexerCommand::Coverage)
        }
        _ => parse_indexer_maintenance(args).map(IndexerCommand::Maintenance),
    }
}

fn parse_indexer_maintenance(args: &[OsString]) -> Result<IndexerOptions, UsageError> {
    let mut journal_override = None;
    let mut reset = false;
    let mut rebuild_edges = false;
    let mut rescan = false;
    let mut rescan_full = false;
    let mut rescan_file = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--reset") {
            if reset {
                return Err(UsageError);
            }
            reset = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--rebuild-edges") {
            if rebuild_edges {
                return Err(UsageError);
            }
            rebuild_edges = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--rescan") {
            if rescan {
                return Err(UsageError);
            }
            rescan = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--rescan-full") {
            if rescan_full {
                return Err(UsageError);
            }
            rescan_full = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_maintenance_indexer_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        if arg == OsStr::new("--rescan-file") {
            if rescan_file.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_maintenance_indexer_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            rescan_file = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }

    if rescan_file.is_some() && (rescan || rescan_full) {
        return Err(UsageError);
    }

    Ok(IndexerOptions {
        journal_override,
        reset,
        rebuild_edges,
        rescan,
        rescan_full,
        rescan_file,
    })
}

fn is_maintenance_indexer_flag(value: &OsStr) -> bool {
    matches!(
        value.to_str(),
        Some(
            "--journal"
                | "--reset"
                | "--rebuild-edges"
                | "--rescan"
                | "--rescan-full"
                | "--rescan-file",
        )
    )
}

fn parse_indexer_search(args: &[OsString]) -> Result<IndexerSearchOptions, UsageError> {
    let parsed = parse_indexer_query(args, true)?;
    Ok(IndexerSearchOptions {
        query: parsed.query,
        limit: parsed.limit,
        offset: parsed.offset,
        counts: parsed.counts,
        order: parsed.order,
    })
}

fn parse_indexer_counts(args: &[OsString]) -> Result<IndexerCountsOptions, UsageError> {
    let parsed = parse_indexer_query(args, false)?;
    Ok(IndexerCountsOptions {
        query: parsed.query,
    })
}

fn parse_indexer_read(args: &[OsString]) -> Result<IndexerReadOptions, UsageError> {
    let mut journal_override = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_query_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        return Err(UsageError);
    }
    Ok(IndexerReadOptions {
        journal_override,
        json,
    })
}

struct ParsedIndexerQuery {
    query: IndexerQueryOptions,
    limit: usize,
    offset: usize,
    counts: bool,
    order: String,
}

fn parse_indexer_query(
    args: &[OsString],
    allow_search_options: bool,
) -> Result<ParsedIndexerQuery, UsageError> {
    let mut query = None;
    let mut journal_override = None;
    let mut json = false;
    let mut day = None;
    let mut day_from = None;
    let mut day_to = None;
    let mut facet = None;
    let mut agent = None;
    let mut stream = None;
    let mut time_bucket = None;
    let mut relax = false;
    let mut limit = 10;
    let mut offset = 0;
    let mut counts = false;
    let mut order = "relevance".to_string();
    let mut limit_seen = false;
    let mut offset_seen = false;
    let mut order_seen = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_os_str();
        if arg == OsStr::new("--json") {
            if json {
                return Err(UsageError);
            }
            json = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--relax") {
            if relax {
                return Err(UsageError);
            }
            relax = true;
            index += 1;
            continue;
        }
        if arg == OsStr::new("--journal") {
            if journal_override.is_some() {
                return Err(UsageError);
            }
            let value = args.get(index + 1).ok_or(UsageError)?;
            if is_query_flag(value.as_os_str()) {
                return Err(UsageError);
            }
            journal_override = Some(value.clone());
            index += 2;
            continue;
        }
        let string_slot = match arg.to_str() {
            Some("--day") => Some(&mut day),
            Some("--day-from") => Some(&mut day_from),
            Some("--day-to") => Some(&mut day_to),
            Some("--facet") => Some(&mut facet),
            Some("--agent") => Some(&mut agent),
            Some("--stream") => Some(&mut stream),
            Some("--time-bucket") => Some(&mut time_bucket),
            _ => None,
        };
        if let Some(slot) = string_slot {
            if slot.is_some() {
                return Err(UsageError);
            }
            let value = query_value(args, index)?;
            *slot = Some(value);
            index += 2;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--limit") {
            if limit_seen {
                return Err(UsageError);
            }
            limit = parse_usize_option(args, index)?;
            limit_seen = true;
            index += 2;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--offset") {
            if offset_seen {
                return Err(UsageError);
            }
            offset = parse_usize_option(args, index)?;
            offset_seen = true;
            index += 2;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--counts") {
            if counts {
                return Err(UsageError);
            }
            counts = true;
            index += 1;
            continue;
        }
        if allow_search_options && arg == OsStr::new("--order") {
            if order_seen {
                return Err(UsageError);
            }
            order = query_value(args, index)?;
            order_seen = true;
            index += 2;
            continue;
        }
        if arg.to_str().is_some_and(|value| value.starts_with('-')) || query.is_some() {
            return Err(UsageError);
        }
        query = Some(arg.to_str().ok_or(UsageError)?.to_string());
        index += 1;
    }
    Ok(ParsedIndexerQuery {
        query: IndexerQueryOptions {
            journal_override,
            json,
            query,
            day,
            day_from,
            day_to,
            facet,
            agent,
            stream,
            time_bucket,
            relax,
        },
        limit,
        offset,
        counts,
        order,
    })
}

fn query_value(args: &[OsString], index: usize) -> Result<String, UsageError> {
    let value = args.get(index + 1).ok_or(UsageError)?;
    if is_query_flag(value.as_os_str()) {
        return Err(UsageError);
    }
    value.to_str().map(str::to_string).ok_or(UsageError)
}

fn parse_usize_option(args: &[OsString], index: usize) -> Result<usize, UsageError> {
    query_value(args, index)?.parse().map_err(|_| UsageError)
}

fn is_query_flag(value: &OsStr) -> bool {
    value.to_str().is_some_and(|value| value.starts_with('-'))
}

pub fn version_line(version: &str) -> String {
    format!("solstone-core {version}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn indexer(command: IndexerCommand) -> Command {
        Command::Indexer(Box::new(command))
    }

    #[test]
    fn accepts_version_flag() {
        assert_eq!(evaluate_args(&args(&["--version"])), Ok(Command::Version));
    }

    #[test]
    fn rejects_empty_args() {
        assert_eq!(evaluate_args(&args(&[])), Err(UsageError));
    }

    #[test]
    fn rejects_unknown_args() {
        assert_eq!(evaluate_args(&args(&["--unknown"])), Err(UsageError));
    }

    #[test]
    fn rejects_extra_args() {
        assert_eq!(
            evaluate_args(&args(&["--version", "extra"])),
            Err(UsageError)
        );
    }

    #[test]
    fn accepts_local_probe_nvidia() {
        assert_eq!(
            evaluate_args(&args(&["local", "probe-nvidia"])),
            Ok(Command::Local(LocalCommand::ProbeNvidia))
        );
    }

    #[test]
    fn accepts_local_plan_and_connect() {
        assert_eq!(
            evaluate_args(&args(&["local", "plan"])),
            Ok(Command::Local(LocalCommand::Plan))
        );
        assert_eq!(
            evaluate_args(&args(&["local", "connect"])),
            Ok(Command::Local(LocalCommand::Connect))
        );
    }

    #[test]
    fn rejects_unimplemented_or_extra_local_args() {
        for values in [&["local"][..], &["local", "probe-nvidia", "extra"][..]] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_journal_path() {
        assert_eq!(
            evaluate_args(&args(&["journal-path"])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: None,
                create: false,
            }))
        );
    }

    #[test]
    fn accepts_indexer_without_operation_flags() {
        assert_eq!(
            evaluate_args(&args(&["indexer"])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: None,
                reset: false,
                rebuild_edges: false,
                rescan: false,
                rescan_full: false,
                rescan_file: None,
            })))
        );
    }

    #[test]
    fn accepts_indexer_search_with_filters_and_options() {
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "search",
                "needle",
                "--journal",
                "/tmp/journal",
                "--json",
                "--limit",
                "12",
                "--offset",
                "3",
                "--day-from",
                "20260101",
                "--agent",
                "flow",
                "--relax",
                "--counts",
                "--order",
                "recency",
            ])),
            Ok(indexer(IndexerCommand::Search(IndexerSearchOptions {
                query: IndexerQueryOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                    json: true,
                    query: Some("needle".to_string()),
                    day: None,
                    day_from: Some("20260101".to_string()),
                    day_to: None,
                    facet: None,
                    agent: Some("flow".to_string()),
                    stream: None,
                    time_bucket: None,
                    relax: true,
                },
                limit: 12,
                offset: 3,
                counts: true,
                order: "recency".to_string(),
            })))
        );
    }

    #[test]
    fn accepts_indexer_counts_and_read_verbs() {
        assert_eq!(
            evaluate_args(&args(&["indexer", "counts", "--facet", "work"])),
            Ok(indexer(IndexerCommand::Counts(IndexerCountsOptions {
                query: IndexerQueryOptions {
                    journal_override: None,
                    json: false,
                    query: None,
                    day: None,
                    day_from: None,
                    day_to: None,
                    facet: Some("work".to_string()),
                    agent: None,
                    stream: None,
                    time_bucket: None,
                    relax: false,
                },
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["indexer", "agents", "--json"])),
            Ok(indexer(IndexerCommand::Agents(IndexerReadOptions {
                journal_override: None,
                json: true,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["indexer", "coverage"])),
            Ok(indexer(IndexerCommand::Coverage(IndexerReadOptions {
                journal_override: None,
                json: false,
            })))
        );
    }

    #[test]
    fn accepts_journal_config_read_and_commit() {
        assert_eq!(
            evaluate_args(&args(&[
                "journal-config",
                "read",
                "--journal",
                "/tmp/journal"
            ])),
            Ok(Command::JournalConfig(JournalConfigCommand::Read(
                JournalConfigReadOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "journal-config",
                "commit",
                "--lock-timeout-ms",
                "25",
                "--expect",
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::JournalConfig(JournalConfigCommand::Commit(
                JournalConfigCommitOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                    lock_timeout_ms: Some(25),
                    expect: JournalConfigExpectArg::Sha256(
                        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                            .to_owned()
                    ),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&["journal-config", "commit", "--expect", "absent"])),
            Ok(Command::JournalConfig(JournalConfigCommand::Commit(
                JournalConfigCommitOptions {
                    journal_override: None,
                    lock_timeout_ms: None,
                    expect: JournalConfigExpectArg::Absent,
                }
            )))
        );
    }

    #[test]
    fn accepts_brain_verbs() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "refresh",
                "--session",
                "--journal",
                "/tmp/journal",
                "--run-id",
                "run-1",
                "--expect-fingerprint",
                hash,
                "--bundled-runtime-fingerprint",
                hash,
            ])),
            Ok(Command::Brain(BrainCommand::RefreshSession(
                BrainRefreshSessionOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                    run_id: Some("run-1".to_owned()),
                    expect: Some(BrainRefreshExpectArg::Sha256(hash.to_owned())),
                    bundled_runtime_fingerprint_sha256: Some(hash.to_owned()),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(
                &["brain", "refresh", "--expect-absent", "--session",]
            )),
            Ok(Command::Brain(BrainCommand::RefreshSession(
                BrainRefreshSessionOptions {
                    journal_override: None,
                    run_id: None,
                    expect: Some(BrainRefreshExpectArg::Absent),
                    bundled_runtime_fingerprint_sha256: None,
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--run-id",
                "renew-1",
                "--expect-fingerprint",
                hash,
                "--bundled-runtime-fingerprint",
                hash,
            ])),
            Ok(Command::Brain(BrainCommand::PrerequisiteRenewalSession(
                BrainPrerequisiteRenewalSessionOptions {
                    journal_override: None,
                    run_id: Some("renew-1".to_owned()),
                    expected_fingerprint_sha256: Some(hash.to_owned()),
                    bundled_runtime_fingerprint_sha256: Some(hash.to_owned()),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "record-runtime-failure",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::Brain(BrainCommand::RecordRuntimeFailure(
                BrainRuntimeFailureOptions {
                    journal_override: Some(OsString::from("/tmp/journal")),
                }
            )))
        );
        assert_eq!(
            evaluate_args(&args(&[
                "brain",
                "inspect",
                "--journal",
                "/tmp/journal",
                "--bundled-runtime-fingerprint",
                hash,
            ])),
            Ok(Command::Brain(BrainCommand::Inspect(BrainInspectOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                bundled_runtime_fingerprint_sha256: Some(hash.to_owned()),
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["brain", "inspect"])),
            Ok(Command::Brain(BrainCommand::Inspect(BrainInspectOptions {
                journal_override: None,
                bundled_runtime_fingerprint_sha256: None,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["brain", "fingerprint"])),
            Ok(Command::Brain(BrainCommand::Fingerprint))
        );
    }

    #[test]
    fn rejects_invalid_brain_args() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        for values in [
            &["brain"][..],
            &["brain", "unknown"][..],
            &["brain", "refresh"][..],
            &["brain", "prerequisite-renewal"][..],
            &[
                "brain",
                "refresh",
                "--session",
                "--expect-fingerprint",
                hash,
                "--expect-absent",
            ][..],
            &["brain", "refresh", "--session", "--session"][..],
            &["brain", "refresh", "--session", "--run-id"][..],
            &[
                "brain",
                "refresh",
                "--session",
                "--journal",
                "--run-id",
                "id",
            ][..],
            &[
                "brain",
                "refresh",
                "--session",
                "--bundled-runtime-fingerprint",
                "bad",
            ][..],
            &[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--expect-absent",
            ][..],
            &[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--expect-fingerprint",
                "bad",
            ][..],
            &[
                "brain",
                "prerequisite-renewal",
                "--session",
                "--expect-fingerprint",
                hash,
                "--expect-fingerprint",
                hash,
            ][..],
            &["brain", "record-runtime-failure", "--journal"][..],
            &["brain", "inspect", "--unknown"][..],
            &["brain", "fingerprint", "--journal", "/tmp/journal"][..],
            &["brain", "inspect", "--journal"][..],
            &["brain", "inspect", "--bundled-runtime-fingerprint", "bad"][..],
            &["brain", "inspect", "--journal", "/a", "--journal", "/b"][..],
            &[
                "brain",
                "record-runtime-failure",
                "--journal",
                "/a",
                "--journal",
                "/b",
            ][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_indexer_verb_unknown_duplicate_and_disallowed_options() {
        for values in [
            &["indexer", "search", "--limit", "10", "--limit", "10"][..],
            &["indexer", "search", "needle", "second"][..],
            &["indexer", "counts", "--limit", "10"][..],
            &["indexer", "agents", "needle"][..],
            &["indexer", "coverage", "--day", "20260101"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_invalid_journal_config_args() {
        for values in [
            &["journal-config"][..],
            &["journal-config", "unknown"][..],
            &["journal-config", "read", "--expect", "absent"][..],
            &["journal-config", "read", "--journal"][..],
            &["journal-config", "read", "--journal", "--expect"][..],
            &["journal-config", "commit"][..],
            &["journal-config", "commit", "--expect"][..],
            &["journal-config", "commit", "--expect", "bogus"][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--expect",
                "absent",
            ][..],
            &[
                "journal-config",
                "commit",
                "--journal",
                "/a",
                "--journal",
                "/b",
                "--expect",
                "absent",
            ][..],
            &[
                "journal-config",
                "commit",
                "--lock-timeout-ms",
                "10",
                "--lock-timeout-ms",
                "20",
                "--expect",
                "absent",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "0",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "-1",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "1ms",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--lock-timeout-ms",
                "18446744073709551616",
            ][..],
            &[
                "journal-config",
                "commit",
                "--expect",
                "absent",
                "--journal",
                "--lock-timeout-ms",
            ][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_spl_service() {
        assert_eq!(
            evaluate_args(&args(&["spl", "service"])),
            Ok(Command::Spl(SplCommand::Service(ServiceOptions {
                verbose: false,
                debug: false,
            })))
        );
    }

    #[test]
    fn accepts_spl_service_verbose_and_debug_flags_in_either_order() {
        assert_eq!(
            evaluate_args(&args(&["spl", "service", "-v", "--debug"])),
            Ok(Command::Spl(SplCommand::Service(ServiceOptions {
                verbose: true,
                debug: true,
            })))
        );
        assert_eq!(
            evaluate_args(&args(&["spl", "service", "-d", "--verbose"])),
            Ok(Command::Spl(SplCommand::Service(ServiceOptions {
                verbose: true,
                debug: true,
            })))
        );
    }

    #[test]
    fn accepts_each_spl_service_flag() {
        for (flag, expected) in [
            (
                "-v",
                ServiceOptions {
                    verbose: true,
                    debug: false,
                },
            ),
            (
                "--verbose",
                ServiceOptions {
                    verbose: true,
                    debug: false,
                },
            ),
            (
                "-d",
                ServiceOptions {
                    verbose: false,
                    debug: true,
                },
            ),
            (
                "--debug",
                ServiceOptions {
                    verbose: false,
                    debug: true,
                },
            ),
        ] {
            assert_eq!(
                evaluate_args(&args(&["spl", "service", flag])),
                Ok(Command::Spl(SplCommand::Service(expected))),
                "{flag}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_or_unknown_spl_service_flags() {
        for values in [
            &["spl", "service", "-v", "-v"][..],
            &["spl", "service", "--verbose", "--verbose"][..],
            &["spl", "service", "-v", "--verbose"][..],
            &["spl", "service", "-d", "-d"][..],
            &["spl", "service", "--debug", "--debug"][..],
            &["spl", "service", "-d", "--debug"][..],
            &["spl", "service", "--unknown"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_spl_service_extra_args() {
        for values in [
            &["spl", "service", "extra"][..],
            &["spl", "service", "service"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn rejects_incomplete_unknown_and_extra_spl_args() {
        for values in [&["spl"][..], &["spl", "unknown"][..]] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_indexer_rescan_full_reset_and_override() {
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "--journal",
                "/tmp/journal",
                "--reset",
                "--rescan-full",
            ])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                reset: true,
                rebuild_edges: false,
                rescan: false,
                rescan_full: true,
                rescan_file: None,
            })))
        );
    }

    #[test]
    fn accepts_indexer_rescan_file() {
        assert_eq!(
            evaluate_args(&args(&[
                "indexer",
                "--rescan-file",
                "20240101/talents/flow.md",
            ])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: None,
                reset: false,
                rebuild_edges: false,
                rescan: false,
                rescan_full: false,
                rescan_file: Some(OsString::from("20240101/talents/flow.md")),
            })))
        );
    }

    #[test]
    fn accepts_indexer_rebuild_edges_composed_with_rescan() {
        assert_eq!(
            evaluate_args(&args(&["indexer", "--rebuild-edges", "--rescan"])),
            Ok(indexer(IndexerCommand::Maintenance(IndexerOptions {
                journal_override: None,
                reset: false,
                rebuild_edges: true,
                rescan: true,
                rescan_full: false,
                rescan_file: None,
            })))
        );
    }

    #[test]
    fn rejects_indexer_conflicts_missing_values_and_duplicates() {
        for values in [
            &["indexer", "--rescan-file"][..],
            &["indexer", "--journal"][..],
            &["indexer", "--rescan-file", "--rescan"][..],
            &["indexer", "--journal", "--reset"][..],
            &["indexer", "--reset", "--reset"][..],
            &["indexer", "--rebuild-edges", "--rebuild-edges"][..],
            &["indexer", "--rescan-file", "a.md", "--rescan"][..],
            &["indexer", "--rescan-file", "a.md", "--rescan-full"][..],
            &["indexer", "--unknown"][..],
        ] {
            assert_eq!(evaluate_args(&args(values)), Err(UsageError), "{values:?}");
        }
    }

    #[test]
    fn accepts_journal_path_create() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--create"])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: None,
                create: true,
            }))
        );
    }

    #[test]
    fn accepts_journal_path_override() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--journal", "/tmp/journal"])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                create: false,
            }))
        );
    }

    #[test]
    fn accepts_journal_path_override_create() {
        assert_eq!(
            evaluate_args(&args(&[
                "journal-path",
                "--journal",
                "/tmp/journal",
                "--create",
            ])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                create: true,
            }))
        );
    }

    #[test]
    fn accepts_journal_path_create_override() {
        assert_eq!(
            evaluate_args(&args(&[
                "journal-path",
                "--create",
                "--journal",
                "/tmp/journal",
            ])),
            Ok(Command::JournalPath(JournalPathOptions {
                journal_override: Some(OsString::from("/tmp/journal")),
                create: true,
            }))
        );
    }

    #[test]
    fn rejects_journal_missing_value() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--journal"])),
            Err(UsageError)
        );
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--journal", "--create"])),
            Err(UsageError)
        );
    }

    #[test]
    fn rejects_journal_path_unknown_flags() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--unknown"])),
            Err(UsageError)
        );
    }

    #[test]
    fn rejects_journal_path_duplicate_flags() {
        assert_eq!(
            evaluate_args(&args(&["journal-path", "--create", "--create"])),
            Err(UsageError)
        );
        assert_eq!(
            evaluate_args(&args(&[
                "journal-path",
                "--journal",
                "/a",
                "--journal",
                "/b",
            ])),
            Err(UsageError)
        );
    }

    #[test]
    fn formats_version_line() {
        assert_eq!(version_line("1.2.3"), "solstone-core 1.2.3\n");
    }

    #[test]
    fn usage_lists_supported_commands() {
        assert_eq!(
            USAGE,
            "Usage:\n  solstone-core --version\n  solstone-core journal-path [--journal PATH] [--create]\n  solstone-core indexer [--journal PATH] [--reset] [--rebuild-edges] [--rescan | --rescan-full | --rescan-file PATH]\n  solstone-core indexer search [QUERY] [--journal PATH] [--json] [--limit N] [--offset N] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax] [--counts] [--order relevance|recency]\n  solstone-core indexer counts [QUERY] [--journal PATH] [--json] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--time-bucket BUCKET] [--relax]\n  solstone-core indexer agents [--journal PATH] [--json]\n  solstone-core indexer coverage [--journal PATH] [--json]\n  solstone-core journal-config read [--journal PATH]\n  solstone-core journal-config commit [--journal PATH] [--lock-timeout-ms N] --expect <fingerprint|absent>\n  solstone-core local probe-nvidia\n  solstone-core local plan\n  solstone-core local connect\n  solstone-core brain refresh --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256 | --expect-absent] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain prerequisite-renewal --session [--journal PATH] [--run-id ID] [--expect-fingerprint SHA256] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain record-runtime-failure [--journal PATH]\n  solstone-core brain inspect [--journal PATH] [--bundled-runtime-fingerprint SHA256]\n  solstone-core brain fingerprint\n  solstone-core spl service [-v | --verbose] [-d | --debug]\n"
        );
    }
}
